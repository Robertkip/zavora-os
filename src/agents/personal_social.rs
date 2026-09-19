//! Personal Social Media agent (S5-T5) — labeled stub until social MCPs exist (BK-102).

use std::sync::Arc;

use super::stub;

pub async fn build() -> anyhow::Result<Arc<dyn adk_core::Agent>> {
    stub::labeled_stub(
        "personal_social_agent",
        "STUB — BK-102",
        "Instagram, Facebook, TikTok and X need social MCPs. Import an export to enable Observe mode.",
    )
}
