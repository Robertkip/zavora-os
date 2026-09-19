//! Arbitration v1 (S6-T5) and the work/home overlap check (S6-T4).
//!
//! Runs after synthesis, before anything reaches the user:
//! - collapses duplicate proposals (same agent, same action) into one;
//! - finds a work commitment that overlaps a home commitment or the user's protected time
//!   (both read from memory as stated by the user) and turns it into **one** neutral question;
//! - health and finance facts pass through untouched.
//! The full Balance Agent (S8, Jotham) replaces the overlap heuristic with ledger-based
//! `conflict` observations; the question wording and the one-question rule stay here.

use serde::Serialize;

use crate::domain::Domain;
use crate::memory::{Kind, MemoryService, Scope};
use crate::mother::synth::{SuggestedAction, Synthesis};
use crate::permissions::Mode;

/// Minutes of the day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Window {
    pub start: u32,
    pub end: u32,
}

impl Window {
    pub fn overlaps(&self, other: &Window) -> bool {
        self.start < other.end && other.start < self.end
    }
    pub fn label(&self) -> String {
        format!("{:02}:{:02}–{:02}:{:02}", self.start / 60, self.start % 60, self.end / 60, self.end % 60)
    }
}

fn parse_clock(tok: &str) -> Option<u32> {
    let t = tok.trim().to_lowercase();
    let (body, pm) = if let Some(b) = t.strip_suffix("pm") { (b.trim().to_string(), Some(true)) } else if let Some(b) = t.strip_suffix("am") { (b.trim().to_string(), Some(false)) } else { (t, None) };
    let (h, m) = match body.split_once([':', '.']) {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    if h > 23 || m > 59 {
        return None;
    }
    let h = match pm {
        Some(true) if h < 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    };
    Some(h * 60 + m)
}

/// `17:30-19:00`, `17:30–19:00`, `6pm to 8pm`, `at 18:30` (default 90 minutes), `after 18:00`.
pub fn parse_window(text: &str) -> Option<Window> {
    let l = text.to_lowercase().replace('–', "-").replace(" to ", "-").replace(" until ", "-");
    let tokens: Vec<&str> = l.split_whitespace().collect();
    for tok in &tokens {
        if let Some((a, b)) = tok.split_once('-') {
            if let (Some(s), Some(e)) = (parse_clock(a), parse_clock(b)) {
                if e > s {
                    return Some(Window { start: s, end: e });
                }
            }
        }
    }
    for (i, tok) in tokens.iter().enumerate() {
        let after = tok.starts_with("after") || tok.starts_with("from");
        if let Some(next) = tokens.get(i + 1) {
            let clock = next.trim_end_matches(|c: char| c == ',' || c == '.');
            if (*tok == "at" || after) && clock.contains(':') || (*tok == "at" || after) && (clock.ends_with("pm") || clock.ends_with("am")) {
                if let Some(s) = parse_clock(clock) {
                    return Some(Window { start: s, end: if after { 23 * 60 + 59 } else { (s + 90).min(23 * 60 + 59) } });
                }
            }
        }
    }
    None
}

/// A day word the user used, normalized (`thursday`, `tomorrow`, `today`); `None` = unspecified.
pub fn parse_day(text: &str) -> Option<String> {
    let l = text.to_lowercase();
    for d in ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "tomorrow", "today", "tonight"] {
        if l.contains(d) {
            return Some(if d == "tonight" { "today".into() } else { d.into() });
        }
    }
    None
}

#[derive(Clone, Debug, Serialize)]
pub struct Commitment {
    pub domain: Domain,
    pub label: String,
    pub day: Option<String>,
    pub window: Window,
}

#[derive(Clone, Debug, Serialize)]
pub struct Conflict {
    pub work: Commitment,
    pub home: Commitment,
}

fn short(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(5).collect();
    words.join(" ")
}

/// Commitments the user stated in memory (known or assumed) that carry a time window.
pub async fn commitments(memory: &MemoryService, user_id: &str) -> Vec<Commitment> {
    memory
        .read(user_id, Scope::MOTHER, None)
        .await
        .into_iter()
        .filter(|i| i.kind != Kind::Recommended && !i.key.starts_with("preference."))
        .filter_map(|i| {
            let text = i.value.as_str()?.to_string();
            let window = parse_window(&text)?;
            Some(Commitment { domain: i.domain, label: short(&text), day: parse_day(&text), window })
        })
        .collect()
}

/// Work × home pairs on the same (or unspecified) day whose windows overlap.
pub fn find_conflicts(items: &[Commitment]) -> Vec<Conflict> {
    let mut out = Vec::new();
    for w in items.iter().filter(|c| c.domain == Domain::Work) {
        for h in items.iter().filter(|c| c.domain == Domain::Home) {
            let same_day = match (&w.day, &h.day) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            if same_day && w.window.overlaps(&h.window) {
                out.push(Conflict { work: w.clone(), home: h.clone() });
            }
        }
    }
    out
}

/// The user's protected time (`preference.protected_time`), if it parses to a window.
pub async fn protected_time(memory: &MemoryService, user_id: &str) -> Option<Window> {
    memory
        .read(user_id, Scope::MOTHER, Some(&["preference.protected_time".to_string()]))
        .await
        .into_iter()
        .find_map(|i| i.value.as_str().and_then(parse_window))
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Arbitration {
    pub question: Option<String>,
    pub conflicts: usize,
    pub protected_violations: usize,
    pub duplicates_dropped: usize,
}

fn norm(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Review a synthesis in place: dedupe proposals, add at most one question.
pub async fn review(memory: &MemoryService, user_id: &str, synthesis: &mut Synthesis) -> Arbitration {
    let mut arb = Arbitration::default();

    // 1. Duplicate proposals → one.
    let mut seen = std::collections::HashSet::new();
    let before = synthesis.actions.len();
    synthesis.actions.retain(|a| seen.insert((a.agent.clone(), norm(&a.text))));
    arb.duplicates_dropped = before - synthesis.actions.len();

    // 2. Work ↔ home overlap and protected time, from what the user stated.
    let items = commitments(memory, user_id).await;
    let conflicts = find_conflicts(&items);
    arb.conflicts = conflicts.len();
    let protected = protected_time(memory, user_id).await;
    let violations: Vec<&Commitment> = protected
        .map(|p| items.iter().filter(|c| c.domain == Domain::Work && c.window.overlaps(&p)).collect())
        .unwrap_or_default();
    arb.protected_violations = violations.len();

    // 3. One question, neutral wording (concept §9.3 / §13.3).
    let question = if let Some(c) = conflicts.first() {
        let when = c.home.day.as_deref().map(|d| format!(" {d}")).unwrap_or_default();
        let more = if conflicts.len() > 1 { format!(" (and {} more overlap{})", conflicts.len() - 1, if conflicts.len() > 2 { "s" } else { "" }) } else { String::new() };
        Some(format!(
            "You have a family commitment{when} at {}, but your current work schedule extends into that period ({} {}){more}. Would you like me to help reorganize your tasks?",
            c.home.window.label().split('–').next().unwrap_or(""),
            c.work.label,
            c.work.window.label()
        ))
    } else if let (Some(p), Some(v)) = (protected, violations.first()) {
        Some(format!(
            "{} ({}) falls inside the time you protect ({}). Would you like me to help move it?",
            v.label,
            v.window.label(),
            p.label()
        ))
    } else {
        None
    };

    if let Some(q) = &question {
        synthesis.html = format!("{q} {}", synthesis.html);
        synthesis.actions.insert(
            0,
            SuggestedAction {
                text: "Reorganize my tasks around the commitment".into(),
                card: "Arbitration".into(),
                agent: "productivity".into(),
                domain: Domain::Work,
                mode: Mode::Suggest,
            },
        );
    }
    arb.question = question;
    arb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitration_parses_windows_and_finds_overlaps() {
        assert_eq!(parse_window("client review Thursday 17:30-19:00"), Some(Window { start: 1050, end: 1140 }));
        assert_eq!(parse_window("family dinner Thursday at 18:30"), Some(Window { start: 1110, end: 1200 }));
        assert_eq!(parse_window("protect evenings after 18:00"), Some(Window { start: 1080, end: 1439 }));
        assert_eq!(parse_window("6pm to 8pm gym"), Some(Window { start: 1080, end: 1200 }));
        assert_eq!(parse_window("no time here"), None);
        assert_eq!(parse_day("dinner Thursday"), Some("thursday".into()));
        let items = vec![
            Commitment { domain: Domain::Work, label: "client review".into(), day: Some("thursday".into()), window: Window { start: 1050, end: 1140 } },
            Commitment { domain: Domain::Home, label: "family dinner".into(), day: Some("thursday".into()), window: Window { start: 1110, end: 1200 } },
            Commitment { domain: Domain::Home, label: "yoga".into(), day: Some("friday".into()), window: Window { start: 1110, end: 1200 } },
        ];
        let c = find_conflicts(&items);
        assert_eq!(c.len(), 1, "different days never conflict");
        assert_eq!(c[0].home.label, "family dinner");
    }
}
