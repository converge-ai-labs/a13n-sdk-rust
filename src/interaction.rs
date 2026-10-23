use crate::{
    Client, Error, ProtocolError, ProtocolKind, Response, SubmissionError,
    generated::models,
    resources::{
        EntryGetOptions, EntryResource, RunGetOptions, RunInterruptOptions, RunResource,
        ThreadResource,
    },
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use std::time::Duration;
use tokio::time::Instant;

const POLL: Duration = Duration::from_millis(500);

/// Fixed settings for one local observer. An absolute deadline includes prior mutation time.
#[derive(Clone, Debug)]
pub struct ObservationOptions {
    pub timeout: Duration,
    pub poll_interval: Duration,
    pub deadline: Option<Instant>,
}
impl Default for ObservationOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),
            poll_interval: POLL,
            deadline: None,
        }
    }
}
impl ObservationOptions {
    pub(crate) fn freeze(mut self) -> Result<Self, Error> {
        if self.timeout.is_zero() || self.poll_interval.is_zero() {
            return Err(Error::InvalidInput);
        }
        self.deadline = Some(match self.deadline {
            Some(deadline) => deadline,
            None => Instant::now()
                .checked_add(self.timeout)
                .ok_or(Error::InvalidInput)?,
        });
        Ok(self)
    }
}

/// Owned local handles share the Client's transport; generated resources still borrow it.
#[derive(Clone)]
pub struct Thread {
    pub id: String,
    client: Client,
}
impl Thread {
    pub fn resource(&self) -> ThreadResource<'_> {
        self.client.api().threads().at(&self.id)
    }
}
#[derive(Clone)]
pub struct Entry {
    pub id: String,
    pub thread_id: String,
    client: Client,
}
impl Entry {
    pub fn resource(&self) -> EntryResource<'_> {
        self.client
            .api()
            .threads()
            .at(&self.thread_id)
            .inbox()
            .at(&self.id)
    }
    /// Recover an exact Entry locally, without a request or a fabricated HTTP receipt.
    pub fn observe(&self, options: ObservationOptions) -> Result<Interaction, Error> {
        Interaction::new(
            self.client.clone(),
            InteractionIdentity {
                thread_id: Some(self.thread_id.clone()),
                entry_id: Some(self.id.clone()),
                run_id: None,
                idempotency_key: None,
            },
            ReceiptEvidence::RecoveredIdentity,
            None,
            options,
        )
    }
    /// Read the complete entry, retaining the HTTP response evidence.
    pub async fn get(&self) -> Result<Response<models::EntryView>, Error> {
        let response = self.resource().get(EntryGetOptions::default()).await?;
        let models::ResponseGetInboxEntry::AnyOf0(data) = response.data else {
            return Err(invalid(&response));
        };
        Ok(Response {
            data,
            status: response.status,
            headers: response.headers,
        })
    }
    /// Advanced disposition-only readback (also used by the CLI inbox wait command).
    pub async fn wait(&self, interval: Duration) -> Result<Response<models::EntryView>, Error> {
        let interval = if interval.is_zero() { POLL } else { interval };
        self.client
            .observe(async {
                loop {
                    let result = self.get().await?;
                    if result.data.id != self.id || result.data.thread_id != self.thread_id {
                        return Err(invalid(&result));
                    }
                    if matches!(
                        result.data.status,
                        models::EntryStatus::Consumed
                            | models::EntryStatus::Failed
                            | models::EntryStatus::Withdrawn
                    ) {
                        return Ok(result);
                    }
                    tokio::time::sleep(interval).await;
                }
            })
            .await
    }
}
#[derive(Clone)]
pub struct Run {
    pub id: String,
    pub thread_id: Option<String>,
    pub(crate) client: Client,
}
impl Run {
    pub fn resource(&self) -> RunResource<'_> {
        self.client.api().runs().at(&self.id)
    }
    /// Advanced committed display/window readback; not a final-reply helper.
    pub fn items(&self) -> crate::resources::RunItemsResource<'_> {
        self.resource().items()
    }
    /// Read the complete run; list metadata cannot satisfy an exact run observation.
    pub async fn get(&self) -> Result<Response<models::RunView>, Error> {
        let response = self.resource().get(RunGetOptions::default()).await?;
        let models::ResponseGetRun::AnyOf0(data) = response.data else {
            return Err(invalid(&response));
        };
        let snapshot = Response {
            data,
            status: response.status,
            headers: response.headers,
        };
        self.check(&snapshot)?;
        Ok(snapshot)
    }
    pub async fn interrupt(&self) -> Result<Response<models::RunView>, Error> {
        let snapshot = self
            .resource()
            .interrupt(RunInterruptOptions::default())
            .await?;
        self.check(&snapshot)?;
        Ok(snapshot)
    }
    pub fn observe(&self, options: ObservationOptions) -> Result<Interaction, Error> {
        Interaction::new(
            self.client.clone(),
            InteractionIdentity {
                thread_id: self.thread_id.clone(),
                entry_id: None,
                run_id: Some(self.id.clone()),
                idempotency_key: None,
            },
            ReceiptEvidence::RecoveredIdentity,
            Some(self.clone()),
            options,
        )
    }
    /// Result-only convenience over the same finite observer, never a successor follower.
    pub async fn result(&self, options: ObservationOptions) -> Result<RunOutcome, Error> {
        self.observe(options)?.result().await
    }
    pub(crate) fn check(&self, snapshot: &Response<models::RunView>) -> Result<(), Error> {
        if snapshot.data.id != self.id
            || snapshot.data.thread_id.is_empty()
            || self
                .thread_id
                .as_ref()
                .is_some_and(|id| id != &snapshot.data.thread_id)
        {
            return Err(invalid(snapshot));
        }
        Ok(())
    }
}
impl Client {
    pub fn run(&self, id: impl Into<String>) -> Run {
        Run {
            id: id.into(),
            thread_id: None,
            client: self.clone(),
        }
    }
    pub fn entry(&self, thread_id: impl Into<String>, id: impl Into<String>) -> Entry {
        Entry {
            id: id.into(),
            thread_id: thread_id.into(),
            client: self.clone(),
        }
    }
    pub fn thread(&self, id: impl Into<String>) -> Thread {
        Thread {
            id: id.into(),
            client: self.clone(),
        }
    }
}

/// Stable intent/observation identity. `run_id` becomes authoritative only after consumption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteractionIdentity {
    pub thread_id: Option<String>,
    pub entry_id: Option<String>,
    pub run_id: Option<String>,
    pub idempotency_key: Option<String>,
}
/// Provenance is explicit. Recovered selectors have no original HTTP acknowledgement.
#[derive(Clone)]
pub enum ReceiptEvidence {
    EntrySubmission(Response<models::Submitted>),
    ResumeRunReceipt(Box<Response<models::RunView>>),
    RecoveredIdentity,
}
impl ReceiptEvidence {
    pub fn submission(&self) -> Option<&Response<models::Submitted>> {
        if let Self::EntrySubmission(receipt) = self {
            Some(receipt)
        } else {
            None
        }
    }
    pub fn resume(&self) -> Option<&Response<models::RunView>> {
        if let Self::ResumeRunReceipt(receipt) = self {
            Some(receipt)
        } else {
            None
        }
    }
}

/// Authoritative exact Run result, with direct workflow fields and explicit HTTP evidence.
#[derive(Clone)]
pub struct RunOutcome {
    pub status: models::RunStatus,
    pub output: serde_json::Value,
    pub text: Option<String>,
    pub pending_approvals: Vec<models::PendingCall>,
    pub pending_calls: Vec<models::PendingCall>,
    pub failure: Option<models::Failure>,
    pub thread_id: String,
    pub run_id: String,
    pub run: Run,
    pub http_response: Response<models::RunView>,
}
impl RunOutcome {
    fn new(mut run: Run, snapshot: Response<models::RunView>) -> Self {
        run.thread_id = Some(snapshot.data.thread_id.clone());
        let output = snapshot
            .data
            .output
            .as_deref()
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Self {
            status: snapshot.data.status.clone(),
            text: output.as_str().map(str::to_owned),
            output,
            pending_approvals: snapshot
                .data
                .pending
                .as_ref()
                .map(|p| p.approvals.clone())
                .unwrap_or_default(),
            pending_calls: snapshot
                .data
                .pending
                .as_ref()
                .map(|p| p.calls.clone())
                .unwrap_or_default(),
            failure: snapshot.data.failure.as_deref().cloned(),
            thread_id: snapshot.data.thread_id.clone(),
            run_id: snapshot.data.id.clone(),
            run,
            http_response: snapshot,
        }
    }
    pub fn decode_output<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.output.clone())
    }
}

/// Advanced acknowledgement binding. `run` here is provisional evidence, not incorporation.
pub struct Submitted {
    pub receipt: Response<models::Submitted>,
    pub thread: Thread,
    pub entry: Entry,
    pub run: Option<Run>,
}
impl Submitted {
    pub fn bind(client: &Client, receipt: Response<models::Submitted>) -> Result<Self, Error> {
        let value = &receipt.data;
        if value.thread.workspace_id.is_empty()
            || value.thread.id.is_empty()
            || value.entry.id.is_empty()
            || value.entry.thread_id != value.thread.id
            || value
                .run
                .as_ref()
                .is_some_and(|run| run.id.is_empty() || run.thread_id != value.thread.id)
        {
            return Err(invalid(&receipt));
        }
        let thread = client.thread(&value.thread.id);
        let entry = client.entry(&value.thread.id, &value.entry.id);
        let run = value.run.as_ref().map(|view| Run {
            id: view.id.clone(),
            thread_id: Some(view.thread_id.clone()),
            client: client.clone(),
        });
        Ok(Self {
            receipt,
            thread,
            entry,
            run,
        })
    }
    pub fn observe(
        self,
        key: Option<String>,
        options: ObservationOptions,
    ) -> Result<Interaction, Error> {
        Interaction::new(
            self.thread.client.clone(),
            InteractionIdentity {
                thread_id: Some(self.thread.id),
                entry_id: Some(self.entry.id),
                run_id: None,
                idempotency_key: key,
            },
            ReceiptEvidence::EntrySubmission(self.receipt),
            None,
            options,
        )
    }
}

/// One finite observer: result-only, raw frames and text projection share ONE reader.
/// Dropping or closing releases local I/O, never interrupts remote execution.
pub struct Interaction {
    pub identity: InteractionIdentity,
    pub evidence: ReceiptEvidence,
    pub thread: Option<Thread>,
    pub run: Option<Run>,
    client: Client,
    stream: Option<ThreadStream>,
    outcome: Option<RunOutcome>,
    deadline: Instant,
    interval: Duration,
    poll_at: Instant,
    closed: bool,
    preview: crate::preview::Preview,
}
impl Interaction {
    pub(crate) fn new(
        client: Client,
        identity: InteractionIdentity,
        evidence: ReceiptEvidence,
        run: Option<Run>,
        options: ObservationOptions,
    ) -> Result<Self, Error> {
        let options = options.freeze()?;
        if identity.thread_id.as_deref().is_some_and(str::is_empty)
            || identity.entry_id.as_deref().is_some_and(str::is_empty)
            || identity.run_id.as_deref().is_some_and(str::is_empty)
        {
            return Err(Error::InvalidInput);
        }
        let thread = identity.thread_id.as_ref().map(|id| client.thread(id));
        Ok(Self {
            client,
            identity,
            evidence,
            thread,
            run,
            stream: None,
            outcome: None,
            deadline: options.deadline.expect("frozen deadline"),
            interval: options.poll_interval,
            poll_at: Instant::now() + options.poll_interval,
            closed: false,
            preview: Default::default(),
        })
    }
    fn remaining(&self) -> Result<Duration, Error> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(Error::Timeout)
    }
    async fn bind(&mut self) -> Result<Run, Error> {
        if let Some(run) = &self.run {
            return Ok(run.clone());
        }
        let thread_id = self.identity.thread_id.clone().ok_or(Error::InvalidInput)?;
        let entry_id = self.identity.entry_id.clone().ok_or(Error::InvalidInput)?;
        let client = self.client.clone();
        let interval = self.interval;
        let run = tokio::time::timeout(
            self.remaining()?,
            client.observe(async {
                loop {
                    let snapshot = client.entry(&thread_id, &entry_id).get().await?;
                    if snapshot.data.id != entry_id || snapshot.data.thread_id != thread_id {
                        return Err(invalid(&snapshot));
                    }
                    match snapshot.data.status {
                        models::EntryStatus::Consumed => {
                            let id = snapshot
                                .data
                                .assigned_run_id
                                .as_ref()
                                .filter(|id| !id.is_empty())
                                .ok_or_else(|| invalid(&snapshot))?;
                            return Ok(Run {
                                id: id.clone(),
                                thread_id: Some(thread_id),
                                client: client.clone(),
                            });
                        }
                        models::EntryStatus::Failed | models::EntryStatus::Withdrawn => {
                            return Err(Error::Submission(Box::new(SubmissionError {
                                thread_id,
                                entry_id,
                                entry: snapshot,
                            })));
                        }
                        models::EntryStatus::Pending | models::EntryStatus::Assigned => {
                            tokio::time::sleep(interval).await
                        }
                    }
                }
            }),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        self.identity.run_id = Some(run.id.clone());
        self.run = Some(run.clone());
        Ok(run)
    }
    async fn poll_run(&mut self, run: &Run) -> Result<bool, Error> {
        let snapshot = tokio::time::timeout(self.remaining()?, run.get())
            .await
            .map_err(|_| Error::Timeout)??;
        run.check(&snapshot)?;
        if self
            .identity
            .thread_id
            .as_ref()
            .is_some_and(|id| id != &snapshot.data.thread_id)
        {
            return Err(invalid(&snapshot));
        }
        if self.thread.is_none() {
            self.identity.thread_id = Some(snapshot.data.thread_id.clone());
            self.thread = Some(self.client.thread(&snapshot.data.thread_id));
            if let Some(bound) = &mut self.run {
                bound.thread_id = Some(snapshot.data.thread_id.clone());
            }
        }
        self.poll_at = Instant::now() + self.interval;
        if sealed(&snapshot.data.status) {
            self.outcome = Some(RunOutcome::new(run.clone(), snapshot));
            self.close();
            return Ok(true);
        }
        Ok(false)
    }
    async fn next_frame(&mut self) -> Result<Option<ThreadFrame>, Error> {
        if self.closed || self.outcome.is_some() {
            return Ok(None);
        }
        let run = self.bind().await?;
        if self.thread.is_none() && self.poll_run(&run).await? {
            return Ok(None);
        }
        if self.stream.is_none() {
            let thread = self.thread.clone().ok_or(Error::InvalidInput)?;
            let mut connect = Box::pin(ThreadStream::open(
                thread.resource(),
                StreamOptions::default(),
            ));
            loop {
                if Instant::now() >= self.poll_at && self.poll_run(&run).await? {
                    return Ok(None);
                }
                let ready = tokio::time::timeout(self.remaining()?, async {
                    tokio::select! { connected = &mut connect => Some(connected), _ = tokio::time::sleep_until(self.poll_at) => None }
                }).await.map_err(|_| Error::Timeout)?;
                if let Some(connected) = ready {
                    self.stream = Some(connected?);
                    break;
                }
            }
        }
        loop {
            let remaining = self.remaining()?;
            if Instant::now() >= self.poll_at && self.poll_run(&run).await? {
                return Ok(None);
            }
            let read = async {
                tokio::select! {
                    frame = self.stream.as_mut().expect("connected stream").next() => Ok(Some(frame?)),
                    _ = tokio::time::sleep_until(self.poll_at) => Ok(None),
                }
            };
            let frame = tokio::time::timeout(remaining, read)
                .await
                .map_err(|_| Error::Timeout)??;
            match frame {
                Some(Some(frame)) => {
                    let own = match &frame {
                        ThreadFrame::Delta { data, .. } => data.run_id == run.id,
                        ThreadFrame::Boundary { data, .. } => data.run_id == run.id,
                        ThreadFrame::Gap(data) => data.run_id == run.id,
                        ThreadFrame::Reset(data) => data.run_id == run.id,
                        ThreadFrame::Changed(_) => false,
                    };
                    if own {
                        return Ok(Some(frame));
                    }
                }
                Some(None) => {
                    self.stream = None;
                    self.result().await?;
                    return Ok(None);
                }
                None => {}
            }
        }
    }
    /// Raw protocol escape hatch. Sequential use after preview interruption is supported.
    pub async fn next(&mut self) -> Result<Option<ThreadFrame>, Error> {
        let frame = self.next_frame().await?;
        if let Some(frame) = &frame {
            let _ = self.preview.project(frame);
        }
        Ok(frame)
    }
    /// Lazy root-assistant string chunks, never final output or a lossless transcript.
    /// Interruption is sticky but does not close result observation or raw iteration.
    pub async fn next_text(&mut self) -> Result<Option<String>, Error> {
        self.preview.check()?;
        loop {
            let frame = match self.next_frame().await {
                Err(Error::Protocol(_)) => {
                    return Err(self
                        .preview
                        .interrupt(crate::PreviewInterruptedKind::InvalidProtocol));
                }
                other => other?,
            };
            let Some(frame) = frame else {
                return Ok(None);
            };
            if let Some(text) = self.preview.project(&frame)? {
                return Ok(Some(text));
            }
        }
    }
    /// Authoritative result. Does not connect SSE when only a result is requested.
    pub async fn result(&mut self) -> Result<RunOutcome, Error> {
        if let Some(outcome) = &self.outcome {
            return Ok(outcome.clone());
        }
        if self.closed {
            return Err(Error::Closed);
        }
        self.stream = None;
        let run = self.bind().await?;
        loop {
            if self.poll_run(&run).await? {
                return Ok(self.outcome.as_ref().expect("sealed outcome").clone());
            }
            let client = self.client.clone();
            tokio::time::timeout(
                self.remaining()?,
                client.observe(async {
                    tokio::time::sleep_until(self.poll_at).await;
                    Ok(())
                }),
            )
            .await
            .map_err(|_| Error::Timeout)??;
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.stream = None;
    }
}
fn invalid<T>(response: &Response<T>) -> Error {
    ProtocolError::response(
        ProtocolKind::InvalidReceipt,
        response.status.as_u16(),
        &response.headers,
    )
}
fn sealed(status: &models::RunStatus) -> bool {
    matches!(
        status,
        models::RunStatus::Completed
            | models::RunStatus::Waiting
            | models::RunStatus::Failed
            | models::RunStatus::Cancelled
    )
}
/// Advanced convenience returning the canonical wire payload.
pub fn text_payload(text: impl Into<String>) -> models::MessagePayload {
    models::MessagePayload::new(vec![models::Part::Text(Box::new(models::TextPart::new(
        text.into(),
        models::text_part::Type::Text,
    )))])
}
