//! Curated workflow values backed by the canonical wire types.
use crate::{Error, generated::models};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Ordered multipart input. Constructors own the wire discriminators.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Input(models::MessagePayload);
impl Input {
    pub fn parts(parts: impl IntoIterator<Item = InputPart>) -> Self {
        Self(models::MessagePayload::new(
            parts.into_iter().map(|p| p.0).collect(),
        ))
    }
    pub fn from_wire(payload: models::MessagePayload) -> Self {
        Self(payload)
    }
    pub fn into_wire(self) -> models::MessagePayload {
        self.0
    }
}
impl From<models::MessagePayload> for Input {
    fn from(v: models::MessagePayload) -> Self {
        Self(v)
    }
}
impl From<&str> for Input {
    fn from(v: &str) -> Self {
        Self(crate::text_payload(v))
    }
}
impl From<String> for Input {
    fn from(v: String) -> Self {
        Self(crate::text_payload(v))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InputPart(models::Part);
impl InputPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self(models::Part::Text(Box::new(models::TextPart::new(
            text.into(),
            models::text_part::Type::Text,
        ))))
    }
    pub fn json(value: Value) -> Self {
        Self(models::Part::Json(Box::new(models::JsonPart::new(
            models::json_part::Type::Json,
            Some(value),
        ))))
    }
    pub fn url(url: impl Into<String>) -> Self {
        Self(models::Part::Url(Box::new(models::UrlPart::new(
            models::url_part::Type::Url,
            url.into(),
        ))))
    }
    pub fn asset(id: impl Into<String>) -> Self {
        Self(models::Part::Asset(Box::new(models::AssetPart::new(
            id.into(),
            models::asset_part::Type::Asset,
        ))))
    }
    pub fn from_wire(part: models::Part) -> Self {
        Self(part)
    }
    pub fn into_wire(self) -> models::Part {
        self.0
    }
}

macro_rules! wire_value {
    ($name:ident, $wire:ident) => {
        #[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(models::$wire);
        impl $name {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn from_wire(value: models::$wire) -> Self {
                Self(value)
            }
            pub fn into_wire(self) -> models::$wire {
                self.0
            }
        }
    };
}
wire_value!(RunOptions, RunOptionsInput);
wire_value!(RunConfiguration, RunConfigurationInput);
wire_value!(Overrides, AgentOverrideInput);
impl RunOptions {
    /// `None` explicitly selects null; not calling the builder omits the field.
    pub fn configuration(mut self, value: Option<RunConfiguration>) -> Self {
        self.0.configuration = Some(value.map(|v| Box::new(v.0)));
        self
    }
    /// Omission inherits; `None` transmits an explicit null.
    pub fn execution(mut self, value: Option<models::ExecutionOverride>) -> Self {
        self.0.execution = Some(value.map(Box::new));
        self
    }
    pub fn overrides(mut self, value: Option<Overrides>) -> Self {
        self.0.overrides = Some(value.map(|v| Box::new(v.0)));
        self
    }
    pub fn labels(mut self, value: HashMap<String, String>) -> Self {
        self.0.labels = Some(value);
        self
    }
    pub fn max_usage(mut self, value: Option<models::UsageLimit>) -> Self {
        self.0.max_usage = Some(value.map(Box::new));
        self
    }
}
impl RunConfiguration {
    pub fn allowed_hosts(mut self, value: Option<Vec<String>>) -> Self {
        self.0.allowed_hosts = Some(value);
        self
    }
    pub fn extensions(mut self, value: HashMap<String, Value>) -> Self {
        self.0.extensions = Some(value);
        self
    }
}
macro_rules! override_field {
    ($name:ident, $ty:ty) => {
        pub fn $name(mut self, value: Option<$ty>) -> Self {
            self.0.$name = Some(value);
            self
        }
    };
}
macro_rules! boxed_override_field {
    ($name:ident, $ty:ty) => {
        pub fn $name(mut self, value: Option<$ty>) -> Self {
            self.0.$name = Some(value.map(Box::new));
            self
        }
    };
}
impl Overrides {
    override_field!(
        builtin_skills,
        std::collections::HashSet<models::agent_override_input::BuiltinSkills>
    );
    override_field!(client_tools, Vec<models::ClientToolDefinition>);
    override_field!(connection_tools, Vec<models::ConnectionSelection>);
    override_field!(instructions, String);
    override_field!(lazy_environment, bool);
    boxed_override_field!(media_understanding, models::MediaUnderstandingSelection);
    override_field!(model, String);
    boxed_override_field!(model_characteristics, models::AgentModelCharacteristics);
    override_field!(model_settings, HashMap<String, Value>);
    boxed_override_field!(output_spec, models::OutputSpec);
    override_field!(plugins, Vec<models::PluginSelection>);
    boxed_override_field!(retries, models::RetryOverride);
    boxed_override_field!(reviewer, models::AgentReviewer);
    boxed_override_field!(skill_sync, models::AgentOverrideInputSkillSync);
    override_field!(skills, Vec<models::SkillSelection>);
    override_field!(subagents, Value);
    /// Replaces the whole tool-search block; `None` retains the Agent revision.
    pub fn tool_search(mut self, value: Option<models::ToolSearch>) -> Self {
        self.0.tool_search = Some(value.map(Box::new));
        self
    }
    override_field!(toolsets, HashMap<String, models::ToolsetSelection>);
}

/// One answer map splits to the two mandatory wire maps in one atomic request.
/// The Service, not the SDK, validates completeness against its checkpoint.
#[derive(Clone, Debug)]
pub enum Answer {
    Approve,
    Deny { reason: Option<Option<String>> },
    Returned(Value),
    Failed(String),
}
impl Answer {
    pub fn approve() -> Self {
        Self::Approve
    }
    pub fn deny() -> Self {
        Self::Deny { reason: None }
    }
    /// `None` sends an explicit null reason; an empty string stays empty.
    pub fn deny_with(reason: Option<String>) -> Self {
        Self::Deny {
            reason: Some(reason),
        }
    }
    /// An explicit JSON value is required; null is a valid returned value.
    pub fn returned(value: Value) -> Self {
        Self::Returned(value)
    }
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}
pub(crate) fn answers_wire(answers: HashMap<String, Answer>) -> models::Resume {
    let mut request = models::Resume::new(HashMap::new(), HashMap::new());
    for (id, answer) in answers {
        match answer {
            Answer::Approve => {
                request
                    .approvals
                    .insert(id, models::ApprovalDecision::Approve(Box::default()));
            }
            Answer::Deny { reason } => {
                request.approvals.insert(
                    id,
                    models::ApprovalDecision::Deny(Box::new(models::Deny {
                        reason,
                        ..Default::default()
                    })),
                );
            }
            Answer::Returned(value) => {
                request.calls.insert(
                    id,
                    models::CallResult::Returned(Box::new(models::Returned::new(
                        models::returned::Status::Returned,
                        Some(value),
                    ))),
                );
            }
            Answer::Failed(message) => {
                request.calls.insert(
                    id,
                    models::CallResult::Failed(Box::new(models::Failed::new(
                        message,
                        models::failed::Status::Failed,
                    ))),
                );
            }
        }
    }
    request
}
pub(crate) fn request_key(key: Option<String>) -> Result<String, Error> {
    match key {
        Some(key) if !key.is_empty() => Ok(key),
        Some(_) => Err(Error::InvalidInput),
        None => {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| Error::InvalidInput)?;
            Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
        }
    }
}
