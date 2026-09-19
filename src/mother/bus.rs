//! The agent message bus (S6-T1). Agents never call each other: worlds and the Mother exchange
//! [`AgentMessage`]s here, every message carries the turn's `trace_id`, and nested requests stop
//! at [`MAX_DEPTH`]. Payloads are facts, never raw content (the ledger rule applies).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::domain::Domain;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Request,
    Result,
    Observation,
    Conflict,
    MemoryProposal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub agent: String,
    pub world: Domain,
}

impl Address {
    pub fn new(agent: &str, world: Domain) -> Self {
        Self { agent: agent.into(), world }
    }
    pub fn mother() -> Self {
        Self::new("mother", Domain::Shared)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentMessage {
    pub trace_id: String,
    pub from: Address,
    pub to: Address,
    pub kind: Kind,
    pub domain: Domain,
    /// Nesting depth of a request chain; the Mother's own requests are depth 1.
    pub depth: u8,
    pub permission_ctx: serde_json::Value,
    pub payload: serde_json::Value,
    pub ts: DateTime<Utc>,
}

impl AgentMessage {
    pub fn new(trace_id: &str, from: Address, to: Address, kind: Kind, payload: serde_json::Value) -> Self {
        let domain = if from.world == to.world { from.world } else { Domain::Shared };
        Self {
            trace_id: trace_id.into(),
            from,
            to,
            kind,
            domain,
            depth: 1,
            permission_ctx: serde_json::json!({}),
            payload,
            ts: Utc::now(),
        }
    }
    pub fn depth(mut self, d: u8) -> Self {
        self.depth = d;
        self
    }
    pub fn permission(mut self, mode: &str, effects: &[&str]) -> Self {
        self.permission_ctx = serde_json::json!({ "mode": mode, "effects_used": effects });
        self
    }
}

/// Fan-out depth cap (concept §14.1): Mother → world → agent, no deeper.
pub const MAX_DEPTH: u8 = 2;
const RECENT: usize = 2_000;

#[derive(Debug, PartialEq, Eq)]
pub enum BusError {
    DepthExceeded(u8),
}

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusError::DepthExceeded(d) => write!(f, "request depth {d} exceeds the cap of {MAX_DEPTH}"),
        }
    }
}

impl std::error::Error for BusError {}

#[derive(Clone)]
pub struct AgentBus {
    tx: broadcast::Sender<AgentMessage>,
    recent: Arc<Mutex<VecDeque<AgentMessage>>>,
}

impl Default for AgentBus {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Self { tx, recent: Arc::new(Mutex::new(VecDeque::with_capacity(RECENT))) }
    }

    pub fn new_trace_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// Publish; requests deeper than [`MAX_DEPTH`] are refused so a chain can never run away.
    pub fn publish(&self, msg: AgentMessage) -> Result<(), BusError> {
        if msg.kind == Kind::Request && msg.depth > MAX_DEPTH {
            return Err(BusError::DepthExceeded(msg.depth));
        }
        {
            let mut r = self.recent.lock().unwrap_or_else(|e| e.into_inner());
            if r.len() >= RECENT {
                r.pop_front();
            }
            r.push_back(msg.clone());
        }
        let _ = self.tx.send(msg);
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentMessage> {
        self.tx.subscribe()
    }

    /// Every message of one turn, in order.
    pub fn trace(&self, trace_id: &str) -> Vec<AgentMessage> {
        self.recent.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|m| m.trace_id == trace_id).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.recent.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

static BUS: OnceLock<AgentBus> = OnceLock::new();

/// The process-wide bus (agents are built before `AppState` exists).
pub fn global() -> &'static AgentBus {
    BUS.get_or_init(AgentBus::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_caps_request_depth_and_keeps_traces() {
        let bus = AgentBus::new();
        let t = AgentBus::new_trace_id();
        bus.publish(AgentMessage::new(&t, Address::mother(), Address::new("work_mother", Domain::Work), Kind::Request, serde_json::json!({"task": "x"}))).unwrap();
        bus.publish(AgentMessage::new(&t, Address::new("work_mother", Domain::Work), Address::new("email", Domain::Work), Kind::Request, serde_json::json!({})).depth(2)).unwrap();
        let too_deep = AgentMessage::new(&t, Address::new("email", Domain::Work), Address::new("x", Domain::Work), Kind::Request, serde_json::json!({})).depth(3);
        assert!(matches!(bus.publish(too_deep), Err(BusError::DepthExceeded(3))));
        bus.publish(AgentMessage::new(&t, Address::new("work_mother", Domain::Work), Address::mother(), Kind::Result, serde_json::json!({"facts": []}))).unwrap();
        assert_eq!(bus.trace(&t).len(), 3);
        assert_eq!(bus.trace(&t)[0].domain, Domain::Shared, "mother ↔ world messages are shared");
        assert_eq!(bus.trace(&t)[1].domain, Domain::Work);
    }
}
