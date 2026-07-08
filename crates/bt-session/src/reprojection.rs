use bt_core::{
    CostBreakdown, EventEnvelope, EventPayload, Result, TokenUsage, TurnId,
    TurnInstructionProvenance,
};

#[derive(Clone, Debug, PartialEq)]
pub struct StoredSessionEvent {
    pub seq_id: i64,
    pub event: EventEnvelope,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReprojectionReport {
    pub events_scanned: u64,
    pub completion_events_scanned: u64,
    pub completion_costs_changed: u64,
}

pub trait CompletionCostReprojector {
    fn reproject_completion_cost(
        &mut self,
        provider: &str,
        model: &str,
        usage: &TokenUsage,
        existing_cost: Option<&CostBreakdown>,
    ) -> Result<Option<CostBreakdown>>;
}

impl<F> CompletionCostReprojector for F
where
    F: FnMut(&str, &str, &TokenUsage, Option<&CostBreakdown>) -> Result<Option<CostBreakdown>>,
{
    fn reproject_completion_cost(
        &mut self,
        provider: &str,
        model: &str,
        usage: &TokenUsage,
        existing_cost: Option<&CostBreakdown>,
    ) -> Result<Option<CostBreakdown>> {
        self(provider, model, usage, existing_cost)
    }
}

#[must_use]
pub fn instruction_provenance_for_turn(
    events: &[StoredSessionEvent],
    turn_id: TurnId,
) -> Option<TurnInstructionProvenance> {
    events
        .iter()
        .rev()
        .find_map(|stored| match &stored.event.payload {
            EventPayload::TurnInstructionProvenanceRecorded { provenance }
                if provenance.turn_id == turn_id =>
            {
                Some(provenance.clone())
            }
            _ => None,
        })
}
