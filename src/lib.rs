#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod agent;
mod client;
mod error;
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
pub use agent::{Agent, Input, SendOptions, StartOptions};
pub use client::{
    ApiError, BinaryResponse, CallError, Client, ClientBuilder, Error, Response, Secret,
    SubmissionError, UploadFile,
};
pub use interaction::{
    Entry, Interaction, Resumed, Run, RunOutcome, Submitted, Thread, text_payload,
};
