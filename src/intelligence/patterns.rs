//! S7-T1 — Pattern Recognition: pure aggregation of the content-free activity ledger into one
//! value per (user, day, dimension), the `activity_daily` shape (concept §7.2).
//!
//! No wall clock and no I/O here: [`aggregate`] takes events and returns rows; persistence lives
//! in [`super::store`] and the statistics in [`super::baseline`]. Day boundaries are UTC for now;
//! the user's timezone is a known memory item and will move the boundary in a later task.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

use crate::domain::Domain;

use super::ledger::ActivityEvent;

/// Dimension ids (concept §7.2 / §8.3). One value per day.
pub mod dim {
    /// Hours since midnight (UTC) of the first work-domain event.
    pub const WORK_START: &str = "work.start";
    /// Hours since midnight (UTC) of the last work-domain event.
    pub const WORK_END: &str = "work.end";
    /// Focused work minutes (`session_focus` minutes, else last − first work event).
    pub const WORK_MINUTES: &str = "work.minutes";
    pub const READING_MINUTES: &str = "reading.minutes";
    pub const READING_SESSIONS: &str = "reading.sessions";
    /// `email` + `team_comms` tool calls in the work domain.
    pub const COMMS_WORK_VOLUME: &str = "comms.work.volume";
    /// `family` agent tool calls.
    pub const COMMS_FAMILY_VOLUME: &str = "comms.family.volume";
    pub const TASKS_POSTPONED: &str = "tasks.postponed";
    pub const EXERCISE_SESSIONS: &str = "exercise.sessions";
    pub const SLEEP_HOURS: &str = "sleep.hours";

    pub const ALL: &[&str] = &[
        WORK_START,
        WORK_END,
        WORK_MINUTES,
        READING_MINUTES,
        READING_SESSIONS,
        COMMS_WORK_VOLUME,
        COMMS_FAMILY_VOLUME,
        TASKS_POSTPONED,
        EXERCISE_SESSIONS,
        SLEEP_HOURS,
    ];
}

/// Unit of a dimension's value, carried in observation facts.
pub fn unit(dimension: &str) -> &'static str {
    match dimension {
        dim::WORK_START | dim::WORK_END => "clock_hours",
        dim::WORK_MINUTES | dim::READING_MINUTES => "minutes",
        dim::SLEEP_HOURS => "hours",
        _ => "count",
    }
}

/// Life domain a dimension belongs to (the `domain` column; what the Balance agent reads across).
pub fn domain_of(dimension: &str) -> Domain {
    match dimension {
        d if d.starts_with("work.") || d == dim::COMMS_WORK_VOLUME => Domain::Work,
        dim::COMMS_FAMILY_VOLUME | dim::TASKS_POSTPONED | dim::EXERCISE_SESSIONS | dim::SLEEP_HOURS => Domain::Home,
        _ => Domain::Shared,
    }
}

/// One row of `activity_daily`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DailyValue {
    pub user_id: String,
    pub day: NaiveDate,
    pub dimension: String,
    pub value: f64,
    /// Ledger events (of any kind) that contributed to this day.
    pub sample_n: usize,
}

#[derive(Default)]
struct DayAcc {
    any: bool,
    count: usize,
    work_first: Option<f64>,
    work_last: Option<f64>,
    focus_minutes: f64,
    has_focus: bool,
    reading_minutes: f64,
    reading_sessions: u32,
    comms_work: u32,
    comms_family: u32,
    postponed: u32,
    exercise: u32,
    sleep_hours: Option<f64>,
}

fn clock_hours(ts: &DateTime<FixedOffset>) -> f64 {
    ts.hour() as f64 + ts.minute() as f64 / 60.0 + ts.second() as f64 / 3600.0
}

fn meta_f64(ev: &ActivityEvent, key: &str) -> Option<f64> {
    ev.meta.get(key).and_then(|v| v.as_f64())
}

fn minutes_of(ev: &ActivityEvent) -> Option<f64> {
    meta_f64(ev, "minutes").or_else(|| ev.duration_ms.map(|d| d as f64 / 60_000.0))
}

/// Aggregate ledger events into daily values. Pure; order of `events` does not matter.
/// Count dimensions are emitted as 0 for any day that has at least one event, so "no reading
/// today" is a data point and "no data today" is not.
pub fn aggregate(events: &[ActivityEvent]) -> Vec<DailyValue> {
    aggregate_in(events, &FixedOffset::east_opt(0).expect("zero offset"))
}

/// Like [`aggregate`], with day boundaries and clock hours taken in a fixed UTC offset (for
/// example +3 h for Nairobi). Fixed offsets cover zones without daylight saving; full timezone
/// rules arrive with the user's `profile.timezone` memory item.
pub fn aggregate_in(events: &[ActivityEvent], offset: &FixedOffset) -> Vec<DailyValue> {
    let mut acc: BTreeMap<(String, NaiveDate), DayAcc> = BTreeMap::new();
    for ev in events {
        let local = ev.ts.with_timezone(offset);
        let day = local.date_naive();
        let a = acc.entry((ev.user_id.clone(), day)).or_default();
        a.any = true;
        a.count += 1;
        if ev.domain == Domain::Work {
            let h = clock_hours(&local);
            a.work_first = Some(a.work_first.map_or(h, |f| f.min(h)));
            a.work_last = Some(a.work_last.map_or(h, |l| l.max(h)));
        }
        match ev.kind.as_str() {
            "session_focus" => {
                if let Some(m) = minutes_of(ev) {
                    a.focus_minutes += m;
                    a.has_focus = true;
                }
            }
            "reading" => {
                a.reading_minutes += minutes_of(ev).unwrap_or(0.0);
                a.reading_sessions += 1;
            }
            "task_postponed" => a.postponed += 1,
            "exercise" => a.exercise += 1,
            "sleep" => {
                let hours = meta_f64(ev, "hours").or_else(|| ev.duration_ms.map(|d| d as f64 / 3_600_000.0));
                if let Some(h) = hours {
                    a.sleep_hours = Some(a.sleep_hours.map_or(h, |s| s.max(h)));
                }
            }
            "tool_call" => match ev.agent_id.as_str() {
                "email" | "team_comms" if ev.domain == Domain::Work => a.comms_work += 1,
                "family" => a.comms_family += 1,
                _ => {}
            },
            _ => {}
        }
    }

    let mut out = Vec::with_capacity(acc.len() * 8);
    for ((user_id, day), a) in acc {
        let sample_n = a.count;
        let mut push = |dimension: &str, value: f64| {
            out.push(DailyValue { user_id: user_id.clone(), day, dimension: dimension.to_string(), value, sample_n });
        };
        if let (Some(first), Some(last)) = (a.work_first, a.work_last) {
            push(dim::WORK_START, first);
            push(dim::WORK_END, last);
            let minutes = if a.has_focus { a.focus_minutes } else { (last - first) * 60.0 };
            push(dim::WORK_MINUTES, minutes);
        }
        if a.any {
            push(dim::READING_MINUTES, a.reading_minutes);
            push(dim::READING_SESSIONS, a.reading_sessions as f64);
            push(dim::COMMS_WORK_VOLUME, a.comms_work as f64);
            push(dim::COMMS_FAMILY_VOLUME, a.comms_family as f64);
            push(dim::TASKS_POSTPONED, a.postponed as f64);
            push(dim::EXERCISE_SESSIONS, a.exercise as f64);
        }
        if let Some(h) = a.sleep_hours {
            push(dim::SLEEP_HOURS, h);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 14, h, m, 0).unwrap()
    }

    #[test]
    fn aggregate_in_offset_moves_day_boundary_and_clock() {
        let mut ev = ActivityEvent::new("u", Domain::Work, "email", "tool_call");
        ev.ts = at(22, 30); // 22:30 UTC on 14 Sep = 01:30 on 15 Sep in Nairobi
        let nairobi = FixedOffset::east_opt(3 * 3600).unwrap();
        let daily = aggregate_in(&[ev], &nairobi);
        let end = daily.iter().find(|r| r.dimension == dim::WORK_END).unwrap();
        assert_eq!(end.day, NaiveDate::from_ymd_opt(2026, 9, 15).unwrap());
        assert!((end.value - 1.5).abs() < 1e-9);
        assert_eq!(end.sample_n, 1);
    }

    #[test]
    fn aggregate_builds_work_window_and_counts_per_day() {
        let mut events = vec![
            ActivityEvent::new("u", Domain::Work, "mother", "intent"),
            ActivityEvent::new("u", Domain::Work, "email", "tool_call"),
            ActivityEvent::new("u", Domain::Work, "productivity", "session_focus").meta(serde_json::json!({"minutes": 420})),
            ActivityEvent::new("u", Domain::Home, "family", "tool_call"),
            ActivityEvent::new("u", Domain::Shared, "reading_knowledge", "reading").meta(serde_json::json!({"minutes": 25.5})),
            ActivityEvent::new("u", Domain::Home, "health_wellness", "sleep").meta(serde_json::json!({"hours": 7.2})),
        ];
        let times = [at(9, 0), at(12, 30), at(17, 45), at(20, 0), at(21, 15), at(7, 0)];
        for (ev, t) in events.iter_mut().zip(times) {
            ev.ts = t;
        }
        let daily = aggregate(&events);
        let get = |d: &str| daily.iter().find(|r| r.dimension == d).map(|r| r.value).unwrap();
        assert!((get(dim::WORK_START) - 9.0).abs() < 1e-9);
        assert!((get(dim::WORK_END) - 17.75).abs() < 1e-9);
        assert_eq!(get(dim::WORK_MINUTES), 420.0);
        assert_eq!(get(dim::COMMS_WORK_VOLUME), 1.0);
        assert_eq!(get(dim::COMMS_FAMILY_VOLUME), 1.0);
        assert_eq!(get(dim::READING_MINUTES), 25.5);
        assert_eq!(get(dim::READING_SESSIONS), 1.0);
        assert_eq!(get(dim::SLEEP_HOURS), 7.2);
        assert_eq!(get(dim::TASKS_POSTPONED), 0.0);
        assert!(daily.iter().all(|r| r.day == NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()));
    }

    #[test]
    fn aggregate_without_work_events_emits_no_work_dimensions() {
        let ev = ActivityEvent::new("u", Domain::Home, "family", "tool_call");
        let daily = aggregate(&[ev]);
        assert!(daily.iter().all(|r| !r.dimension.starts_with("work.")));
        assert!(daily.iter().any(|r| r.dimension == dim::COMMS_FAMILY_VOLUME && r.value == 1.0));
    }

    #[test]
    fn dimension_metadata_is_complete() {
        for d in dim::ALL {
            let _ = unit(d);
            let _ = domain_of(d);
        }
        assert_eq!(domain_of(dim::WORK_END), Domain::Work);
        assert_eq!(domain_of(dim::SLEEP_HOURS), Domain::Home);
        assert_eq!(domain_of(dim::READING_MINUTES), Domain::Shared);
    }
}
