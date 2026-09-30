//! Cancellation-safe Thread SSE observation; no background tasks or Run mutations.
use crate::{
    BinaryResponse, Error, ProtocolError, ProtocolKind, TransportError, TransportKind,
    TransportStage,
    resources::{ThreadResource, ThreadStreamGetOptions},
};
use bytes::{Buf, Bytes};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    TextMessage,
    ReasoningMessage,
    ToolCall,
    Observation,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    InProgress,
    Completed,
    Interrupted,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ItemRef {
    pub id: String,
    pub kind: ItemKind,
    pub state: ItemState,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Boundary {
    pub run_id: String,
    pub attempt: i64,
    pub sequence: i64,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Delta {
    pub run_id: String,
    pub attempt: i64,
    pub sequence: i64,
    pub event: Map<String, Value>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub item: Option<ItemRef>,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Changed {
    pub version: i64,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct RunSignal {
    pub run_id: String,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GapSignal {
    pub run_id: String,
    /// Coverage that readback must reach; absent/null when the target is unknown.
    pub position: Option<String>,
}
/// Only data/checkpoint frames carry resumable cursors; hints require readback.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadFrame {
    Delta { cursor: String, data: Delta },
    Boundary { cursor: String, data: Boundary },
    Changed(Changed),
    Gap(GapSignal),
    Reset(RunSignal),
}
impl ThreadFrame {
    pub fn cursor(&self) -> Option<&str> {
        match self {
            Self::Delta { cursor, .. } | Self::Boundary { cursor, .. } => Some(cursor),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct StreamOptions {
    /// Last applied Redis cursor; a hint when Run/position coverage is supplied.
    pub after: Option<String>,
    /// Run owning the supplied committed/applied display position. Requires `position`.
    pub run: Option<String>,
    /// Applied display coverage (`attempt-sequence`), not a Redis cursor. Requires `run`.
    pub position: Option<String>,
    /// Zero disables reconnection; acknowledged new cursor progress resets this budget.
    pub max_reconnects: usize,
    pub max_frame_bytes: usize,
    pub reconnect_delay: Duration,
}
impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            after: None,
            run: None,
            position: None,
            max_reconnects: 0,
            max_frame_bytes: 1 << 20,
            reconnect_delay: Duration::from_millis(250),
        }
    }
}
/// One mutable reader. A new `next` poll acknowledges the previous cursor frame.
/// Drop or `close` never acknowledges pending data. Dropped read futures retain
/// parser and recovery state, so a timeout cannot silently discard half a frame.
pub struct ThreadStream<'a> {
    thread: ThreadResource<'a>,
    options: StreamOptions,
    response: Option<BinaryResponse>,
    parser: Parser,
    applied: Option<String>,
    received: Option<String>,
    pending: Option<String>,
    pending_position: Option<String>,
    applied_position: Option<String>,
    coverage_lost: bool,
    retries: usize,
    retry_at: Option<Instant>,
    closed: bool,
}
impl<'a> ThreadStream<'a> {
    /// Advanced Thread-wide protocol reader; not a finite Agent interaction.
    pub async fn open(thread: ThreadResource<'a>, options: StreamOptions) -> Result<Self, Error> {
        if options.max_frame_bytes == 0
            || options.after.as_deref().is_some_and(|v| !valid_cursor(v))
            || options.run.is_some() != options.position.is_some()
            || options.run.as_deref().is_some_and(str::is_empty)
            || options
                .position
                .as_deref()
                .is_some_and(|v| !valid_position(v))
        {
            return Err(Error::InvalidInput);
        }
        let mut stream = Self {
            thread,
            parser: Parser::new(options.max_frame_bytes),
            applied: options.after.clone(),
            applied_position: options.position.clone(),
            options,
            response: None,
            received: None,
            pending: None,
            pending_position: None,
            coverage_lost: false,
            retries: 0,
            retry_at: None,
            closed: false,
        };
        stream.connect().await?;
        Ok(stream)
    }
}
impl ThreadStream<'_> {
    pub fn applied_cursor(&self) -> Option<&str> {
        self.applied.as_deref()
    }
    /// Last contiguous applied coverage for the claimed Run. A gap/reset stops advancement;
    /// reopen from covering Run Items to establish a new baseline.
    pub fn applied_position(&self) -> Option<&str> {
        self.applied_position.as_deref()
    }
    pub fn last_received_cursor(&self) -> Option<&str> {
        self.received.as_deref()
    }
    pub fn response(&self) -> Option<(reqwest::StatusCode, &reqwest::header::HeaderMap)> {
        self.response.as_ref().map(|r| (r.status, &r.headers))
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.response = None;
        self.parser = Parser::new(self.options.max_frame_bytes);
    }
    fn check_parent(&mut self) -> Result<(), Error> {
        if self.thread.0.client.shutdown.is_cancelled() {
            self.close();
            return Err(Error::Closed);
        }
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<ThreadFrame>, Error> {
        self.check_parent()?;
        if self.closed {
            return Ok(None);
        }
        if let Some(cursor) = self.pending.take() {
            if self.applied.as_ref() != Some(&cursor) {
                self.retries = 0;
            }
            self.applied = Some(cursor);
            if let Some(position) = self.pending_position.take() {
                self.applied_position = Some(position);
            }
        }
        let result = self.read().await;
        if result.is_err() {
            self.close();
        }
        result
    }
    async fn read(&mut self) -> Result<Option<ThreadFrame>, Error> {
        loop {
            self.check_parent()?;
            if self.response.is_none() {
                self.connect().await?;
            }
            if let Some(frame) = self.parser.next()? {
                self.check_parent()?;
                self.track_coverage(&frame);
                if let Some(cursor) = frame.cursor() {
                    self.pending = Some(cursor.into());
                    self.received = self.pending.clone();
                }
                return Ok(Some(frame));
            }
            let chunk = self.response.as_mut().ok_or(Error::Closed)?.chunk().await;
            match chunk {
                Ok(Some(chunk)) => self.parser.chunk = chunk,
                Ok(None) => {
                    self.parser.finish()?;
                    self.response = None;
                    if self.options.max_reconnects == 0 {
                        self.close();
                        return Ok(None);
                    }
                    self.schedule(Error::Transport(TransportError {
                        stage: TransportStage::Body,
                        kind: TransportKind::StreamEnded,
                    }))?;
                }
                Err(error) => {
                    self.response = None;
                    self.schedule(error)?;
                }
            }
        }
    }
    // A coverage claim advances only with contiguous frames the caller applied.
    // Boundaries are still observable but cannot claim delivery across a hole.
    fn track_coverage(&mut self, frame: &ThreadFrame) {
        let Some(run) = self.options.run.as_deref() else {
            return;
        };
        let (run_id, attempt, sequence, delta) = match frame {
            ThreadFrame::Gap(data) if data.run_id == run => {
                self.coverage_lost = true;
                return;
            }
            ThreadFrame::Reset(data) if data.run_id == run => {
                self.coverage_lost = true;
                return;
            }
            ThreadFrame::Delta { data, .. } => (&data.run_id, data.attempt, data.sequence, true),
            ThreadFrame::Boundary { data, .. } => {
                (&data.run_id, data.attempt, data.sequence, false)
            }
            _ => return,
        };
        if run_id != run || self.coverage_lost {
            return;
        }
        let Some((covered_attempt, covered_sequence)) = self
            .applied_position
            .as_deref()
            .and_then(|v| v.split_once('-'))
            .and_then(|(a, s)| Some((a.parse::<u128>().ok()?, s.parse::<u128>().ok()?)))
        else {
            return;
        };
        if attempt < 0 || sequence < 0 || attempt as u128 != covered_attempt {
            self.coverage_lost = true;
            return;
        }
        let sequence = sequence as u128;
        if delta && sequence == covered_sequence + 1 {
            self.pending_position = Some(format!("{attempt}-{sequence}"));
        } else if sequence > covered_sequence {
            self.coverage_lost = true;
        }
    }
    // The deadline is kept on the stream, not inside the dropped read future.
    fn schedule(&mut self, error: Error) -> Result<(), Error> {
        self.check_parent()?;
        let transient = match &error {
            Error::Transport(_) => true,
            Error::Api(api) => matches!(api.status, 429 | 502 | 503 | 504),
            _ => false,
        };
        if !transient || self.retries >= self.options.max_reconnects {
            return Err(error);
        }
        let mut delay = self.options.reconnect_delay;
        if let Error::Api(api) = &error
            && let Some(advice) = &api.retry_after
        {
            if let Ok(seconds) = advice.parse::<u64>() {
                if seconds <= 86400 {
                    delay = delay.max(Duration::from_secs(seconds));
                }
            } else if let Ok(date) = chrono::DateTime::parse_from_rfc2822(advice)
                && let Ok(until) = date.signed_duration_since(chrono::Utc::now()).to_std()
            {
                delay = delay.max(until);
            }
        }
        self.retry_at = Some(
            Instant::now()
                .checked_add(delay)
                .ok_or(Error::InvalidInput)?,
        );
        self.retries += 1;
        Ok(())
    }
    async fn connect(&mut self) -> Result<(), Error> {
        loop {
            self.check_parent()?;
            if let Some(deadline) = self.retry_at {
                self.thread
                    .0
                    .client
                    .observe(async {
                        tokio::time::sleep_until(deadline).await;
                        Ok(())
                    })
                    .await?;
                self.retry_at = None;
            }
            let response = self
                .thread
                .stream()
                .get(ThreadStreamGetOptions {
                    last_event_id: self.applied.clone(),
                    run: self.options.run.clone(),
                    position: self.applied_position.clone(),
                    ..Default::default()
                })
                .await;
            match response {
                Ok(response) => {
                    let kind = response
                        .headers
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.split(';').next());
                    if !kind.is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream")) {
                        return Err(ProtocolError::response(
                            ProtocolKind::UnexpectedContentType,
                            response.status.as_u16(),
                            &response.headers,
                        ));
                    }
                    self.response = Some(response);
                    self.parser = Parser::new(self.options.max_frame_bytes);
                    return Ok(());
                }
                Err(error) => self.schedule(error)?,
            }
        }
    }
}
fn valid_cursor(value: &str) -> bool {
    value.split_once('-').is_some_and(|(a, b)| {
        [a, b]
            .iter()
            .all(|v| !v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_digit()))
    })
}
fn valid_position(value: &str) -> bool {
    valid_cursor(value)
        && value
            .split_once('-')
            .is_some_and(|(a, b)| [a, b].iter().all(|v| v.len() == 1 || !v.starts_with('0')))
}
fn decode(event: &str, cursor: Option<String>, data: &str) -> Result<ThreadFrame, Error> {
    fn parse<T: serde::de::DeserializeOwned>(data: &str) -> Result<T, Error> {
        serde_json::from_str(data).map_err(|_| ProtocolError::local(ProtocolKind::InvalidFrame))
    }
    if matches!(event, "delta" | "boundary") {
        let cursor = cursor
            .filter(|v| valid_cursor(v))
            .ok_or_else(|| ProtocolError::local(ProtocolKind::InvalidFrame))?;
        if event == "boundary" {
            let data: Boundary = parse(data)?;
            if data.run_id.is_empty() {
                return Err(ProtocolError::local(ProtocolKind::InvalidFrame));
            }
            return Ok(ThreadFrame::Boundary { cursor, data });
        }
        let data: Delta = parse(data)?;
        if data.run_id.is_empty() || data.item.as_ref().is_some_and(|i| i.id.is_empty()) {
            return Err(ProtocolError::local(ProtocolKind::InvalidFrame));
        }
        return Ok(ThreadFrame::Delta { cursor, data });
    }
    if cursor.is_some() {
        return Err(ProtocolError::local(ProtocolKind::InvalidFrame));
    }
    match event {
        "changed" => Ok(ThreadFrame::Changed(parse(data)?)),
        "gap" => {
            let data: GapSignal = parse(data)?;
            if data.run_id.is_empty()
                || data.position.as_deref().is_some_and(|v| !valid_position(v))
            {
                return Err(ProtocolError::local(ProtocolKind::InvalidFrame));
            }
            Ok(ThreadFrame::Gap(data))
        }
        "reset" => {
            let data: RunSignal = parse(data)?;
            if data.run_id.is_empty() {
                return Err(ProtocolError::local(ProtocolKind::InvalidFrame));
            }
            Ok(ThreadFrame::Reset(data))
        }
        _ => Err(ProtocolError::local(ProtocolKind::InvalidFrame)),
    }
}
struct Parser {
    chunk: Bytes,
    line: Vec<u8>,
    event: String,
    data: Vec<String>,
    cursor: Option<String>,
    size: usize,
    limit: usize,
    skip_lf: bool,
    first: bool,
}
impl Parser {
    fn new(limit: usize) -> Self {
        Self {
            chunk: Bytes::new(),
            line: Vec::new(),
            event: String::new(),
            data: Vec::new(),
            cursor: None,
            size: 0,
            limit,
            skip_lf: false,
            first: true,
        }
    }
    fn next(&mut self) -> Result<Option<ThreadFrame>, Error> {
        while self.chunk.has_remaining() {
            let byte = self.chunk.get_u8();
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            self.size += 1;
            if self.size > self.limit {
                return Err(ProtocolError::local(ProtocolKind::FrameTooLarge));
            }
            if byte != b'\n' && byte != b'\r' {
                self.line.push(byte);
                continue;
            }
            self.skip_lf = byte == b'\r';
            let raw = std::mem::take(&mut self.line);
            let line = std::str::from_utf8(&raw)
                .map_err(|_| ProtocolError::local(ProtocolKind::InvalidUtf8))?;
            let line = if self.first {
                self.first = false;
                line.strip_prefix('\u{feff}').unwrap_or(line)
            } else {
                line
            };
            if line.is_empty() {
                self.size = 0;
                let event = std::mem::take(&mut self.event);
                let cursor = self.cursor.take();
                let data = std::mem::take(&mut self.data);
                if !data.is_empty() {
                    return decode(&event, cursor, &data.join("\n")).map(Some);
                }
            } else if !line.starts_with(':') {
                let (field, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => self.event = value.into(),
                    "id" => self.cursor = Some(value.into()),
                    "data" => self.data.push(value.into()),
                    _ => {}
                }
            }
        }
        Ok(None)
    }
    fn finish(&self) -> Result<(), Error> {
        if !self.line.is_empty()
            || !self.data.is_empty()
            || !self.event.is_empty()
            || self.cursor.is_some()
        {
            Err(ProtocolError::local(ProtocolKind::IncompleteFrame))
        } else {
            Ok(())
        }
    }
}
