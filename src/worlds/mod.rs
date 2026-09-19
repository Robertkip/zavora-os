//! Work World and Home World coordinating agents (ADR-002; concept §4–§5).
//!
//! [`work`] ships in S4 (`work_mother`, work agent registry, follow-up tracker). `home` lands in S5.
//! [`fan_out`] is what the Mother calls for a multi-target turn: every target runs concurrently;
//! targets that belong to a world with a mother are folded into that world's structured result
//! and the world may append outcomes of its own (follow-ups, labeled stubs).

pub mod home;
pub mod work;

use serde::Serialize;

/// Per-agent outcome inside a [`WorldResult`].
#[derive(Clone, Debug, Serialize)]
pub struct AgentOutcome {
    pub agent: String,
    pub scenario: String,
    pub cards: usize,
    pub resolved: usize,
    pub timed_out: bool,
}

/// One structured result for a whole world — what the Mother receives back from a world mother.
#[derive(Clone, Debug, Serialize)]
pub struct WorldResult {
    pub world: Domain,
    pub agents: Vec<AgentOutcome>,
    /// Content-free facts for synthesis ("2 threads unanswered for 3+ days", "1 date ahead").
    pub facts: Vec<String>,
    pub stubs: Vec<&'static str>,
}

impl WorldResult {
    pub fn new(world: Domain) -> Self {
        Self { world, agents: Vec::new(), facts: Vec::new(), stubs: Vec::new() }
    }

    /// Fold the collected events of this world's targets into per-agent outcomes.
    pub fn from_targets(world: Domain, targets: &[Target], collected: &[(Vec<serde_json::Value>, bool)]) -> Self {
        let mut r = Self::new(world);
        for (t, (events, timed_out)) in targets.iter().zip(collected) {
            if t.world != world {
                continue;
            }
            r.agents.push(AgentOutcome {
                agent: t.agent.clone(),
                scenario: t.scenario.clone(),
                cards: count_events(events, "card_spawn"),
                resolved: count_events(events, "card_resolve"),
                timed_out: *timed_out,
            });
        }
        r
    }
}

pub fn count_events(events: &[serde_json::Value], kind: &str) -> usize {
    events.iter().filter(|e| e.get("type").and_then(|t| t.as_str()) == Some(kind)).count()
}

/// A world-native card (no Phase 1 adapter behind it).
pub fn card(title: &str, agent: &str, glyph: &str, domain: Domain) -> serde_json::Value {
    serde_json::json!({ "glyph": glyph, "title": title, "agent": agent, "domain": domain.as_str(), "stream": ["Checking…"] })
}

/// Events for one already-resolved card, in the shape `merge_target_events` expects.
pub fn one_card_events(c: serde_json::Value, resolve: serde_json::Value, domain: Domain) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"type": "scenario", "key": domain.as_str(), "text": "", "total_cards": 1}),
        serde_json::json!({"type": "card_spawn", "index": 0, "card": c, "domain": domain.as_str()}),
        serde_json::json!({"type": "card_resolve", "index": 0, "resolve": resolve}),
    ]
}

/// Scenario keys that are world-native (no Phase 1 stream to run).
pub fn is_world_native(scenario: &str) -> bool {
    matches!(scenario, "stub" | "home" | "work")
}

/// Whether a world mother exists (and is enabled) for `world`.
pub fn has_mother(world: Domain) -> bool {
    match world {
        Domain::Work => work::enabled(),
        Domain::Home => home::enabled(),
        Domain::Shared => false,
    }
}

use std::time::Duration;

use crate::domain::Domain;
use crate::mother::intake::Target;
use crate::orchestrator::{dispatch, sse_collect};
use crate::state::AppState;

/// Per-target fan-out budget (mock scenarios finish in seconds; live MCP workflows take longer).
pub const TARGET_TIMEOUT: Duration = Duration::from_secs(90);

/// Run one target's scenario without persisting and collect its events.
pub async fn run_target(state: &AppState, session_id: &str, user_id: &str, text: &str, target: &Target) -> (Vec<serde_json::Value>, bool) {
    if is_world_native(&target.scenario) {
        return (Vec::new(), false);
    }
    let resp = dispatch::stream_scenario(state, &target.scenario, session_id.to_string(), user_id.to_string(), text.to_string(), false).await;
    match tokio::time::timeout(TARGET_TIMEOUT, sse_collect::collect_sse_events(resp)).await {
        Ok(events) => (events, false),
        Err(_) => (Vec::new(), true),
    }
}

/// What a fan-out produced, ready for `mother::delegate::merge_target_events`.
pub struct FanOut {
    pub targets: Vec<Target>,
    pub collected: Vec<(Vec<serde_json::Value>, bool)>,
    pub work: Option<WorldResult>,
    pub home: Option<WorldResult>,
}

/// Fan out all targets concurrently, then let each world fold its share into one structured
/// result and append its own outcomes.
pub async fn fan_out(state: &AppState, session_id: &str, user_id: &str, text: &str, targets: Vec<Target>, trace_id: &str) -> FanOut {
    use crate::mother::bus::{global as bus, Address, AgentMessage, Kind};
    for t in &targets {
        let world_agent = match t.world { Domain::Work => "work_mother", Domain::Home => "home_mother", Domain::Shared => &t.agent };
        let _ = bus().publish(
            AgentMessage::new(trace_id, Address::mother(), Address::new(world_agent, t.world), Kind::Request, serde_json::json!({ "agent": t.agent, "task": t.task, "scenario": t.scenario }))
                .permission("suggest", &[]),
        );
        if t.world != Domain::Shared {
            let _ = bus().publish(
                AgentMessage::new(trace_id, Address::new(world_agent, t.world), Address::new(&t.agent, t.world), Kind::Request, serde_json::json!({ "task": t.task, "scenario": t.scenario })).depth(2),
            );
        }
    }
    let futures: Vec<_> = targets.iter().map(|t| run_target(state, session_id, user_id, text, t)).collect();
    let mut collected: Vec<(Vec<serde_json::Value>, bool)> = futures::future::join_all(futures).await;
    let mut targets = targets;
    let mut work_result = None;
    let mut home_result = None;

    if work::enabled() && targets.iter().any(|t| t.world == Domain::Work) {
        let (result, extra) = work::fold(&state.ledger, user_id, &targets, &collected).await;
        for (t, events) in extra {
            targets.push(t);
            collected.push((events, false));
        }
        let _ = bus().publish(AgentMessage::new(trace_id, Address::new("work_mother", Domain::Work), Address::mother(), Kind::Result, serde_json::to_value(&result).unwrap_or_default()));
        work_result = Some(result);
    }
    if home::enabled() && targets.iter().any(|t| t.world == Domain::Home) {
        let (result, extra) = home::fold(&state.memory, &state.ledger, user_id, &targets, &collected).await;
        for (t, events) in extra {
            targets.push(t);
            collected.push((events, false));
        }
        let _ = bus().publish(AgentMessage::new(trace_id, Address::new("home_mother", Domain::Home), Address::mother(), Kind::Result, serde_json::to_value(&result).unwrap_or_default()));
        home_result = Some(result);
    }

    FanOut { targets, collected, work: work_result, home: home_result }
}
