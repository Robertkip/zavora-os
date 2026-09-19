//! Professional Social Media agent (S4-T6) — labeled stub until a LinkedIn MCP exists (BK-101).

use std::sync::Arc;

use super::stub;

pub async fn build() -> anyhow::Result<Arc<dyn adk_core::Agent>> {
    stub::labeled_stub(
        "professional_social_agent",
        "STUB — BK-101",
        "Mentions, posts and networking opportunities need a LinkedIn MCP. Import a LinkedIn export to enable Observe mode.",
    )
}
