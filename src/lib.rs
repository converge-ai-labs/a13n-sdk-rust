#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod agent;
mod client;
mod error;
mod preview;
mod workflow;
pub use error::{ProtocolError, ProtocolKind, TransportError, TransportKind, TransportStage};
/// Generated wire models and advanced HTTP access.
/// ```compile_fail
/// use a13n::generated::models::AgentUpdate;
/// let _ = AgentUpdate { name: Some(Some(42)), ..Default::default() };
/// ```
/// ```compile_fail
/// use a13n::generated::models::NewThread;
/// let _ = NewThread { payload: 42, ..Default::default() };
/// ```
pub mod generated;
mod interaction;
pub mod resources;
pub mod streaming;
pub use agent::{Agent, ForkOptions, ResumeOptions, SendOptions, StartOptions};
pub use preview::{PreviewInterruptedError, PreviewInterruptedKind};
pub use workflow::{Answer, Input, InputPart, Overrides, RunConfiguration, RunOptions};
// Selected canonical workflow primitives, not a duplicate schema hierarchy.
pub use client::{
    ApiError, BinaryResponse, CallError, Client, ClientBuilder, Error, Response, Secret,
    SubmissionError, UnknownOutcomeError, UploadFile,
};
pub use generated::models::{
    AgentModelCharacteristics, AgentReviewer, ClientToolDefinition, ConnectionSelection, Delivery,
    EntryStatus, EnvironmentAction, EnvironmentPermissionSetInput, Failure, FrozenMemoryMount,
    IsolatedSkillSync, MediaUnderstandingSelection, MemoryAccess, MemoryMount, MountCreate,
    NativeToolSearchConfig, OutputSpec, OutputVariant, PendingCall, PlatformGroup,
    PlatformSelection, PluginSelection, ProxyToolSearchConfig, RetryOverride, RunStatus,
    SessionMemorySettings, SharedSkillSync, SkillSelection, SkillSync, SpaceBinding,
    ToolPermissionSetting, ToolReviewRule, ToolRiskLevel, ToolSearch, ToolSearchConfig,
    ToolSearchGroup, ToolsetSelection, UsageLimit,
};
pub use generated::models::{
    AgentOverrideInputSkillSync as OverrideSkillSync,
    agent_override_input::BuiltinSkills as BuiltinSkill,
    agent_reviewer::{OnError as ReviewerOnError, OnFlagged as ReviewerOnFlagged},
    client_tool_definition::Permission as ClientToolPermission,
    isolated_skill_sync::Mode as IsolatedSkillSyncMode,
    platform_selection::{
        ReadPermission as PlatformReadPermission, WritePermission as PlatformWritePermission,
    },
    shared_skill_sync::Mode as SharedSkillSyncMode,
    tool_review_rule::OnFlagged as RuleOnFlagged,
};
pub use interaction::{
    Entry, Interaction, InteractionIdentity, ObservationOptions, ReceiptEvidence, Run, RunOutcome,
    Submitted, Thread, text_payload,
};
