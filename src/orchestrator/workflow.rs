//! Shared SSE streaming for multi-card scenario workflows.

use std::convert::Infallible;
use std::sync::Arc;

use adk_core::{Content, SessionId, UserId};
use adk_runner::Runner;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::events::sse::{to_event, FieldEvent};
use crate::orchestrator::{coordinator, persist};
use crate::state::SessionStore;

pub struct AgentSlot {
    pub author: &'static str,
    pub index: usize,
}

pub struct WorkflowStreamConfig {
    pub scenario_key: &'static str,
    pub cards: Vec<serde_json::Value>,
    pub slots: Vec<AgentSlot>,
    pub status_line: fn(&str) -> &'static str,
    pub resolve: fn(&str, &str, &str) -> serde_json::Value,
    pub integrations_note: String,
}

fn text_from_response(response: &serde_json::Value) -> String {
    response
        .get("output")
        .and_then(|o| o.as_str())
        .or_else(|| response.get("text").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_string()
}

fn card_index(author: &str, slots: &[AgentSlot]) -> Option<usize> {
    slots
        .iter()
        .find(|s| s.author == author)
        .map(|s| s.index)
}

pub fn stream_workflow(
    runner: Arc<Runner>,
    user_id: String,
    session_id: String,
    intent: String,
    config: WorkflowStreamConfig,
    sessions: Option<SessionStore>,
    suzy_runner: Option<Arc<Runner>>,
) -> ReceiverStream<Result<axum::response::sse::Event, Infallible>> {
    let (tx, rx) = mpsc::channel(128);
    let scenario_key = config.scenario_key;
    let cards = config.cards;
    let slots = config.slots;
    let integrations = config.integrations_note;

    tokio::spawn(async move {
        let started = chrono::Utc::now();
        let services = crate::permissions::gate::services();
        if let Some(ref store) = sessions {
            store
                .set_scenario(&session_id, scenario_key, Some(&intent))
                .await;
        }

        let _ = tx
            .send(Ok(to_event(&FieldEvent::Scenario {
                key: scenario_key.into(),
                text: intent.clone(),
                total_cards: cards.len(),
            })))
            .await;

        for (index, card) in cards.iter().enumerate() {
            if let Some(ref store) = sessions {
                persist::card_spawn(store, &session_id, index, card.clone()).await;
            }
            let _ = tx
                .send(Ok(to_event(&FieldEvent::CardSpawn {
                    index,
                    card: card.clone(),
                    domain: crate::domain::Domain::for_card(scenario_key, card),
                })))
                .await;
        }

        let prompt = format!(
            "{intent}\n\n[Integrations]\n{integrations}\nPrepare the scenario cards.",
        );
        let content = Content::new("user").with_text(&prompt);

        let uid = match UserId::try_from(user_id.as_str()) {
            Ok(u) => u,
            Err(e) => {
                let _ = tx
                    .send(Ok(to_event(&FieldEvent::Error {
                        message: e.to_string(),
                    })))
                    .await;
                return;
            }
        };
        let sid = match SessionId::try_from(session_id.as_str()) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(Ok(to_event(&FieldEvent::Error {
                        message: e.to_string(),
                    })))
                    .await;
                return;
            }
        };

        crate::agents::ensure_runner_session(&runner, &user_id, &session_id).await;
        let stream = match runner.run(uid, sid, content).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(Ok(to_event(&FieldEvent::Error {
                        message: e.to_string(),
                    })))
                    .await;
                return;
            }
        };

        futures::pin_mut!(stream);
        let card_count = cards.len();
        let mut resolved = vec![false; card_count];
        let mut tool_buf = vec![String::new(); card_count];
        let mut agent_buf = vec![String::new(); card_count];

        while let Some(result) = stream.next().await {
            let event = match result {
                Ok(ev) => ev,
                Err(e) => {
                    let _ = tx
                        .send(Ok(to_event(&FieldEvent::Error {
                            message: e.to_string(),
                        })))
                        .await;
                    break;
                }
            };

            let author = event.author.clone();
            let Some(index) = card_index(&author, &slots) else {
                continue;
            };

            if let Some(content) = event.content() {
                for part in &content.parts {
                    match part {
                        adk_core::Part::FunctionCall { name, .. } => {
                            let _ = tx
                                .send(Ok(to_event(&FieldEvent::CardStatus {
                                    index,
                                    status: "working".into(),
                                    line: Some((config.status_line)(name).into()),
                                })))
                                .await;
                        }
                        adk_core::Part::FunctionResponse { function_response, .. } => {
                            tool_buf[index].push_str(&text_from_response(&function_response.response));
                            tool_buf[index].push('\n');
                        }
                        adk_core::Part::Text { text } => {
                            agent_buf[index].push_str(text);
                            agent_buf[index].push('\n');
                        }
                        _ => {}
                    }
                }
            }

            if event.is_final_response() && !resolved[index] {
                let resolve = (config.resolve)(&author, &tool_buf[index], &agent_buf[index]);
                services.ledger.record(
                    crate::intelligence::ledger::ActivityEvent::new(
                        &user_id,
                        cards
                            .get(index)
                            .map(|c| crate::domain::Domain::for_card(scenario_key, c))
                            .unwrap_or_else(|| crate::domain::Domain::for_scenario(scenario_key)),
                        &author,
                        "card_resolve",
                    )
                    .meta(serde_json::json!({ "scenario": scenario_key, "index": index })),
                );
                if let Some(ref store) = sessions {
                    let card = cards.get(index).cloned().unwrap_or_default();
                    persist::card_resolve(store, &session_id, index, card, resolve.clone(), false)
                        .await;
                }
                let _ = tx
                    .send(Ok(to_event(&FieldEvent::CardResolve { index, resolve })))
                    .await;
                resolved[index] = true;
            }
        }

        for slot in &slots {
            if resolved[slot.index] {
                continue;
            }
            let resolve =
                (config.resolve)(slot.author, &tool_buf[slot.index], &agent_buf[slot.index]);
            if let Some(ref store) = sessions {
                let card = cards.get(slot.index).cloned().unwrap_or_default();
                persist::card_resolve(
                    store,
                    &session_id,
                    slot.index,
                    card,
                    resolve.clone(),
                    false,
                )
                .await;
            }
            let _ = tx
                .send(Ok(to_event(&FieldEvent::CardResolve {
                    index: slot.index,
                    resolve,
                })))
                .await;
        }

        // Anything an agent queued for approval during this run is announced on the same stream.
        for a in services
            .pending
            .list(&user_id, Some(crate::permissions::PendingStatus::Pending), Some(&session_id))
            .await
            .into_iter()
            .filter(|a| a.created_at >= started)
        {
            let _ = tx.send(Ok(to_event(&crate::routes::actions::permission_request_event(&a)))).await;
        }

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        if let Some(ref store) = sessions {
            coordinator::emit_suzy_and_suggest(
                &tx,
                suzy_runner.as_ref(),
                store,
                &session_id,
                &user_id,
                scenario_key,
            )
            .await;
        }
        let _ = tx.send(Ok(to_event(&FieldEvent::Done))).await;
    });

    ReceiverStream::new(rx)
}