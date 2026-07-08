// lineage.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn session_tree(&self, session_id: SessionId) -> Result<Option<SessionTreeInspection>> {
        let Some(inspection) = self.inspect_session(session_id)? else {
            return Ok(None);
        };
        let active_branch_id = inspection
            .active_branch
            .as_ref()
            .map(|branch| branch.branch_id);
        let branches = inspection.branches.clone();
        let session = inspection.session;
        let related_sessions = inspection.related_sessions;
        let turns = self.turn_history(session_id)?;

        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;

        let branches = branches
            .into_iter()
            .map(|branch| {
                let branch_messages = store.load_messages(session_id, Some(branch.branch_id))?;
                let local_messages =
                    store.load_local_branch_messages(session_id, branch.branch_id)?;
                let branch_turns = turns
                    .iter()
                    .filter(|turn| turn.branch_id == branch.branch_id)
                    .collect::<Vec<_>>();
                let latest_turn = branch_turns.last();

                Ok(BranchInspection {
                    depth: branch_depth(session_id, &store, &branch)?,
                    is_active: Some(branch.branch_id) == active_branch_id,
                    total_message_count: branch_messages.len() as u32,
                    local_message_count: local_messages.len() as u32,
                    turn_count: branch_turns.len() as u32,
                    latest_turn_id: latest_turn.map(|turn| turn.turn_id),
                    latest_event_seq: latest_turn
                        .and_then(|turn| turn.event_seq_end.or(turn.event_seq_start)),
                    latest_message_preview: latest_message_preview(&branch_messages),
                    branch,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        drop(store);

        Ok(Some(SessionTreeInspection {
            session,
            active_branch_id,
            branches,
            related_sessions,
        }))
    }

    pub fn session_lineage(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionLineageInspection>> {
        let Some(focus_session) = self.load_session(session_id)? else {
            return Ok(None);
        };
        let root_session = lineage_root_session(self, focus_session.clone())?;
        let mut nodes = Vec::new();
        collect_lineage_nodes(self, root_session.clone(), 0, session_id, &mut nodes)?;
        Ok(Some(SessionLineageInspection {
            focus_session_id: session_id,
            root_session_id: root_session.session_id,
            nodes,
        }))
    }

    pub fn session_workflow(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionWorkflowInspection>> {
        let Some(focus_session) = self.load_session(session_id)? else {
            return Ok(None);
        };
        let root_session = lineage_root_session(self, focus_session)?;
        let mut nodes = Vec::new();
        let mut runtime_counts = WorkflowRuntimeCounts::default();
        let mut status_counts = WorkflowStatusCounts::default();
        collect_workflow_nodes(
            self,
            root_session.clone(),
            0,
            session_id,
            &mut nodes,
            &mut runtime_counts,
            &mut status_counts,
        )?;
        Ok(Some(SessionWorkflowInspection {
            focus_session_id: session_id,
            root_session_id: root_session.session_id,
            node_count: nodes.len() as u32,
            runtime_counts,
            status_counts,
            nodes,
        }))
    }
}
