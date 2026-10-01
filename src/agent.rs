//! Agent-first conversations built on the complete generated resource API.
use crate::{
    Client, Error, Interaction, Submitted,
    generated::models,
    resources::{InboxCreateOptions, ThreadsCreateOptions},
};

/// Message input. A string becomes a single text part; full structured payloads
/// (including ordered media URL/file references and JSON values) pass through unchanged.
/// Acquisition, media preparation and Run hostname policy remain Service/Harness-owned.
pub struct Input(pub models::MessagePayload);
impl From<models::MessagePayload> for Input {
    fn from(value: models::MessagePayload) -> Self {
        Self(value)
    }
}
impl From<&str> for Input {
    fn from(value: &str) -> Self {
        Self(crate::text_payload(value))
    }
}
impl From<String> for Input {
    fn from(value: String) -> Self {
        Self(crate::text_payload(value))
    }
}

/// Optional new-Thread fields. `None` omits a nullable field; `Some(None)` sends null.
#[derive(Default)]
pub struct StartOptions {
    pub agent_revision_id: Option<Option<String>>,
    pub delivery: Option<models::Delivery>,
    pub environments: Option<Vec<models::MountCreate>>,
    pub mcp_headers: Option<serde_json::Value>,
    pub memories: Option<Vec<models::MemoryMount>>,
    /// Completed native Pydantic AI ModelMessage JSON objects for an initial import.
    /// The Service validates this history; it is not repeated by `send` or `resume`.
    pub message_history: Option<Vec<std::collections::HashMap<String, serde_json::Value>>>,
    /// Complete Run options, including `configuration` independently of Agent overrides.
    /// Configuration omission/null selects Service defaults or retains steering's frozen value;
    /// an explicit object selects that entire snapshot without SDK merging or normalization.
    pub options: Option<Box<models::RunOptionsInput>>,
    pub session_id: Option<Option<String>>,
}
/// Optional continuation message fields.
#[derive(Default)]
pub struct SendOptions {
    pub agent_revision_id: Option<Option<String>>,
    pub delivery: Option<models::Delivery>,
    /// Complete Run options, including `configuration` independently of Agent overrides.
    /// Configuration omission/null selects Service defaults or retains steering's frozen value;
    /// an explicit object selects that entire snapshot without SDK merging or normalization.
    pub options: Option<Box<models::RunOptionsInput>>,
}

/// A local Agent ID binding, never a Service-side thread-owner assignment.
#[derive(Clone)]
pub struct Agent<'a> {
    client: &'a Client,
    id: String,
}
impl Client {
    /// Bind an Agent ID locally. The ordinary API-key path needs no workspace ID.
    pub fn agent(&self, id: impl Into<String>) -> Agent<'_> {
        Agent {
            client: self,
            id: id.into(),
        }
    }
}
impl<'a> Agent<'a> {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Create a Thread and submit its initial message. Acceptance is not completion.
    /// The caller supplies a stable request key for explicit readback after uncertainty.
    pub async fn start(
        &self,
        input: impl Into<Input>,
        idempotency_key: impl Into<String>,
    ) -> Result<Interaction<'a>, Error> {
        self.start_with(input, idempotency_key, StartOptions::default())
            .await
    }
    /// Start with all typed new-Thread fields; no Service scheduling is inferred locally.
    pub async fn start_with(
        &self,
        input: impl Into<Input>,
        idempotency_key: impl Into<String>,
        options: StartOptions,
    ) -> Result<Interaction<'a>, Error> {
        if self.id.is_empty() {
            return Err(Error::InvalidInput);
        }
        let mut body = models::NewThread::new(self.id.clone(), input.into().0);
        body.agent_revision_id = options.agent_revision_id;
        body.delivery = options.delivery;
        body.environments = options.environments;
        body.mcp_headers = options.mcp_headers;
        body.memories = options.memories;
        body.message_history = options.message_history;
        body.options = options.options;
        body.session_id = options.session_id;
        let submitted = self
            .client
            .resources()
            .threads()
            .create(
                &body,
                ThreadsCreateOptions {
                    idempotency_key: idempotency_key.into(),
                    ..Default::default()
                },
            )
            .await?;
        Ok(Interaction::new(Submitted::bind(self.client, submitted)?))
    }

    /// Continue an existing Thread with THIS Agent. Threads have no permanent Agent.
    pub async fn send(
        &self,
        thread_id: impl Into<String>,
        input: impl Into<Input>,
        idempotency_key: impl Into<String>,
    ) -> Result<Interaction<'a>, Error> {
        self.send_with(thread_id, input, idempotency_key, SendOptions::default())
            .await
    }
    /// Continue with the complete message selection and delivery options.
    pub async fn send_with(
        &self,
        thread_id: impl Into<String>,
        input: impl Into<Input>,
        idempotency_key: impl Into<String>,
        options: SendOptions,
    ) -> Result<Interaction<'a>, Error> {
        if self.id.is_empty() {
            return Err(Error::InvalidInput);
        }
        let thread_id = thread_id.into();
        if thread_id.is_empty() {
            return Err(Error::InvalidInput);
        }
        let mut body = models::Message::new(self.id.clone(), input.into().0);
        body.agent_revision_id = options.agent_revision_id;
        body.delivery = options.delivery;
        body.options = options.options;
        let submitted = self
            .client
            .resources()
            .threads()
            .at(thread_id.clone())
            .inbox()
            .create(
                &body,
                InboxCreateOptions {
                    idempotency_key: idempotency_key.into(),
                    ..Default::default()
                },
            )
            .await?;
        let submitted = Submitted::bind(self.client, submitted)?;
        if submitted.thread.id != thread_id {
            return Err(crate::ProtocolError::response(
                crate::ProtocolKind::InvalidReceipt,
                submitted.receipt.status.as_u16(),
                &submitted.receipt.headers,
            ));
        }
        Ok(Interaction::new(submitted))
    }
}
