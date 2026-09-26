use reqwest::header::HeaderMap;
use std::fmt;

/// The local phase in which transport failed, not evidence of Service rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportStage {
    Build,
    Request,
    Body,
}

/// Safe categories exposed by the HTTP transport, without retaining its raw cause.
/// Connect includes DNS, TLS and connection failures; it does not identify which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    Timeout,
    Connect,
    Builder,
    Request,
    Body,
    Decode,
    Other,
    /// A clean SSE EOF exhausted the explicitly configured reconnect budget.
    StreamEnded,
}

/// Transport diagnostics never retain URLs, bodies, credentials or a raw cause.
/// No category permits automatic mutation retry or proves rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportError {
    pub stage: TransportStage,
    pub kind: TransportKind,
}
impl TransportError {
    pub(crate) fn classify(error: reqwest::Error, stage: TransportStage) -> crate::Error {
        let kind = if error.is_timeout() {
            TransportKind::Timeout
        } else if error.is_connect() {
            TransportKind::Connect
        } else if error.is_builder() {
            TransportKind::Builder
        } else if error.is_body() {
            TransportKind::Body
        } else if error.is_decode() {
            TransportKind::Decode
        } else if error.is_request() {
            TransportKind::Request
        } else {
            TransportKind::Other
        };
        crate::Error::Transport(Self { stage, kind })
    }
}
impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Service transport failed ({:?}: {:?}); mutation outcome may be unknown",
            self.stage, self.kind
        )
    }
}
impl std::error::Error for TransportError {}

/// The violated response or observation boundary, without untrusted response text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolKind {
    InvalidJson,
    ResponseTooLarge,
    InvalidReceipt,
    RepeatedCursor,
    UnexpectedContentType,
    InvalidFrame,
    FrameTooLarge,
    InvalidUtf8,
    IncompleteFrame,
}

/// Protocol diagnostics. Request ID is explicitly readable but omitted by Display
/// and Debug because it is untrusted response data. No response body is retained.
#[derive(Clone)]
pub struct ProtocolError {
    pub kind: ProtocolKind,
    pub status: Option<u16>,
    pub request_id: Option<String>,
}
impl ProtocolError {
    pub(crate) fn local(kind: ProtocolKind) -> crate::Error {
        crate::Error::Protocol(Self {
            kind,
            status: None,
            request_id: None,
        })
    }
    pub(crate) fn response(kind: ProtocolKind, status: u16, headers: &HeaderMap) -> crate::Error {
        crate::Error::Protocol(Self {
            kind,
            status: Some(status),
            request_id: headers
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        })
    }
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid Service response ({:?}", self.kind)?;
        if let Some(status) = self.status {
            write!(f, "; HTTP {status}")?;
        }
        f.write_str(")")
    }
}
impl fmt::Debug for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolError")
            .field("kind", &self.kind)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
impl std::error::Error for ProtocolError {}
