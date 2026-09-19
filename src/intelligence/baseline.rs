//! S7-T2 — Personal Baseline: robust statistics per dimension and day class over a rolling
//! window, a warm-up period, drift detection against two recent windows, the "meaningful" rule
//! and a per-dimension cooldown (concept §8.2). Pure: no clock, no I/O — `as_of` is an argument.
//!
//! Windows, for `as_of` = today:
//!
//! ```text
//!  |<---------- baseline: 28 days ---------->|<------ recent: 14 days ------>|
//!  |                                         |            |<-- short: 7 -->|
//!  base_start                          base_end                          as_of
//! ```
//!
//! The baseline window ends the day before the recent window starts, so a change in the
//! recent weeks cannot pull its own baseline toward itself. A dimension has drifted when both
//! recent medians (14 d and 7 d) sit beyond `max(k·MAD, floor)` from the baseline median on the
//! same side. Warm-up: no observation until the baseline window holds `warmup_days` days of data.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

use super::patterns::{self, dim, DailyValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DayClass {
    Weekday,
    Weekend,
}

impl DayClass {
    pub fn of(day: NaiveDate) -> Self {
        match day.weekday() {
            Weekday::Sat | Weekday::Sun => DayClass::Weekend,
            _ => DayClass::Weekday,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            DayClass::Weekday => "weekday",
            DayClass::Weekend => "weekend",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "weekday" => Some(DayClass::Weekday),
            "weekend" => Some(DayClass::Weekend),
            _ => None,
        }
    }
    fn phrase(&self) -> &'static str {
        match self {
            DayClass::Weekday => "on weekdays",
            DayClass::Weekend => "at weekends",
        }
    }
}

/// Learning-model parameters (concept §8.2). Defaults are the spec's; floors were calibrated on
/// `scripts/synth_ledger.py --weeks 6 --drift` (work end shifts ~1.7 h against a MAD of ~0.2 h).
#[derive(Clone, Debug)]
pub struct Config {
    pub window_days: i64,
    pub warmup_days: usize,
    pub recent_days: i64,
    pub short_days: i64,
    /// Deviation multiplier on the MAD.
    pub k: f64,
    pub cooldown_days: i64,
    /// "Meaningful" needs at least this many distinct drifting dimensions …
    pub min_dimensions: usize,
    /// … or one dimension beyond threshold on at least this many days of the recent window.
    pub single_dimension_min_days: usize,
    pub min_baseline_samples: usize,
    pub min_recent_samples: usize,
    pub min_short_samples: usize,
    /// Dimensions the user switched off (S7-T8).
    pub disabled: BTreeSet<String>,
    /// Per-dimension absolute floors for the deviation threshold, in the dimension's unit.
    pub floors: BTreeMap<String, f64>,
}

impl Default for Config {
    fn default() -> Self {
        let floors = [
            (dim::WORK_START, 0.75),
            (dim::WORK_END, 0.75),
            (dim::WORK_MINUTES, 45.0),
            (dim::READING_MINUTES, 10.0),
            (dim::READING_SESSIONS, 1.0),
            (dim::COMMS_WORK_VOLUME, 3.0),
            (dim::COMMS_FAMILY_VOLUME, 2.0),
            (dim::TASKS_POSTPONED, 1.0),
            (dim::EXERCISE_SESSIONS, 1.0),
            (dim::SLEEP_HOURS, 0.5),
        ]
        .into_iter()
        .map(|(d, f)| (d.to_string(), f))
        .collect();
        Self {
            window_days: 28,
            warmup_days: 14,
            recent_days: 14,
            short_days: 7,
            k: 2.5,
            cooldown_days: 7,
            min_dimensions: 2,
            single_dimension_min_days: 10,
            min_baseline_samples: 4,
            min_recent_samples: 3,
            min_short_samples: 2,
            disabled: BTreeSet::new(),
            floors,
        }
    }
}

impl Config {
    pub fn floor(&self, dimension: &str) -> f64 {
        self.floors.get(dimension).copied().unwrap_or(0.0)
    }
}

/// One row of `baselines`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub user_id: String,
    pub dimension: String,
    pub day_class: DayClass,
    pub median: f64,
    pub mad: f64,
    pub sample_n: usize,
    pub window_end: NaiveDate,
    pub confirmed: bool,
}

/// A dimension whose recent medians sit beyond the threshold. Numbers only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DimensionDrift {
    pub dimension: String,
    pub day_class: DayClass,
    pub baseline_median: f64,
    pub recent_median: f64,
    pub short_median: f64,
    pub delta: f64,
    pub threshold: f64,
    /// Days of the recent window whose value is beyond the threshold.
    pub days_beyond: usize,
    pub unit: String,
}

/// One row of `observations` with `kind = "drift"` (concept §8.3). `text` follows the neutral
/// language rules (§7.6): number, period, baseline reference; no verdict, no cause, no advice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftObservation {
    pub user_id: String,
    pub kind: String,
    pub dimensions: Vec<String>,
    pub window_days: i64,
    pub facts: serde_json::Value,
    pub text: String,
    pub offer: String,
    pub as_of: NaiveDate,
}

/// What the cooldown needs to know about earlier observations.
#[derive(Clone, Debug, PartialEq)]
pub struct ExistingObservation {
    pub dimensions: Vec<String>,
    pub created_on: NaiveDate,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WarmupStatus {
    pub days_seen: usize,
    pub days_needed: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub baselines: Vec<Baseline>,
    /// `Some` while still learning; then no drift is evaluated.
    pub warmup: Option<WarmupStatus>,
    /// Every dimension beyond threshold, before cooldown.
    pub drifts: Vec<DimensionDrift>,
    /// Dimensions suppressed by the cooldown.
    pub cooled_down: Vec<String>,
    /// At most one observation per run (the meaningful rule groups dimensions).
    pub observation: Option<DriftObservation>,
}

/// Median of `values` (sorted in place). `None` when empty.
pub fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = values.len();
    Some(if n % 2 == 1 { values[n / 2] } else { (values[n / 2 - 1] + values[n / 2]) / 2.0 })
}

/// Median absolute deviation around `center`.
pub fn mad(values: &[f64], center: f64) -> f64 {
    let mut dev: Vec<f64> = values.iter().map(|v| (v - center).abs()).collect();
    median(&mut dev).unwrap_or(0.0)
}

/// Compute baselines and drift for one user as of `as_of`.
pub fn evaluate(user_id: &str, daily: &[DailyValue], as_of: NaiveDate, existing: &[ExistingObservation], cfg: &Config) -> Outcome {
    let recent_start = as_of - Duration::days(cfg.recent_days - 1);
    let short_start = as_of - Duration::days(cfg.short_days - 1);
    let base_end = recent_start - Duration::days(1);
    let base_start = base_end - Duration::days(cfg.window_days - 1);

    let rows: Vec<&DailyValue> =
        daily.iter().filter(|d| d.user_id == user_id && !cfg.disabled.contains(&d.dimension)).collect();

    let mut out = Outcome::default();
    let base_days: BTreeSet<NaiveDate> =
        rows.iter().filter(|d| d.day >= base_start && d.day <= base_end).map(|d| d.day).collect();
    if base_days.len() < cfg.warmup_days {
        out.warmup = Some(WarmupStatus { days_seen: base_days.len(), days_needed: cfg.warmup_days });
        return out;
    }

    type Key = (String, DayClass);
    let mut base: BTreeMap<Key, Vec<f64>> = BTreeMap::new();
    let mut recent: BTreeMap<Key, Vec<f64>> = BTreeMap::new();
    let mut short: BTreeMap<Key, Vec<f64>> = BTreeMap::new();
    for d in rows {
        let key = (d.dimension.clone(), DayClass::of(d.day));
        if d.day >= base_start && d.day <= base_end {
            base.entry(key).or_default().push(d.value);
        } else if d.day >= recent_start && d.day <= as_of {
            recent.entry(key.clone()).or_default().push(d.value);
            if d.day >= short_start {
                short.entry(key).or_default().push(d.value);
            }
        }
    }

    for ((dimension, day_class), mut values) in base {
        if values.len() < cfg.min_baseline_samples {
            continue;
        }
        let med = median(&mut values).expect("non-empty");
        out.baselines.push(Baseline {
            user_id: user_id.to_string(),
            dimension,
            day_class,
            median: med,
            mad: mad(&values, med),
            sample_n: values.len(),
            window_end: base_end,
            confirmed: false,
        });
    }

    for b in &out.baselines {
        let key = (b.dimension.clone(), b.day_class);
        let (Some(rv), Some(sv)) = (recent.get(&key), short.get(&key)) else { continue };
        if rv.len() < cfg.min_recent_samples || sv.len() < cfg.min_short_samples {
            continue;
        }
        let recent_median = median(&mut rv.clone()).expect("non-empty");
        let short_median = median(&mut sv.clone()).expect("non-empty");
        let threshold = (cfg.k * b.mad).max(cfg.floor(&b.dimension));
        let delta = recent_median - b.median;
        let short_delta = short_median - b.median;
        let same_side = delta.signum() == short_delta.signum();
        if delta.abs() > threshold && short_delta.abs() > threshold && same_side {
            out.drifts.push(DimensionDrift {
                dimension: b.dimension.clone(),
                day_class: b.day_class,
                baseline_median: b.median,
                recent_median,
                short_median,
                delta,
                threshold,
                days_beyond: rv.iter().filter(|v| (*v - b.median).abs() > threshold).count(),
                unit: patterns::unit(&b.dimension).to_string(),
            });
        }
    }

    let cooldown_since = as_of - Duration::days(cfg.cooldown_days);
    let cooled: BTreeSet<&str> = existing
        .iter()
        .filter(|o| o.created_on > cooldown_since)
        .flat_map(|o| o.dimensions.iter().map(String::as_str))
        .collect();
    let (active, suppressed): (Vec<&DimensionDrift>, Vec<&DimensionDrift>) =
        out.drifts.iter().partition(|d| !cooled.contains(d.dimension.as_str()));
    out.cooled_down = suppressed.iter().map(|d| d.dimension.clone()).collect::<BTreeSet<_>>().into_iter().collect();

    let dimensions: BTreeSet<&str> = active.iter().map(|d| d.dimension.as_str()).collect();
    let meaningful = dimensions.len() >= cfg.min_dimensions
        || active.iter().any(|d| d.days_beyond >= cfg.single_dimension_min_days);
    if meaningful {
        out.observation = Some(compose(user_id, &active, as_of, cfg));
    }
    out
}

fn compose(user_id: &str, drifts: &[&DimensionDrift], as_of: NaiveDate, cfg: &Config) -> DriftObservation {
    // One sentence per dimension, weekday class first, at most three.
    let mut seen = BTreeSet::new();
    let mut ordered: Vec<&DimensionDrift> = drifts.to_vec();
    ordered.sort_by_key(|d| (d.day_class, d.dimension.clone()));
    let sentences: Vec<String> = ordered
        .iter()
        .filter(|d| seen.insert(d.dimension.clone()))
        .take(3)
        .map(|d| phrase(d))
        .collect();
    let period = period_phrase(cfg.recent_days);
    let mut text = format!("Over the last {period}, ");
    for (i, s) in sentences.iter().enumerate() {
        if i == 0 {
            text.push_str(s);
        } else {
            text.push_str(". ");
            text.push_str(&capitalize(s));
        }
    }
    text.push('.');
    let dimensions: Vec<String> = seen.into_iter().collect();
    let facts = serde_json::json!({
        "window_days": cfg.recent_days,
        "short_window_days": cfg.short_days,
        "baseline_days": cfg.window_days,
        "as_of": as_of,
        "dimensions": drifts,
    });
    DriftObservation {
        user_id: user_id.to_string(),
        kind: "drift".to_string(),
        dimensions,
        window_days: cfg.recent_days,
        facts,
        text,
        offer: "Would you like me to help you review what's changed?".to_string(),
        as_of,
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn period_phrase(days: i64) -> String {
    match days {
        7 => "week".to_string(),
        14 => "two weeks".to_string(),
        n if n % 7 == 0 => format!("{} weeks", n / 7),
        n => format!("{n} days"),
    }
}

/// Fractional hours → `HH:MM`.
pub fn clock(hours: f64) -> String {
    let total = (hours * 60.0).round() as i64;
    format!("{:02}:{:02}", (total / 60).rem_euclid(24), total.rem_euclid(60))
}

/// Neutral statement of one drifted dimension: the number, the class, the baseline. No verdict.
fn phrase(d: &DimensionDrift) -> String {
    let when = d.day_class.phrase();
    let (r, b) = (d.recent_median, d.baseline_median);
    match d.dimension.as_str() {
        dim::WORK_END => format!("your work day ended around {} {when}; your usual is {}", clock(r), clock(b)),
        dim::WORK_START => format!("your work day started around {} {when}; your usual is {}", clock(r), clock(b)),
        dim::WORK_MINUTES => format!("you worked about {:.1} hours a day {when}; your usual is {:.1}", r / 60.0, b / 60.0),
        dim::READING_MINUTES => format!("you read about {r:.0} minutes a day {when}; your usual is {b:.0}"),
        dim::READING_SESSIONS => format!("you read {r:.1} times a day {when}; your usual is {b:.1}"),
        dim::COMMS_FAMILY_VOLUME => format!("you exchanged about {r:.1} family messages a day {when}; your usual is {b:.1}"),
        dim::COMMS_WORK_VOLUME => format!("you handled about {r:.1} work messages a day {when}; your usual is {b:.1}"),
        dim::TASKS_POSTPONED => format!("you postponed about {r:.1} personal tasks a day {when}; your usual is {b:.1}"),
        dim::EXERCISE_SESSIONS => format!("you exercised {r:.1} times a day {when}; your usual is {b:.1}"),
        dim::SLEEP_HOURS => format!("you slept about {r:.1} hours {when}; your usual is {b:.1}"),
        other => format!("{other} was {r:.1} {when}; your usual is {b:.1}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn median_and_mad_are_robust() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&mut []), None);
        let mut v = vec![17.5, 17.4, 17.6, 17.5, 23.0]; // one late night
        let m = median(&mut v).unwrap();
        assert_eq!(m, 17.5);
        assert!(mad(&v, m) < 0.2);
    }

    #[test]
    fn day_class_and_clock_format() {
        assert_eq!(DayClass::of(day("2026-09-19")), DayClass::Weekend); // Saturday
        assert_eq!(DayClass::of(day("2026-09-14")), DayClass::Weekday);
        assert_eq!(clock(17.5), "17:30");
        assert_eq!(clock(19.283), "19:17");
        assert_eq!(clock(23.999), "00:00");
    }

    fn series(dimension: &str, from: &str, days: i64, weekday_value: impl Fn(i64) -> f64) -> Vec<DailyValue> {
        let start = day(from);
        (0..days)
            .map(|i| start + Duration::days(i))
            .filter(|d| DayClass::of(*d) == DayClass::Weekday)
            .enumerate()
            .map(|(i, d)| DailyValue { user_id: "u".into(), day: d, dimension: dimension.into(), value: weekday_value(i as i64), sample_n: 1 })
            .collect()
    }

    #[test]
    fn warmup_blocks_until_fourteen_baseline_days() {
        let daily = series(dim::WORK_END, "2026-09-01", 20, |_| 17.5);
        let out = evaluate("u", &daily, day("2026-09-20"), &[], &Config::default());
        assert!(out.warmup.is_some());
        assert!(out.observation.is_none());
    }

    #[test]
    fn two_shifted_dimensions_make_one_observation_and_cooldown_suppresses_it() {
        // 28 baseline weekdays at 17:30, then 14 recent weekdays at 19:15; reading 30 → 8.
        let mut daily = series(dim::WORK_END, "2026-08-03", 28, |_| 17.5);
        daily.extend(series(dim::WORK_END, "2026-08-31", 14, |_| 19.25));
        daily.extend(series(dim::READING_MINUTES, "2026-08-03", 28, |i| 30.0 + (i % 3) as f64));
        daily.extend(series(dim::READING_MINUTES, "2026-08-31", 14, |_| 8.0));
        let as_of = day("2026-09-13");
        let out = evaluate("u", &daily, as_of, &[], &Config::default());
        assert!(out.warmup.is_none());
        assert_eq!(out.drifts.len(), 2);
        let obs = out.observation.expect("meaningful");
        assert_eq!(obs.dimensions, vec![dim::READING_MINUTES.to_string(), dim::WORK_END.to_string()]);
        assert!(obs.text.contains("two weeks") && obs.text.contains("19:15") && obs.text.contains("your usual is 17:30"), "{}", obs.text);

        let recent = [ExistingObservation { dimensions: obs.dimensions.clone(), created_on: as_of - Duration::days(2) }];
        let again = evaluate("u", &daily, as_of, &recent, &Config::default());
        assert!(again.observation.is_none());
        assert_eq!(again.cooled_down.len(), 2);

        let old = [ExistingObservation { dimensions: obs.dimensions.clone(), created_on: as_of - Duration::days(8) }];
        assert!(evaluate("u", &daily, as_of, &old, &Config::default()).observation.is_some());
    }

    #[test]
    fn a_single_busy_week_does_not_count_as_drift() {
        // Recent 14 days: first week late, second week back to normal → 7-day window disagrees.
        let mut daily = series(dim::WORK_END, "2026-08-03", 28, |_| 17.5);
        daily.extend(series(dim::WORK_END, "2026-08-31", 7, |_| 19.5));
        daily.extend(series(dim::WORK_END, "2026-09-07", 7, |_| 17.5));
        let out = evaluate("u", &daily, day("2026-09-13"), &[], &Config::default());
        assert!(out.drifts.is_empty(), "{:?}", out.drifts);
    }
}
