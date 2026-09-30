use crate::{
    Client, Error, ProtocolError, ProtocolKind, Response, SubmissionError,
    generated::models,
    resources::{
        EntryGetOptions, EntryResource, RunGetOptions, RunInterruptOptions, RunResource,
        RunResumeOptions, ThreadResource,
    },
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use std::time::Duration;
use tokio::time::Instant;

const DEFAULT_WAIT: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(500);

/// Small authored handles delegate all I/O to generated schema operations.
#[derive(Clone)]
pub struct Thread<'a> {
    pub id: String,
    resource: ThreadResource<'a>,
}
impl<'a> Thread<'a> {
    pub fn resource(&self) -> &ThreadResource<'a> {
        &self.resource
    }
}
#[derive(Clone)]
pub struct Entry<'a> {
    pub id: String,
    pub thread_id: String,
    resource: EntryResource<'a>,
}
impl<'a> Entry<'a> {
    pub fn resource(&self) -> &EntryResource<'a> {
        &self.resource
    }
    /// Observe incorporation or a failed/withdrawn disposition, not provisional assignment.
    pub async fn wait(&self, interval: Duration) -> Result<Response<models::EntryView>, Error> {
        let interval = if interval.is_zero() { POLL } else { interval };
        self.resource
            .0
            .client
            .observe(async {
                loop {
                    let result = self.resource.get(EntryGetOptions::default()).await?;
                    if result.data.id != self.id || result.data.thread_id != self.thread_id {
                        return Err(ProtocolError::response(
                            ProtocolKind::InvalidReceipt,
                            result.status.as_u16(),
                            &result.headers,
                        ));
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
pub struct Run<'a> {
    pub id: String,
    resource: RunResource<'a>,
}
impl<'a> Run<'a> {
    pub fn resource(&self) -> &RunResource<'a> {
        &self.resource
    }
    pub fn items(&self) -> crate::resources::RunItemsResource<'a> {
        self.resource.items()
    }
    /// Explicitly interrupt remote execution. Closing an Interaction never calls this.
    pub async fn interrupt(&self) -> Result<Response<models::RunView>, Error> {
        let snapshot = self
            .resource
            .interrupt(RunInterruptOptions::default())
            .await?;
        if snapshot.data.id != self.id {
            return Err(ProtocolError::response(
                ProtocolKind::InvalidReceipt,
                snapshot.status.as_u16(),
                &snapshot.headers,
            ));
        }
        Ok(snapshot)
    }
    /// Submit the complete typed result batch and optional ordinary message input together.
    /// The explicit key identifies this one immutable intent; the receipt binds a distinct successor.
    pub async fn resume(
        &self,
        request: &models::Resume,
        idempotency_key: impl Into<String>,
    ) -> Result<Resumed<'a>, Error> {
        let receipt = self
            .resource
            .resume(
                request,
                RunResumeOptions {
                    idempotency_key: idempotency_key.into(),
                    ..Default::default()
                },
            )
            .await?;
        if receipt.data.id.is_empty()
            || receipt.data.id == self.id
            || receipt.data.thread_id.is_empty()
        {
            return Err(ProtocolError::response(
                ProtocolKind::InvalidReceipt,
                receipt.status.as_u16(),
                &receipt.headers,
            ));
        }
        let run = self.resource.0.client.run(receipt.data.id.clone());
        Ok(Resumed { run, receipt })
    }
    pub async fn wait(&self) -> Result<RunOutcome<'a>, Error> {
        self.wait_with(DEFAULT_WAIT, POLL).await
    }
    pub async fn wait_with(
        &self,
        timeout: Duration,
        interval: Duration,
    ) -> Result<RunOutcome<'a>, Error> {
        if timeout.is_zero() || interval.is_zero() {
            return Err(Error::InvalidInput);
        }
        self.resource
            .0
            .client
            .observe(async {
                tokio::time::timeout(timeout, self.wait_inner(interval))
                    .await
                    .map_err(|_| Error::Timeout)?
            })
            .await
    }
    async fn wait_inner(&self, interval: Duration) -> Result<RunOutcome<'a>, Error> {
        loop {
            let snapshot = self.resource.get(RunGetOptions::default()).await?;
            if snapshot.data.id != self.id {
                return Err(ProtocolError::response(
                    ProtocolKind::InvalidReceipt,
                    snapshot.status.as_u16(),
                    &snapshot.headers,
                ));
            }
            if sealed(&snapshot.data.status) {
                return Ok(RunOutcome {
                    run: self.clone(),
                    snapshot,
                });
            }
            tokio::time::sleep(interval).await;
        }
    }
}
impl Client {
    pub fn run(&self, id: impl Into<String>) -> Run<'_> {
        let id = id.into();
        Run {
            resource: self.resources().runs().at(id.clone()),
            id,
        }
    }
    pub fn entry(&self, thread_id: impl Into<String>, id: impl Into<String>) -> Entry<'_> {
        let thread_id = thread_id.into();
        let id = id.into();
        Entry {
            resource: self
                .resources()
                .threads()
                .at(thread_id.clone())
                .inbox()
                .at(id.clone()),
            id,
            thread_id,
        }
    }
    pub fn thread(&self, id: impl Into<String>) -> Thread<'_> {
        let id = id.into();
        Thread {
            resource: self.resources().threads().at(id.clone()),
            id,
        }
    }
}

/// Successor of an explicit resume, retaining the original HTTP response and status.
pub struct Resumed<'a> {
    pub run: Run<'a>,
    pub receipt: Response<models::RunView>,
}

/// A sealed or waiting exact Run, with original HTTP metadata and its bound resource.
#[derive(Clone)]
pub struct RunOutcome<'a> {
    pub run: Run<'a>,
    pub snapshot: Response<models::RunView>,
}
impl RunOutcome<'_> {
    pub fn status(&self) -> &models::RunStatus {
        &self.snapshot.data.status
    }
    pub fn output(&self) -> Option<&serde_json::Value> {
        self.snapshot.data.output.as_deref()
    }
    pub fn pending(&self) -> Option<&models::Pending> {
        self.snapshot.data.pending.as_deref()
    }
    pub fn failure(&self) -> Option<&models::Failure> {
        self.snapshot.data.failure.as_deref()
    }
}

/// Acceptance receipt with canonical references. A queued entry has no Run.
pub struct Submitted<'a> {
    pub receipt: Response<models::Submitted>,
    pub thread: Thread<'a>,
    pub entry: Entry<'a>,
    pub run: Option<Run<'a>>,
}
impl<'a> Submitted<'a> {
    /// Bind a raw schema-level submission receipt to authored local handles.
    pub fn bind(client: &'a Client, receipt: Response<models::Submitted>) -> Result<Self, Error> {
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
            return Err(ProtocolError::response(
                ProtocolKind::InvalidReceipt,
                receipt.status.as_u16(),
                &receipt.headers,
            ));
        }
        let thread = client.thread(value.thread.id.clone());
        let entry = client.entry(value.thread.id.clone(), value.entry.id.clone());
        let run = value.run.as_ref().map(|run| client.run(run.id.clone()));
        Ok(Self {
            receipt,
            thread,
            entry,
            run,
        })
    }

    /// Wait until this Entry is incorporated (not merely assigned), then observe
    /// its assigned exact Run. One deadline covers both phases and in-flight reads.
    pub async fn wait(&self) -> Result<RunOutcome<'a>, Error> {
        self.wait_with(DEFAULT_WAIT, POLL).await
    }
    /// Customize a single total deadline and polling interval; neither restarts at
    /// the Entry-to-Run transition. Dropping this future stops only local observation.
    pub async fn wait_with(
        &self,
        timeout: Duration,
        interval: Duration,
    ) -> Result<RunOutcome<'a>, Error> {
        if timeout.is_zero() || interval.is_zero() {
            return Err(Error::InvalidInput);
        }
        self.entry
            .resource
            .0
            .client
            .observe(async {
                tokio::time::timeout(timeout, async {
                    let run = self.incorporating_run(interval).await?;
                    let outcome = run.wait_inner(interval).await?;
                    self.check_run(&outcome.snapshot)?;
                    if outcome.snapshot.data.id != run.id {
                        return Err(ProtocolError::response(
                            ProtocolKind::InvalidReceipt,
                            outcome.snapshot.status.as_u16(),
                            &outcome.snapshot.headers,
                        ));
                    }
                    Ok(outcome)
                })
                .await
                .map_err(|_| Error::Timeout)?
            })
            .await
    }
    fn check_run(&self, snapshot: &Response<models::RunView>) -> Result<(), Error> {
        if snapshot.data.thread_id != self.receipt.data.thread.id {
            return Err(ProtocolError::response(
                ProtocolKind::InvalidReceipt,
                snapshot.status.as_u16(),
                &snapshot.headers,
            ));
        }
        Ok(())
    }
    async fn incorporating_run(&self, interval: Duration) -> Result<Run<'a>, Error> {
        loop {
            let snapshot = self.entry.resource.get(EntryGetOptions::default()).await?;
            if snapshot.data.id != self.receipt.data.entry.id
                || snapshot.data.thread_id != self.receipt.data.thread.id
            {
                return Err(ProtocolError::response(
                    ProtocolKind::InvalidReceipt,
                    snapshot.status.as_u16(),
                    &snapshot.headers,
                ));
            }
            match snapshot.data.status {
                models::EntryStatus::Consumed => {
                    let run_id = snapshot
                        .data
                        .assigned_run_id
                        .as_ref()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| {
                            ProtocolError::response(
                                ProtocolKind::InvalidReceipt,
                                snapshot.status.as_u16(),
                                &snapshot.headers,
                            )
                        })?;
                    return Ok(self.entry.resource.0.client.run(run_id.clone()));
                }
                models::EntryStatus::Failed | models::EntryStatus::Withdrawn => {
                    return Err(Error::Submission(Box::new(SubmissionError {
                        thread_id: self.receipt.data.thread.id.clone(),
                        entry_id: self.receipt.data.entry.id.clone(),
                        entry: snapshot,
                    })));
                }
                models::EntryStatus::Pending | models::EntryStatus::Assigned => {
                    tokio::time::sleep(interval).await
                }
            }
        }
    }
}

/// One finite invocation. `next` yields only frames of the incorporating Run;
/// `result` returns its authoritative sealed snapshot, even without iteration.
/// Dropping the object closes the local SSE connection, never interrupts the Run.
pub struct Interaction<'a> {
    pub receipt: Response<models::Submitted>,
    pub thread: Thread<'a>,
    submitted: Submitted<'a>,
    run: Option<Run<'a>>,
    stream: Option<ThreadStream<'a>>,
    outcome: Option<RunOutcome<'a>>,
    deadline: Instant,
    poll_at: Instant,
    closed: bool,
}
impl<'a> Interaction<'a> {
    pub(crate) fn new(submitted: Submitted<'a>) -> Self {
        Self {
            receipt: submitted.receipt.clone(),
            thread: submitted.thread.clone(),
            submitted,
            run: None,
            stream: None,
            outcome: None,
            deadline: Instant::now() + DEFAULT_WAIT,
            poll_at: Instant::now() + POLL,
            closed: false,
        }
    }
    fn remaining(&self) -> Result<Duration, Error> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(Error::Timeout)
    }
    async fn bind(&mut self) -> Result<Run<'a>, Error> {
        if let Some(run) = &self.run {
            return Ok(run.clone());
        }
        let run = tokio::time::timeout(self.remaining()?, self.submitted.incorporating_run(POLL))
            .await
            .map_err(|_| Error::Timeout)??;
        self.run = Some(run.clone());
        Ok(run)
    }
    async fn poll_run(&mut self, run: &Run<'a>) -> Result<bool, Error> {
        let snapshot = tokio::time::timeout(
            self.remaining()?,
            run.resource.get(RunGetOptions::default()),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        self.submitted.check_run(&snapshot)?;
        if snapshot.data.id != run.id {
            return Err(ProtocolError::response(
                ProtocolKind::InvalidReceipt,
                snapshot.status.as_u16(),
                &snapshot.headers,
            ));
        }
        self.poll_at = Instant::now() + POLL;
        if sealed(&snapshot.data.status) {
            self.outcome = Some(RunOutcome {
                run: run.clone(),
                snapshot,
            });
            self.close();
            return Ok(true);
        }
        Ok(false)
    }
    /// Return the next frame from this interaction's exact Run. Gap/reset are
    /// readback hints; absence of a frame is not evidence that output is durable.
    pub async fn next(&mut self) -> Result<Option<ThreadFrame>, Error> {
        if self.closed || self.outcome.is_some() {
            return Ok(None);
        }
        let run = self.bind().await?;
        if self.stream.is_none() {
            let thread = self.thread.clone();
            let mut connect = Box::pin(ThreadStream::open(
                thread.resource.clone(),
                StreamOptions::default(),
            ));
            loop {
                if Instant::now() >= self.poll_at && self.poll_run(&run).await? {
                    return Ok(None);
                }
                let ready = tokio::time::timeout(self.remaining()?, async {
                    tokio::select! {
                        connected = &mut connect => Some(connected),
                        _ = tokio::time::sleep_until(self.poll_at) => None,
                    }
                })
                .await
                .map_err(|_| Error::Timeout)?;
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
                    let _ = self.result().await?;
                    return Ok(None);
                }
                None => {}
            }
        }
    }
    /// Read the authoritative exact Run. Does not connect SSE if no frames were requested.
    pub async fn result(&mut self) -> Result<RunOutcome<'a>, Error> {
        if let Some(outcome) = &self.outcome {
            return Ok(outcome.clone());
        }
        if self.closed {
            return Err(Error::Closed);
        }
        self.stream = None;
        let run = self.bind().await?;
        let outcome = tokio::time::timeout(self.remaining()?, run.wait_inner(POLL))
            .await
            .map_err(|_| Error::Timeout)??;
        self.submitted.check_run(&outcome.snapshot)?;
        if outcome.snapshot.data.id != run.id {
            return Err(ProtocolError::response(
                ProtocolKind::InvalidReceipt,
                outcome.snapshot.status.as_u16(),
                &outcome.snapshot.headers,
            ));
        }
        self.outcome = Some(outcome.clone());
        self.close();
        Ok(outcome)
    }
    /// Stop local iteration. The remote Run continues until explicitly interrupted.
    pub fn close(&mut self) {
        self.closed = true;
        self.stream = None;
    }
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
/// Convenience over the full generated MessagePayload union.
pub fn text_payload(text: impl Into<String>) -> models::MessagePayload {
    models::MessagePayload::new(vec![models::Part::Text(Box::new(models::TextPart::new(
        text.into(),
        models::text_part::Type::Text,
    )))])
}
