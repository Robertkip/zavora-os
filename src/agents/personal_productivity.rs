//! Personal Productivity agent (S5-T4): personal tasks from the shared `tasks` store (S4-T3,
//! James) — home and shared domains, open status, postponement count on the row — plus the
//! `task.*` items a user states in chat (`remember to renew my passport`), whose postponements
//! are counted from hashed task keys in the ledger. Routines come from `routine.*` in memory.
//! The agent's five task tools (`create_task` … `plan_day`) attach through its allowlist entry
//! with `mcp_server = "tasks"` and run behind the permission gate, scoped to the home world.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::domain::Domain;
use crate::intelligence::ledger::{LedgerQuery, LedgerService};
use crate::memory::{Kind, MemoryService, Scope};
use crate::tools::tasks::{store_handle, TaskFilter, TaskStatus};

/// Where a personal task came from.
pub const SOURCE_STORE: &str = "tasks";
pub const SOURCE_MEMORY: &str = "memory";

#[derive(Clone, Debug, Serialize)]
pub struct PersonalTask {
    /// Task id for store-backed tasks; the memory key for chat-stated ones.
    pub key: String,
    pub title: String,
    pub postponed: usize,
    pub known: bool,
    /// Due date for store-backed tasks; chat-stated tasks are undated.
    pub due: Option<DateTime<Utc>>,
    /// [`SOURCE_STORE`] or [`SOURCE_MEMORY`].
    pub source: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PersonalFacts {
    pub tasks: Vec<PersonalTask>,
    pub postponed_total: usize,
    pub routines: Vec<String>,
}

pub async fn facts(memory: &MemoryService, ledger: &LedgerService, user_id: &str) -> PersonalFacts {
    let mut f = PersonalFacts::default();

    // Shared tasks store first: open tasks visible from the home world (home + shared), already
    // sorted due-soonest. The store counts postponements on the row.
    let open = TaskFilter { status: Some(TaskStatus::Open), ..Default::default() };
    for t in store_handle().list(user_id, Domain::Home, &open).await {
        let postponed = usize::try_from(t.postponed_count).unwrap_or(0);
        f.postponed_total += postponed;
        f.tasks.push(PersonalTask { key: t.id.to_string(), title: t.title, postponed, known: true, due: t.due, source: SOURCE_STORE });
    }

    // Then what the user stated in chat: `task.*` and `routine.*` from home memory.
    let items = memory.read(user_id, Scope { domain: Domain::Home }, Some(&["task.".to_string(), "routine.".to_string()])).await;

    // Postponements of chat-stated tasks are ledgered by hashed task key; count per hash.
    let postponed = ledger
        .query(&LedgerQuery { user_id: user_id.into(), kind: Some("task_postponed".into()), ..Default::default() })
        .await;
    let count_for = |key: &str| {
        let h = ledger.hash(key);
        postponed.iter().filter(|e| e.subject_hash.as_deref() == Some(h.as_str())).count()
    };

    for item in items {
        let title = item.value.as_str().map(str::to_string).unwrap_or_else(|| item.value.to_string());
        if item.key.starts_with("task.") {
            let postponed = count_for(&item.key);
            f.postponed_total += postponed;
            f.tasks.push(PersonalTask { key: item.key.clone(), title, postponed, known: item.kind == Kind::Known, due: None, source: SOURCE_MEMORY });
        } else if item.domain == Domain::Home || item.domain == Domain::Shared {
            f.routines.push(title);
        }
    }
    // Most postponed first; ties keep store order (due-soonest), then chat order.
    f.tasks.sort_by_key(|t| std::cmp::Reverse(t.postponed));
    f
}
