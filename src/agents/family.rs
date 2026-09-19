//! Family agent (S5-T3): important dates from memory (`date.*`) and an optional contacts CSV
//! (`CONTACTS_CSV_PATH`: `name,relation,birthday`). Content stays in memory / the file; cards
//! show names and day counts only. Replaced by the `contacts` table when migration 009 lands.

use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use serde::Serialize;

use crate::memory::{MemoryService, Scope};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ImportantDate {
    pub name: String,
    /// birthday | anniversary | event
    pub label: String,
    pub month: u32,
    pub day: u32,
    pub days_until: i64,
    pub source: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Contact {
    pub name: String,
    pub relation: String,
    pub birthday: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FamilyFacts {
    pub upcoming: Vec<ImportantDate>,
    pub contacts: usize,
    pub family_calendar_id: Option<String>,
    pub sources: Vec<&'static str>,
}

/// Days until the next occurrence of `month/day` from `today`.
pub fn days_until(today: NaiveDate, month: u32, day: u32) -> Option<i64> {
    let mut year = today.year();
    for _ in 0..2 {
        if let Some(d) = NaiveDate::from_ymd_opt(year, month, day) {
            if d >= today {
                return Some((d - today).num_days());
            }
        }
        year += 1;
    }
    None
}

const MONTHS: &[(&str, u32)] = &[
    ("jan", 1), ("feb", 2), ("mar", 3), ("apr", 4), ("may", 5), ("jun", 6),
    ("jul", 7), ("aug", 8), ("sep", 9), ("oct", 10), ("nov", 11), ("dec", 12),
];

/// Find a month/day in free text: `20 Oct`, `Oct 20`, `October 20th`, `2026-10-20`, `10-20`, `20/10`.
pub fn parse_month_day(text: &str) -> Option<(u32, u32)> {
    let l = text.to_lowercase();
    if let Some(c) = regex_lite_iso(&l) {
        return Some(c);
    }
    let tokens: Vec<&str> = l.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    for (i, tok) in tokens.iter().enumerate() {
        if let Some(&(_, m)) = MONTHS.iter().find(|(name, _)| tok.starts_with(name) && tok.len() >= 3) {
            let num = |s: &str| s.trim_end_matches(|c: char| c.is_alphabetic()).parse::<u32>().ok().filter(|d| (1..=31).contains(d));
            if let Some(d) = tokens.get(i + 1).and_then(|s| num(s)) {
                return Some((m, d));
            }
            if i > 0 {
                if let Some(d) = num(tokens[i - 1]) {
                    return Some((m, d));
                }
            }
        }
    }
    None
}

fn regex_lite_iso(l: &str) -> Option<(u32, u32)> {
    // YYYY-MM-DD, MM-DD, DD/MM
    for part in l.split_whitespace() {
        let nums: Vec<u32> = part.split(['-', '/', '.']).filter_map(|p| p.parse().ok()).collect();
        match nums.as_slice() {
            [y, m, d] if *y > 31 && (1..=12).contains(m) && (1..=31).contains(d) => return Some((*m, *d)),
            [m, d] if part.contains('-') && (1..=12).contains(m) && (1..=31).contains(d) => return Some((*m, *d)),
            [d, m] if part.contains('/') && (1..=12).contains(m) && (1..=31).contains(d) => return Some((*m, *d)),
            _ => {}
        }
    }
    None
}

/// `Sara's birthday is 20 Oct` → ("Sara", "birthday").
pub fn parse_person_and_label(text: &str) -> (String, String) {
    let l = text.to_lowercase();
    let label = if l.contains("anniversary") { "anniversary" } else if l.contains("birthday") { "birthday" } else { "event" };
    let name = text
        .split(|c: char| c == '\'' || c == '’')
        .next()
        .map(|s| s.trim().trim_start_matches("remember").trim().to_string())
        .filter(|s| !s.is_empty() && s.split_whitespace().count() <= 3)
        .unwrap_or_else(|| "Someone".into());
    let name = name.split_whitespace().last().unwrap_or("Someone").to_string();
    let mut chars = name.chars();
    let name = match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => name,
    };
    (name, label.into())
}

pub fn parse_contacts_csv(text: &str) -> Vec<Contact> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.to_lowercase().starts_with("name"))
        .filter_map(|l| {
            let cols: Vec<&str> = l.split(',').map(str::trim).collect();
            let name = cols.first()?.to_string();
            if name.is_empty() {
                return None;
            }
            Some(Contact {
                name,
                relation: cols.get(1).unwrap_or(&"").to_string(),
                birthday: cols.get(2).and_then(|b| parse_month_day(b)),
            })
        })
        .collect()
}

pub fn contacts_from_env() -> Vec<Contact> {
    std::env::var("CONTACTS_CSV_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_contacts_csv(&t))
        .unwrap_or_default()
}

/// Dates within the next `horizon_days`, soonest first.
pub async fn facts(memory: &MemoryService, user_id: &str, today: NaiveDate) -> FamilyFacts {
    facts_with(memory, user_id, today, 30, contacts_from_env()).await
}

pub async fn facts_with(memory: &MemoryService, user_id: &str, today: NaiveDate, horizon_days: i64, contacts: Vec<Contact>) -> FamilyFacts {
    let mut f = FamilyFacts::default();
    let items = memory.read(user_id, Scope { domain: crate::domain::Domain::Home }, Some(&["date.".to_string()])).await;
    if !items.is_empty() {
        f.sources.push("memory");
    }
    for item in items {
        let text = item.value.as_str().map(str::to_string).unwrap_or_else(|| item.value.to_string());
        let Some((m, d)) = parse_month_day(&text) else { continue };
        let Some(days) = days_until(today, m, d) else { continue };
        if days <= horizon_days {
            let (name, label) = parse_person_and_label(&text);
            f.upcoming.push(ImportantDate { name, label, month: m, day: d, days_until: days, source: "memory" });
        }
    }
    f.contacts = contacts.len();
    if !contacts.is_empty() {
        f.sources.push("contacts_csv");
    }
    for c in contacts {
        if let Some((m, d)) = c.birthday {
            if let Some(days) = days_until(today, m, d) {
                if days <= horizon_days && !f.upcoming.iter().any(|u| u.name == c.name && u.month == m && u.day == d) {
                    f.upcoming.push(ImportantDate { name: c.name, label: "birthday".into(), month: m, day: d, days_until: days, source: "contacts_csv" });
                }
            }
        }
    }
    f.upcoming.sort_by_key(|u| u.days_until);
    f.family_calendar_id = memory.profile(user_id, "family_calendar_id").await;
    f
}

/// adk agent form (deterministic) for workflows that want a Family card from a runner.
pub async fn build(memory: MemoryService) -> anyhow::Result<Arc<dyn adk_core::Agent>> {
    use adk_agent::CustomAgentBuilder;
    use adk_core::{Content, Event};
    use futures::stream;
    let agent = CustomAgentBuilder::new("family_agent")
        .description("Family — important dates and household coordination from memory and contacts")
        .handler(move |ctx| {
            let memory = memory.clone();
            async move {
                let f = facts(&memory, ctx.user_id(), chrono::Local::now().date_naive()).await;
                let text = if f.upcoming.is_empty() {
                    "No family dates in the next 30 days on file.".to_string()
                } else {
                    f.upcoming.iter().map(|u| format!("{} — {} in {} days", u.label, u.name, u.days_until)).collect::<Vec<_>>().join("; ")
                };
                let mut event = Event::new("family");
                event.author = "family_agent".to_string();
                event.llm_response.content = Some(Content::new("assistant").with_text(text));
                Ok(Box::pin(stream::iter(vec![Ok(event)])) as adk_core::EventStream)
            }
        })
        .build()?;
    Ok(Arc::new(agent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_date_parsing() {
        assert_eq!(parse_month_day("Sara's birthday is 20 Oct"), Some((10, 20)));
        assert_eq!(parse_month_day("Oct 20th is Sara's birthday"), Some((10, 20)));
        assert_eq!(parse_month_day("anniversary 2026-06-14"), Some((6, 14)));
        assert_eq!(parse_month_day("Mum 03-09"), Some((3, 9)));
        assert_eq!(parse_month_day("no date here"), None);
        assert_eq!(parse_person_and_label("Sara's birthday is 20 Oct"), ("Sara".into(), "birthday".into()));
        let today = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(days_until(today, 10, 20), Some(19));
        assert_eq!(days_until(today, 1, 5), Some(96));
        let c = parse_contacts_csv("name,relation,birthday\nSara,partner,10-20\nMum,mother,1970-03-09\n");
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].birthday, Some((3, 9)));
    }
}
