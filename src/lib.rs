#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod client;
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
pub use client::{
    ApiError, BinaryResponse, CallError, Client, ClientBuilder, Error, Response, Secret, UploadFile,
};
pub use interaction::{Submitted, text_payload};
