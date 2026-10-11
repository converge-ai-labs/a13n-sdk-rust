//! Bounded, conservative projection, not a display reducer or transcript buffer.
use crate::{Error, streaming::ThreadFrame};
use serde_json::Value;
use std::{collections::HashMap, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewInterruptedKind {
    Gap,
    Reset,
    Fragment,
    MissingStart,
    AmbiguousIdentity,
    Discontinuity,
    Capacity,
    InvalidProtocol,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewInterruptedError {
    pub kind: PreviewInterruptedKind,
}
impl fmt::Display for PreviewInterruptedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Text preview interrupted ({:?}); final Run output remains authoritative",
            self.kind
        )
    }
}
impl std::error::Error for PreviewInterruptedError {}
#[derive(Default)]
pub(crate) struct Preview {
    // Eight simultaneously active root text identities; no completed history or text retained.
    active: HashMap<String, bool>,
    position: Option<(i64, i64)>,
    interrupted: Option<PreviewInterruptedKind>,
}
impl Preview {
    pub(crate) fn check(&self) -> Result<(), Error> {
        match self.interrupted {
            Some(kind) => Err(Error::PreviewInterrupted(PreviewInterruptedError { kind })),
            None => Ok(()),
        }
    }
    pub(crate) fn interrupt(&mut self, kind: PreviewInterruptedKind) -> Error {
        let kind = *self.interrupted.get_or_insert(kind);
        self.active.clear();
        Error::PreviewInterrupted(PreviewInterruptedError { kind })
    }
    pub(crate) fn project(&mut self, frame: &ThreadFrame) -> Result<Option<String>, Error> {
        self.check()?;
        let (attempt, sequence, delta) = match frame {
            ThreadFrame::Gap(_) => return Err(self.interrupt(PreviewInterruptedKind::Gap)),
            ThreadFrame::Reset(_) => return Err(self.interrupt(PreviewInterruptedKind::Reset)),
            ThreadFrame::Delta { data, .. } => (data.attempt, data.sequence, true),
            ThreadFrame::Boundary { data, .. } => (data.attempt, data.sequence, false),
            ThreadFrame::Changed(_) => return Ok(None),
        };
        // Only Service's outer per-attempt sequence is dense. Native event/value sequence
        // and foreign Run coordinates are unrelated and never participate in this check.
        if attempt < 0
            || sequence < 0
            || self.position.is_some_and(|(a, s)| {
                a != attempt
                    || if delta {
                        s.checked_add(1) != Some(sequence)
                    } else {
                        sequence > s
                    }
            })
        {
            return Err(self.interrupt(PreviewInterruptedKind::Discontinuity));
        }
        // Checkpoint I/O can append an already-covered marker after newer deltas.
        // Such a marker is observable but must never move the coordinate backwards.
        if delta || self.position.is_none() {
            self.position = Some((attempt, sequence));
        }
        let ThreadFrame::Delta { data, .. } = frame else {
            return Ok(None);
        };
        let event = &data.event;
        if event.get("subagentRunId").is_some_and(|v| !v.is_null()) {
            return Ok(None);
        }
        let hidden =
            event.get("metadata").and_then(|v| v.get("display")) == Some(&Value::Bool(false));
        let kind = event.get("type").and_then(Value::as_str);
        if kind == Some("CUSTOM") {
            if !hidden && event.get("name").and_then(Value::as_str) == Some("a13n.stream.fragment")
            {
                // Opaque framing of ORIGINAL CUSTOM JSON, not a text suffix. No assembly.
                return Err(self.interrupt(PreviewInterruptedKind::Fragment));
            }
            return Ok(None);
        }
        if !matches!(
            kind,
            Some("TEXT_MESSAGE_START" | "TEXT_MESSAGE_CONTENT" | "TEXT_MESSAGE_END")
        ) {
            return Ok(None);
        }
        let Some(id) = event
            .get("messageId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            if hidden {
                return Ok(None);
            }
            return Err(self.interrupt(PreviewInterruptedKind::AmbiguousIdentity));
        };
        match kind {
            Some("TEXT_MESSAGE_START") => {
                if self.active.contains_key(id) {
                    return Err(self.interrupt(PreviewInterruptedKind::AmbiguousIdentity));
                }
                if self.active.len() >= 8 {
                    return Err(self.interrupt(PreviewInterruptedKind::Capacity));
                }
                let role = event.get("role").and_then(Value::as_str);
                if role.is_none() && !hidden {
                    return Err(self.interrupt(PreviewInterruptedKind::MissingStart));
                }
                self.active
                    .insert(id.into(), !hidden && role == Some("assistant"));
                Ok(None)
            }
            Some("TEXT_MESSAGE_CONTENT") => {
                if hidden {
                    return Ok(None);
                }
                let Some(visible) = self.active.get(id) else {
                    return Err(self.interrupt(PreviewInterruptedKind::MissingStart));
                };
                if !visible {
                    return Ok(None);
                }
                let Some(text) = event.get("delta").and_then(Value::as_str) else {
                    return Err(self.interrupt(PreviewInterruptedKind::InvalidProtocol));
                };
                Ok(Some(text.to_owned()))
            }
            Some("TEXT_MESSAGE_END") => {
                if self.active.remove(id).is_none() && !hidden {
                    return Err(self.interrupt(PreviewInterruptedKind::MissingStart));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
