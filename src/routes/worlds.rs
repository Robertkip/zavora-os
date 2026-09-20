//! `GET /api/worlds` — the two worlds' agent rosters (concept §4–§5) for the world pages.
//!
//! A static catalog derived from the world registries: ids, titles, glyphs, missions, default
//! authority modes (from `mcp_allowlists.toml`), stub flags and a starter prompt per agent. No
//! personal data, so it is `anonymous` like the other rail feeds. The page shows the Work roster
//! in the Work world and the Home roster in the Home world; the session's live tiles overlay it.

use axum::{
    extract::State,
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Serialize;

use crate::domain::Domain;
use crate::state::AppState;
use crate::tools::allowlist;
use crate::worlds::{home, work};

#[derive(Serialize)]
pub struct WorldAgentView {
    pub id: &'static str,
    pub title: String,
    pub glyph: &'static str,
    pub world: Domain,
    pub mission: &'static str,
    /// Default authority mode of the Phase 1 agent that does the work (`observe|suggest|automate`).
    pub mode: String,
    /// Labeled stub until its MCP server exists (BK-101/102).
    pub stub: bool,
    pub scenario: &'static str,
    pub phase1_agents: &'static [&'static str],
    /// A natural prompt the Mother routes to this agent — what a click on the tile types.
    pub prompt: &'static str,
}

#[derive(Serialize)]
pub struct WorldsResponse {
    pub work: Vec<WorldAgentView>,
    pub home: Vec<WorldAgentView>,
}

pub fn glyph(id: &str) -> &'static str {
    match id {
        "productivity" => "📅",
        "email" => "✉️",
        "team_comms" => "💬",
        "project" => "🎯",
        "research_knowledge" => "🔎",
        "work_automation" => "⚙️",
        "career" => "🧭",
        "professional_social" => "💼",
        "family" => "🏠",
        "personal_productivity" => "✅",
        "health_wellness" => "💪",
        "finance" => "💳",
        "entertainment" => "📻",
        "social_fun" => "🎲",
        "travel" => "✈️",
        "personal_social" => "📱",
        _ => "✦",
    }
}

pub fn title(id: &str) -> String {
    match id {
        "team_comms" => "Team Comms".into(),
        "research_knowledge" => "Research & Knowledge".into(),
        "health_wellness" => "Health & Wellness".into(),
        "social_fun" => "Social & Fun".into(),
        other => other
            .split('_')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

pub fn prompt(id: &str) -> &'static str {
    match id {
        "productivity" => "What's on my plate today?",
        "email" => "What's happening in my inbox?",
        "team_comms" => "Catch me up on the team",
        "project" => "How are my projects doing?",
        "research_knowledge" => "Show me what you found",
        "work_automation" => "Build me a pitch deck",
        "career" => "Where am I on my career goals?",
        "professional_social" => "Anything on LinkedIn I should know about?",
        "family" => "Remind me about family commitments",
        "personal_productivity" => "What personal errands are due?",
        "health_wellness" => "How am I sleeping this week?",
        "finance" => "How's my spending?",
        "entertainment" => "What is happening live",
        "social_fun" => "Suggest something fun for the weekend",
        "travel" => "Plan a weekend trip to Lisbon",
        "personal_social" => "Anything on my socials?",
        _ => "What's happening?",
    }
}

fn mode_for(phase1_agents: &[&str]) -> String {
    phase1_agents
        .first()
        .map(|a| allowlist::catalog().mode_for(a).as_str().to_string())
        .unwrap_or_else(|| "observe".into())
}

pub fn rosters() -> WorldsResponse {
    WorldsResponse {
        work: work::WORK_AGENTS
            .iter()
            .map(|a| WorldAgentView {
                id: a.id,
                title: title(a.id),
                glyph: glyph(a.id),
                world: Domain::Work,
                mission: a.mission,
                mode: mode_for(a.phase1_agents),
                stub: a.stub,
                scenario: a.scenario,
                phase1_agents: a.phase1_agents,
                prompt: prompt(a.id),
            })
            .collect(),
        home: home::HOME_AGENTS
            .iter()
            .map(|a| WorldAgentView {
                id: a.id,
                title: title(a.id),
                glyph: glyph(a.id),
                world: Domain::Home,
                mission: a.mission,
                mode: mode_for(a.phase1_agents),
                stub: a.stub,
                scenario: a.scenario,
                phase1_agents: a.phase1_agents,
                prompt: prompt(a.id),
            })
            .collect(),
    }
}

pub async fn get_worlds(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<WorldsResponse>, Response> {
    state.awp.check(&headers, "worlds", "get_worlds").await?;
    Ok(Json(rosters()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rosters_cover_both_registries_with_titles_glyphs_and_prompts() {
        let r = rosters();
        assert_eq!(r.work.len(), work::WORK_AGENTS.len());
        assert_eq!(r.home.len(), home::HOME_AGENTS.len());
        assert!(r.work.iter().all(|a| a.world == Domain::Work));
        assert!(r.home.iter().all(|a| a.world == Domain::Home));
        for a in r.work.iter().chain(r.home.iter()) {
            assert_ne!(a.glyph, "✦", "no glyph for {}", a.id);
            assert!(!a.title.is_empty() && !a.title.contains('_'), "{}", a.title);
            assert_ne!(a.prompt, "What's happening?", "no prompt for {}", a.id);
            assert!(["observe", "suggest", "automate"].contains(&a.mode.as_str()));
        }
        assert_eq!(title("research_knowledge"), "Research & Knowledge");
        assert_eq!(title("personal_productivity"), "Personal Productivity");
        let work_ids: Vec<&str> = r.work.iter().map(|a| a.id).collect();
        let home_ids: Vec<&str> = r.home.iter().map(|a| a.id).collect();
        assert!(work_ids.iter().all(|id| !home_ids.contains(id)), "the two rosters must not overlap");
    }
}
