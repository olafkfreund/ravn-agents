//! The [`Message`] envelope: a detection [`Event`] plus its optional LLM
//! [`Explanation`].

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::event::Event;

/// LLM-produced enrichment for an event (epic #2).
///
/// Probabilistic and advisory: never used to decide *whether* something is
/// wrong — only to explain a already-flagged [`Event`] and suggest a check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Explanation {
    /// Concise (2–3 sentence) explanation of the event.
    pub text: String,
    /// A concrete command or check the operator can run, when the model offers
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_check: Option<String>,
    /// Identifier of the model that produced this explanation, for provenance
    /// and eval (epic #8).
    pub model: String,
    /// When the explanation was generated, in UTC.
    pub generated_at: DateTime<Utc>,
}

/// The transport/persistence envelope.
///
/// This is what crosses the agent → control-plane transport (#22), what the
/// control plane persists (#24), and what the portal feed renders (#29). The
/// `explanation` is absent until (and unless) inference enriches the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    pub event: Event,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<Explanation>,
}

impl Message {
    /// Wrap a bare detection event with no explanation yet.
    pub fn new(event: Event) -> Self {
        Self { event, explanation: None }
    }
}
