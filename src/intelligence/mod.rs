//! Observation & intelligence layer (concept §7–§9).
//!
//! S2 adds the content-free activity ledger ([`ledger`]); S7 adds daily pattern aggregation
//! ([`patterns`]), the personal baseline with drift detection ([`baseline`]) and their Postgres
//! I/O ([`store`]). Balance, behaviour and knowledge (S8–S9) follow on the ambient cron
//! infrastructure. Statistics are deterministic; an LLM only phrases results (S7-T3).

pub mod baseline;
pub mod ledger;
pub mod patterns;
pub mod store;

pub use ledger::{ActivityEvent, LedgerQuery, LedgerService};
