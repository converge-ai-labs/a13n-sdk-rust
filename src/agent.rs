//! Agent-first workflows reuse the complete generated resource API and one transport.
use crate::{
    Answer, Client, Error, Input, Interaction, InteractionIdentity, ObservationOptions,
    ReceiptEvidence, Run, RunOptions, Submitted,
    generated::models,
    resources::{InboxCreateOptions, RunForkOptions, RunResumeOptions, ThreadsCreateOptions},
    workflow::{answers_wire, request_key},
};
use std::collections::HashMap;

/// Optional request selectors and one grouped RunOptions value.
/// Nullable fields use `None` for omission and `Some(None)` for explicit null.
#[derive(Default)]
pub struct StartOptions {
    pub idempotency_key: Option<String>,
    pub observation: ObservationOptions,
    pub agent_revision_id: Option<Option<String>>,
    pub delivery: Option<models::Delivery>,
    pub environments: Option<Vec<models::MountCreate>>,
    pub mcp_headers: Option<serde_json::Value>,
    pub memories: Option<Vec<models::MemoryMount>>,
    pub message_history: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    pub run_options: Option<RunOptions>,
    pub session_id: Option<Option<String>>,
}
#[derive(Default)]
pub struct SendOptions {
    pub idempotency_key: Option<String>,
    pub observation: ObservationOptions,
    pub agent_revision_id: Option<Option<String>>,
    pub delivery: Option<models::Delivery>,
    pub run_options: Option<RunOptions>,
}
#[derive(Default)]
pub struct ResumeOptions {
    pub idempotency_key: Option<String>,
    pub observation: ObservationOptions,
    pub input: Option<Option<Input>>,
}
#[derive(Default)]
pub struct ForkOptions {
    pub idempotency_key: Option<String>,
    pub observation: ObservationOptions,
    pub agent_revision_id: Option<Option<String>>,
    pub delivery: Option<models::Delivery>,
    pub environments: Option<Vec<models::MountCreate>>,
    pub fresh_environments: Option<bool>,
    pub memories: Option<Vec<models::MemoryMount>>,
    pub run_options: Option<RunOptions>,
}
/// Owned Agent ID selection, never a permanent Thread owner assignment.
#[derive(Clone)]
pub struct Agent {
    client: Client,
    id: String,
}
impl Client {
    pub fn agent(&self, id: impl Into<String>) -> Agent {
        Agent {
            client: self.clone(),
            id: id.into(),
        }
    }
}
impl Agent {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub async fn start(&self, input: impl Into<Input>) -> Result<Interaction, Error> {
        self.start_with(input, StartOptions::default()).await
    }
    /// One key is chosen before dispatch; no mutation is automatically retried.
    /// Persist your own key/body before sending when cancellation-safe recovery matters.
    pub async fn start_with(
        &self,
        input: impl Into<Input>,
        options: StartOptions,
    ) -> Result<Interaction, Error> {
        if self.id.is_empty() {
            return Err(Error::InvalidInput);
        }
        let key = request_key(options.idempotency_key)?;
        let request_deadline = options.observation.deadline;
        let observation = options.observation.freeze()?;
        let mut body = models::NewThread::new(self.id.clone(), input.into().into_wire());
        body.agent_revision_id = options.agent_revision_id;
        body.delivery = options.delivery;
        body.environments = options.environments;
        body.mcp_headers = options.mcp_headers;
        body.memories = options.memories;
        body.message_history = options.message_history;
        body.options = options.run_options.map(|v| Box::new(v.into_wire()));
        body.session_id = options.session_id;
        self.client
            .mutation(
                "start",
                key.clone(),
                None,
                None,
                request_deadline,
                async |transport| {
                    let receipt = transport
                        .api()
                        .threads()
                        .create(
                            &body,
                            ThreadsCreateOptions {
                                idempotency_key: key.clone(),
                                ..Default::default()
                            },
                        )
                        .await?;
                    Submitted::bind(&self.client, receipt)?.observe(Some(key), observation)
                },
            )
            .await
    }
    pub async fn send(
        &self,
        thread: impl Into<String>,
        input: impl Into<Input>,
    ) -> Result<Interaction, Error> {
        self.send_with(thread, input, SendOptions::default()).await
    }
    pub async fn send_with(
        &self,
        thread: impl Into<String>,
        input: impl Into<Input>,
        options: SendOptions,
    ) -> Result<Interaction, Error> {
        let thread = thread.into();
        if self.id.is_empty() || thread.is_empty() {
            return Err(Error::InvalidInput);
        }
        let key = request_key(options.idempotency_key)?;
        let request_deadline = options.observation.deadline;
        let observation = options.observation.freeze()?;
        let mut body = models::Message::new(self.id.clone(), input.into().into_wire());
        body.agent_revision_id = options.agent_revision_id;
        body.delivery = options.delivery;
        body.options = options.run_options.map(|v| Box::new(v.into_wire()));
        self.client
            .mutation(
                "send",
                key.clone(),
                Some(thread.clone()),
                None,
                request_deadline,
                async |transport| {
                    let receipt = transport
                        .api()
                        .threads()
                        .at(&thread)
                        .inbox()
                        .create(
                            &body,
                            InboxCreateOptions {
                                idempotency_key: key.clone(),
                                ..Default::default()
                            },
                        )
                        .await?;
                    let submitted = Submitted::bind(&self.client, receipt)?;
                    if submitted.thread.id != thread {
                        return Err(invalid(&submitted.receipt));
                    }
                    submitted.observe(Some(key), observation)
                },
            )
            .await
    }
}
impl Run {
    pub async fn resume(&self, answers: HashMap<String, Answer>) -> Result<Interaction, Error> {
        self.resume_with(answers, ResumeOptions::default()).await
    }
    /// One complete atomic answer batch returns the distinct authoritative successor.
    /// No Entry is fabricated and no client tool executes automatically.
    pub async fn resume_with(
        &self,
        answers: HashMap<String, Answer>,
        options: ResumeOptions,
    ) -> Result<Interaction, Error> {
        if self.id.is_empty() {
            return Err(Error::InvalidInput);
        }
        let key = request_key(options.idempotency_key)?;
        let request_deadline = options.observation.deadline;
        let observation = options.observation.freeze()?;
        let mut body = answers_wire(answers);
        body.input = options.input.map(|v| v.map(|v| Box::new(v.into_wire())));
        self.client
            .mutation(
                "resume",
                key.clone(),
                self.thread_id.clone(),
                Some(self.id.clone()),
                request_deadline,
                async |transport| {
                    let receipt = transport
                        .api()
                        .runs()
                        .at(&self.id)
                        .resume(
                            &body,
                            RunResumeOptions {
                                idempotency_key: key.clone(),
                                ..Default::default()
                            },
                        )
                        .await?;
                    if receipt.data.id.is_empty()
                        || receipt.data.id == self.id
                        || receipt.data.thread_id.is_empty()
                        || self
                            .thread_id
                            .as_ref()
                            .is_some_and(|id| id != &receipt.data.thread_id)
                    {
                        return Err(invalid(&receipt));
                    }
                    let run = Run {
                        id: receipt.data.id.clone(),
                        thread_id: Some(receipt.data.thread_id.clone()),
                        client: self.client.clone(),
                    };
                    Interaction::new(
                        self.client.clone(),
                        InteractionIdentity {
                            thread_id: run.thread_id.clone(),
                            entry_id: None,
                            run_id: Some(run.id.clone()),
                            idempotency_key: Some(key),
                        },
                        ReceiptEvidence::ResumeRunReceipt(Box::new(receipt)),
                        Some(run),
                        observation,
                    )
                },
            )
            .await
    }
    /// Fork uses the existing wire operation: a new Thread/Entry with normal incorporation.
    pub async fn fork(
        &self,
        agent_id: impl Into<String>,
        input: impl Into<Input>,
        options: ForkOptions,
    ) -> Result<Interaction, Error> {
        let agent = agent_id.into();
        if self.id.is_empty() || agent.is_empty() {
            return Err(Error::InvalidInput);
        }
        let key = request_key(options.idempotency_key)?;
        let request_deadline = options.observation.deadline;
        let observation = options.observation.freeze()?;
        let mut body = models::Fork::new(agent, input.into().into_wire());
        body.agent_revision_id = options.agent_revision_id;
        body.delivery = options.delivery;
        body.environments = options.environments;
        body.fresh_environments = options.fresh_environments;
        body.memories = options.memories;
        body.options = options.run_options.map(|v| Box::new(v.into_wire()));
        self.client
            .mutation(
                "fork",
                key.clone(),
                self.thread_id.clone(),
                Some(self.id.clone()),
                request_deadline,
                async |transport| {
                    let receipt = transport
                        .api()
                        .runs()
                        .at(&self.id)
                        .fork(
                            &body,
                            RunForkOptions {
                                idempotency_key: key.clone(),
                                ..Default::default()
                            },
                        )
                        .await?;
                    Submitted::bind(&self.client, receipt)?.observe(Some(key), observation)
                },
            )
            .await
    }
}
fn invalid<T>(response: &crate::Response<T>) -> Error {
    crate::ProtocolError::response(
        crate::ProtocolKind::InvalidReceipt,
        response.status.as_u16(),
        &response.headers,
    )
}
