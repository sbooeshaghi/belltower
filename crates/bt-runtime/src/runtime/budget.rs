// budget.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub(super) fn turn_budget_window(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        projection: &SessionBudgetProjection,
        terminal_at: Option<time::OffsetDateTime>,
    ) -> Result<TurnBudgetWindow> {
        let events = self.all_events(session_id)?;
        let mut window = TurnBudgetWindow {
            base_tokens_used: projection.tokens_used,
            base_turns_used: projection.turns_used,
            base_elapsed_seconds: projection.elapsed_seconds,
            base_cost_used_usd: projection.cost_used_usd,
            anchor_time: projection.updated_at,
            delta_usage: zero_usage(),
            delta_cost_used_usd: None,
            delta_cost_is_unknown: false,
            finished_at: None,
            already_checkpointed: false,
        };
        let mut started_at = None;

        for event in events {
            let event_turn_id = event.turn_id.or_else(|| payload_turn_id(&event.payload));
            if event_turn_id != Some(turn_id) {
                continue;
            }

            match &event.payload {
                EventPayload::TurnStarted { .. } => {
                    started_at = Some(event.occurred_at);
                }
                EventPayload::CompletionFinished { usage, cost, .. } => {
                    window.delta_usage = sum_token_usage(&window.delta_usage, usage);
                    if let Some(cost) = cost {
                        window.delta_cost_used_usd =
                            Some(window.delta_cost_used_usd.unwrap_or(0.0) + cost.total_usd);
                    } else {
                        window.delta_cost_is_unknown = true;
                    }
                }
                EventPayload::TurnFinished { .. } => {
                    window.finished_at = Some(event.occurred_at);
                }
                EventPayload::BudgetCheckpoint {
                    tokens_used,
                    turns_used,
                    elapsed_seconds,
                    cost_used_usd,
                    ..
                } => {
                    window.base_tokens_used = *tokens_used;
                    window.base_turns_used = *turns_used;
                    window.base_elapsed_seconds = *elapsed_seconds;
                    window.base_cost_used_usd = *cost_used_usd;
                    window.anchor_time = event.occurred_at;
                    window.delta_usage = zero_usage();
                    window.delta_cost_used_usd = None;
                    window.delta_cost_is_unknown = false;
                    window.already_checkpointed = true;
                }
                _ => {}
            }
        }

        if !window.already_checkpointed {
            window.anchor_time = started_at.ok_or_else(|| {
                bt_core::BelltowerError::InvalidState(format!(
                    "turn `{turn_id}` has no durable start event for budget checkpointing"
                ))
            })?;
        }
        if window.finished_at.is_none() {
            window.finished_at = terminal_at;
        }
        if window.finished_at.is_none() {
            return Err(bt_core::BelltowerError::InvalidState(format!(
                "turn `{turn_id}` has neither a durable finish nor an active terminal timestamp for budget checkpointing"
            )));
        }

        Ok(window)
    }
}
