//! Career agent (S4-T5) — labeled stub until a career/network source exists (BK-101).
//! Goals and learning plans can already be stated with "remember …" and read via `read_memory`.

use std::sync::Arc;

use super::stub;

pub async fn build() -> anyhow::Result<Arc<dyn adk_core::Agent>> {
    stub::labeled_stub(
        "career_agent",
        "STUB — BK-101",
        "Career goals, skills and opportunities need a career/network source (LinkedIn MCP). Goals stated with \"remember …\" are readable now.",
    )
}
