//! The Mother Agent — single orchestrator (ADR-001).
//!
//! Pipeline: intake → context → delegation → arbitration → synthesis → action gate.
//!
//! - [`intake`] classifies an utterance into worlds, targets, kind and urgency.
//! - [`delegate`] fans out to targets (Phase 1 scenario adapters until S4/S5), merges and
//!   persists the cards, and streams one field.
//! - [`synth`] composes one answer in Suzy's voice with mode-tagged actions.
//! - [`agent`] is the LLM half (internal tools only; no MCP).
//! - [`bus`] carries `AgentMessage`s between the Mother and the worlds (trace ids, depth cap).
//! - [`arbitrate`] dedupes proposals and turns a work/home clash into one neutral question.
//!
//! Every entry point (intent route, chat route, voice tools, `/awp/a2a`) calls
//! [`handle_intent`].

pub mod agent;
pub mod arbitrate;
pub mod bus;
pub mod delegate;
pub mod intake;
pub mod synth;

pub use delegate::{handle_intent, Entry, MotherRequest};

use crate::domain::Domain;
use crate::state::SessionRecord;

/// Counts of cards and active agents per life domain — the Mother Agent's cheap view of a
/// session's shape.
pub fn domain_summary(record: &SessionRecord) -> serde_json::Value {
    let mut cards = serde_json::Map::new();
    let mut agents = serde_json::Map::new();
    for d in Domain::ALL {
        let c = record.cards.iter().filter(|c| !c.removed && c.domain == d).count();
        let a = record.agents_active.iter().filter(|a| a.domain == d).count();
        cards.insert(d.as_str().into(), serde_json::json!(c));
        agents.insert(d.as_str().into(), serde_json::json!(a));
    }
    serde_json::json!({ "cards": cards, "agents_active": agents })
}
