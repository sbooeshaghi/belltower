use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalRequest, ApprovalRequirement, ApprovalScope,
    Result, SessionId, ToolCallId, ToolDisplayGroup, ToolRiskClass, traits::ApprovalEvaluator,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct ApprovalState {
    inner: Mutex<ApprovalStore>,
}

#[derive(Default)]
struct ApprovalStore {
    per_call: HashMap<(SessionId, ToolCallId), CallApproval>,
    per_session: HashMap<(SessionId, String), ApprovalDecision>,
    global: HashMap<String, ApprovalDecision>,
}

#[derive(Clone)]
struct CallApproval {
    request_fingerprint: Option<String>,
    decision: ApprovalDecision,
}

impl ApprovalState {
    fn lock_store(&self) -> Result<std::sync::MutexGuard<'_, ApprovalStore>> {
        self.inner
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("approval lock poisoned".to_owned()))
    }

    fn apply_reusable_decision(
        store: &mut ApprovalStore,
        session_id: SessionId,
        fingerprint: String,
        decision: ApprovalDecision,
    ) {
        match decision_scope(&decision) {
            ApprovalScope::Once => {}
            ApprovalScope::Session => {
                store
                    .per_session
                    .insert((session_id, fingerprint), decision);
            }
            ApprovalScope::Always => {
                store.global.insert(fingerprint, decision);
            }
        }
    }

    pub fn record_call(
        &self,
        session_id: SessionId,
        call_id: ToolCallId,
        decision: ApprovalDecision,
    ) -> Result<()> {
        self.lock_store()?.per_call.insert(
            (session_id, call_id),
            CallApproval {
                request_fingerprint: None,
                decision,
            },
        );
        Ok(())
    }

    pub fn record_for_request(
        &self,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let fingerprint = approval_request_fingerprint(request);
        let mut store = self.lock_store()?;
        store.per_call.insert(
            (request.session_id, request.call_id.clone()),
            CallApproval {
                request_fingerprint: Some(fingerprint.clone()),
                decision: decision.clone(),
            },
        );
        Self::apply_reusable_decision(&mut store, request.session_id, fingerprint, decision);
        Ok(())
    }

    pub fn seed_reusable_decision(
        &self,
        session_id: SessionId,
        fingerprint: String,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let mut store = self.lock_store()?;
        Self::apply_reusable_decision(&mut store, session_id, fingerprint, decision);
        Ok(())
    }

    pub fn get(
        &self,
        session_id: SessionId,
        call_id: &ToolCallId,
    ) -> Result<Option<ApprovalDecision>> {
        Ok(self
            .lock_store()?
            .per_call
            .get(&(session_id, call_id.clone()))
            .map(|approval| approval.decision.clone()))
    }

    pub fn evaluate_request(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
        let fingerprint = approval_request_fingerprint(request);
        let mut store = self.lock_store()?;

        if let Some(existing) = store
            .per_call
            .get(&(request.session_id, request.call_id.clone()))
            .filter(|approval| {
                approval.request_fingerprint.as_deref() == Some(fingerprint.as_str())
            })
            .cloned()
        {
            if matches!(decision_scope(&existing.decision), ApprovalScope::Once) {
                store
                    .per_call
                    .remove(&(request.session_id, request.call_id.clone()));
            }
            return Ok(Some(existing.decision));
        }

        if let Some(decision) = store
            .per_session
            .get(&(request.session_id, fingerprint.clone()))
            .cloned()
        {
            store.per_call.insert(
                (request.session_id, request.call_id.clone()),
                CallApproval {
                    request_fingerprint: Some(fingerprint.clone()),
                    decision: decision.clone(),
                },
            );
            return Ok(Some(decision));
        }

        if let Some(decision) = store.global.get(&fingerprint).cloned() {
            store.per_call.insert(
                (request.session_id, request.call_id.clone()),
                CallApproval {
                    request_fingerprint: Some(fingerprint),
                    decision: decision.clone(),
                },
            );
            return Ok(Some(decision));
        }

        Ok(None)
    }
}

impl ApprovalEvaluator for ApprovalState {
    fn evaluate(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
        self.evaluate_request(request)
    }
}

pub struct PolicyApprovalEvaluator {
    state: Arc<ApprovalState>,
    auto_approve_patterns: Vec<String>,
    /// Sessions running in auto-approval mode: every approval request
    /// resolves as a recorded policy decision instead of waiting on a human.
    /// Process-local by design (fail-safe: a restart drops back to asking);
    /// each resolved decision is still durably recorded in the event log.
    auto_approve_sessions: std::sync::RwLock<std::collections::HashSet<bt_core::SessionId>>,
}

impl PolicyApprovalEvaluator {
    #[must_use]
    pub fn new(state: Arc<ApprovalState>, auto_approve_patterns: Vec<String>) -> Self {
        Self {
            state,
            auto_approve_patterns,
            auto_approve_sessions: std::sync::RwLock::new(std::collections::HashSet::new()),
        }
    }

    pub fn set_session_auto_approval(&self, session_id: bt_core::SessionId, enabled: bool) {
        let mut sessions = self
            .auto_approve_sessions
            .write()
            .expect("auto-approval registry poisoned");
        if enabled {
            sessions.insert(session_id);
        } else {
            sessions.remove(&session_id);
        }
    }

    #[must_use]
    pub fn session_auto_approval(&self, session_id: bt_core::SessionId) -> bool {
        self.auto_approve_sessions
            .read()
            .expect("auto-approval registry poisoned")
            .contains(&session_id)
    }

    fn auto_approve(&self, request: &ApprovalRequest) -> Option<ApprovalDecision> {
        if self.session_auto_approval(request.session_id) {
            return Some(ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "policy:session-auto-approve".to_owned(),
                // Auto mode is process-local and intentionally fail-safe. Record the
                // decision for this call without creating a reusable grant that would
                // silently survive after auto mode is disabled or the process restarts.
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Policy {
                    rule: "session_auto_approve".to_owned(),
                },
            });
        }
        if matches!(request.requirement, ApprovalRequirement::FirstUsePerSession)
            && matches!(
                request.tool_metadata.risk_class,
                ToolRiskClass::Safe | ToolRiskClass::Moderate
            )
            && !matches!(
                request.tool_metadata.display_group,
                ToolDisplayGroup::External
            )
        {
            return Some(ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "policy:first-use-session".to_owned(),
                scope: ApprovalScope::Session,
                source: ApprovalDecisionSource::Policy {
                    rule: "first_use_session".to_owned(),
                },
            });
        }

        if matches!(
            request.tool_metadata.display_group,
            ToolDisplayGroup::Execution
        ) {
            let command = request
                .arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if let Some(pattern) = self
                .auto_approve_patterns
                .iter()
                .find(|pattern| command_matches_pattern(command, pattern))
            {
                return Some(ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "policy:shell-pattern".to_owned(),
                    scope: ApprovalScope::Session,
                    source: ApprovalDecisionSource::Policy {
                        rule: format!("command_pattern:{pattern}"),
                    },
                });
            }
        }

        None
    }
}

impl ApprovalEvaluator for PolicyApprovalEvaluator {
    fn evaluate(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
        if let Some(existing) = self.state.evaluate_request(request)? {
            return Ok(Some(existing));
        }

        if let Some(decision) = self.auto_approve(request) {
            self.state.record_for_request(request, decision.clone())?;
            return Ok(Some(decision));
        }

        Ok(None)
    }
}

pub struct RuntimeApprovalEvaluator {
    state: ApprovalState,
}

impl RuntimeApprovalEvaluator {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: ApprovalState::default(),
        }
    }

    #[must_use]
    pub fn state(&self) -> &ApprovalState {
        &self.state
    }
}

impl Default for RuntimeApprovalEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalEvaluator for RuntimeApprovalEvaluator {
    fn evaluate(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
        self.state.evaluate_request(request)
    }
}

fn command_matches_pattern(command: &str, pattern: &str) -> bool {
    let command = command.trim();
    let pattern = pattern.trim();
    if pattern.is_empty() || contains_shell_control_syntax(command) {
        return false;
    }

    if command == pattern {
        return true;
    }

    command.strip_prefix(pattern).is_some_and(|remainder| {
        remainder
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace())
    })
}

fn contains_shell_control_syntax(command: &str) -> bool {
    command.chars().any(|ch| {
        matches!(
            ch,
            ';' | '&' | '|' | '<' | '>' | '`' | '$' | '\\' | '\n' | '\r'
        )
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{ApprovalState, PolicyApprovalEvaluator};
    use bt_core::{
        ApprovalDecision, ApprovalDecisionSource, ApprovalRequest, ApprovalRequirement,
        ApprovalScope, SessionId, ToolCallId, ToolDisplayGroup, ToolExecutionMode,
        ToolInterruptBehavior, ToolMetadata, ToolRiskClass, traits::ApprovalEvaluator,
    };
    use serde_json::json;
    use std::sync::Arc;

    fn request(
        session_id: SessionId,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ApprovalRequest {
        ApprovalRequest {
            session_id,
            call_id: ToolCallId::new(format!("call-{tool_name}")),
            tool_name: tool_name.to_owned(),
            arguments,
            requirement: if matches!(tool_name, "write" | "edit") {
                ApprovalRequirement::FirstUsePerSession
            } else if tool_name == "shell" {
                ApprovalRequirement::Always
            } else {
                ApprovalRequirement::Never
            },
            tool_metadata: metadata_for(tool_name),
            requested_at: time::OffsetDateTime::now_utc(),
        }
    }

    fn metadata_for(tool_name: &str) -> ToolMetadata {
        match tool_name {
            "write" | "edit" => ToolMetadata {
                risk_class: ToolRiskClass::Moderate,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: Vec::new(),
                display_group: ToolDisplayGroup::Codebase,
            },
            "shell" => ToolMetadata {
                risk_class: ToolRiskClass::High,
                is_read_only: false,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::TerminateProcess,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: Vec::new(),
                display_group: ToolDisplayGroup::Execution,
            },
            _ => ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: Vec::new(),
                display_group: ToolDisplayGroup::Inspection,
            },
        }
    }

    #[test]
    fn write_is_auto_approved() {
        let evaluator =
            PolicyApprovalEvaluator::new(Arc::new(ApprovalState::default()), Vec::new());
        let session_id = SessionId::new();
        let decision = evaluator
            .evaluate(&request(session_id, "write", json!({"path": "note.txt"})))
            .expect("evaluation should succeed");
        assert!(matches!(
            decision,
            Some(ApprovalDecision::Approved {
                scope: ApprovalScope::Session,
                ..
            })
        ));
    }

    #[test]
    fn session_auto_approval_resolves_always_requirements_as_policy() {
        let evaluator =
            PolicyApprovalEvaluator::new(Arc::new(ApprovalState::default()), Vec::new());
        let auto_session = SessionId::new();
        let other_session = SessionId::new();

        let blocked = evaluator
            .evaluate(&request(other_session, "shell", json!({"command": "rm x"})))
            .expect("evaluation should succeed");
        assert!(blocked.is_none(), "non-auto session must still prompt");

        evaluator.set_session_auto_approval(auto_session, true);
        let decision = evaluator
            .evaluate(&request(auto_session, "shell", json!({"command": "rm x"})))
            .expect("evaluation should succeed");
        assert!(matches!(
            decision,
            Some(ApprovalDecision::Approved {
                source: ApprovalDecisionSource::Policy { ref rule },
                scope: ApprovalScope::Once,
                ..
            }) if rule == "session_auto_approve"
        ));

        let still_blocked = evaluator
            .evaluate(&request(other_session, "shell", json!({"command": "rm x"})))
            .expect("evaluation should succeed");
        assert!(
            still_blocked.is_none(),
            "auto mode must stay scoped to its session"
        );

        evaluator.set_session_auto_approval(auto_session, false);
        let mut disabled_request = request(auto_session, "shell", json!({"command": "rm x"}));
        disabled_request.call_id = ToolCallId::new("call-shell-after-auto");
        let disabled = evaluator
            .evaluate(&disabled_request)
            .expect("evaluation should succeed");
        assert!(
            disabled.is_none(),
            "an auto decision must not become a reusable session grant"
        );
    }

    #[test]
    fn matching_shell_pattern_is_auto_approved() {
        let evaluator = PolicyApprovalEvaluator::new(
            Arc::new(ApprovalState::default()),
            vec!["cargo test".to_owned()],
        );
        let session_id = SessionId::new();
        let decision = evaluator
            .evaluate(&request(
                session_id,
                "shell",
                json!({"command": "cargo test --workspace"}),
            ))
            .expect("evaluation should succeed");
        assert!(matches!(
            decision,
            Some(ApprovalDecision::Approved {
                scope: ApprovalScope::Session,
                ..
            })
        ));
    }

    #[test]
    fn shell_pattern_does_not_match_suffix_smuggling() {
        let evaluator = PolicyApprovalEvaluator::new(
            Arc::new(ApprovalState::default()),
            vec!["cargo test".to_owned()],
        );
        let session_id = SessionId::new();
        let decision = evaluator
            .evaluate(&request(
                session_id,
                "shell",
                json!({"command": "cargo testevil"}),
            ))
            .expect("evaluation should succeed");
        assert!(decision.is_none());
    }

    #[test]
    fn shell_pattern_does_not_match_control_operator_smuggling() {
        let evaluator = PolicyApprovalEvaluator::new(
            Arc::new(ApprovalState::default()),
            vec!["cargo test".to_owned()],
        );
        let session_id = SessionId::new();

        for command in [
            "cargo test --workspace && rm -rf /tmp/demo",
            "cargo test --workspace; rm -rf /tmp/demo",
            "cargo test --workspace | sh",
            "cargo test --workspace\nrm -rf /tmp/demo",
            "cargo test $(cat /tmp/hidden-args)",
            "cargo test > /tmp/output",
        ] {
            let decision = evaluator
                .evaluate(&request(session_id, "shell", json!({"command": command})))
                .expect("evaluation should succeed");
            assert!(decision.is_none(), "{command} should not be auto-approved");
        }
    }

    #[test]
    fn unknown_shell_command_stays_pending() {
        let evaluator = PolicyApprovalEvaluator::new(
            Arc::new(ApprovalState::default()),
            vec!["cargo test".to_owned()],
        );
        let session_id = SessionId::new();
        let decision = evaluator
            .evaluate(&request(
                session_id,
                "shell",
                json!({"command": "rm -rf /tmp/demo"}),
            ))
            .expect("evaluation should succeed");
        assert!(decision.is_none());
    }

    #[test]
    fn recorded_request_decision_wins_over_policy() {
        let state = Arc::new(ApprovalState::default());
        let session_id = SessionId::new();
        let approval_request = ApprovalRequest {
            session_id,
            call_id: ToolCallId::new("call-shell"),
            tool_name: "shell".to_owned(),
            arguments: json!({"command": "cargo test --workspace"}),
            requirement: ApprovalRequirement::Always,
            tool_metadata: metadata_for("shell"),
            requested_at: time::OffsetDateTime::now_utc(),
        };
        state
            .record_for_request(
                &approval_request,
                ApprovalDecision::Denied {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "user".to_owned(),
                    reason: Some("no".to_owned()),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            )
            .expect("record decision");
        let evaluator = PolicyApprovalEvaluator::new(state, vec!["cargo test".to_owned()]);
        let decision = evaluator
            .evaluate(&approval_request)
            .expect("evaluation should succeed");
        assert!(matches!(
            decision,
            Some(ApprovalDecision::Denied {
                scope: ApprovalScope::Once,
                ..
            })
        ));
    }

    #[test]
    fn once_decision_for_reused_call_id_does_not_cross_sessions() {
        let state = ApprovalState::default();
        let first_session = SessionId::new();
        let second_session = SessionId::new();
        let first = request(first_session, "shell", json!({"command":"pwd"}));
        state
            .record_for_request(
                &first,
                ApprovalDecision::Denied {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "user".to_owned(),
                    reason: Some("not in this session".to_owned()),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            )
            .expect("record decision");

        let second = ApprovalRequest {
            session_id: second_session,
            call_id: first.call_id,
            tool_name: first.tool_name,
            arguments: first.arguments,
            requirement: first.requirement,
            tool_metadata: first.tool_metadata,
            requested_at: time::OffsetDateTime::now_utc(),
        };
        assert!(state.evaluate(&second).expect("evaluate").is_none());
    }

    #[test]
    fn once_decision_for_reused_call_id_does_not_cross_request_fingerprints() {
        let state = ApprovalState::default();
        let session_id = SessionId::new();
        let first = request(session_id, "shell", json!({"command":"pwd"}));
        state
            .record_for_request(
                &first,
                ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "user".to_owned(),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            )
            .expect("record decision");

        let second = ApprovalRequest {
            session_id,
            call_id: first.call_id,
            tool_name: first.tool_name,
            arguments: json!({"command":"rm example.txt"}),
            requirement: first.requirement,
            tool_metadata: first.tool_metadata,
            requested_at: time::OffsetDateTime::now_utc(),
        };
        assert!(state.evaluate(&second).expect("evaluate").is_none());
    }

    #[test]
    fn once_decision_is_consumed_by_the_resumed_request() {
        let state = ApprovalState::default();
        let session_id = SessionId::new();
        let request = request(session_id, "shell", json!({"command":"pwd"}));
        state
            .record_for_request(
                &request,
                ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "user".to_owned(),
                    scope: ApprovalScope::Once,
                    source: ApprovalDecisionSource::Human,
                },
            )
            .expect("record decision");

        assert!(matches!(
            state.evaluate(&request).expect("first evaluation"),
            Some(ApprovalDecision::Approved {
                scope: ApprovalScope::Once,
                ..
            })
        ));
        assert!(
            state
                .evaluate(&request)
                .expect("second evaluation")
                .is_none()
        );
    }

    #[test]
    fn session_scoped_approval_reuses_matching_request_in_same_session() {
        let state = ApprovalState::default();
        let session_id = SessionId::new();
        let first = request(session_id, "shell", json!({"command":"pwd"}));
        state
            .record_for_request(
                &first,
                ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "user".to_owned(),
                    scope: ApprovalScope::Session,
                    source: ApprovalDecisionSource::Human,
                },
            )
            .expect("record session approval");

        let next = ApprovalRequest {
            session_id,
            call_id: ToolCallId::new("call-2"),
            tool_name: "shell".to_owned(),
            arguments: json!({"command":"pwd","call_id":"ignored"}),
            requirement: ApprovalRequirement::Always,
            tool_metadata: metadata_for("shell"),
            requested_at: time::OffsetDateTime::now_utc(),
        };
        let decision = state.evaluate(&next).expect("evaluate");
        assert!(matches!(
            decision,
            Some(ApprovalDecision::Approved {
                scope: ApprovalScope::Session,
                ..
            })
        ));
    }

    #[test]
    fn session_scoped_approval_does_not_cross_sessions() {
        let state = ApprovalState::default();
        let first_session = SessionId::new();
        let second_session = SessionId::new();
        let first = request(first_session, "shell", json!({"command":"pwd"}));
        state
            .record_for_request(
                &first,
                ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "user".to_owned(),
                    scope: ApprovalScope::Session,
                    source: ApprovalDecisionSource::Human,
                },
            )
            .expect("record session approval");

        let next = ApprovalRequest {
            session_id: second_session,
            call_id: ToolCallId::new("call-2"),
            tool_name: "shell".to_owned(),
            arguments: json!({"command":"pwd"}),
            requirement: ApprovalRequirement::Always,
            tool_metadata: metadata_for("shell"),
            requested_at: time::OffsetDateTime::now_utc(),
        };
        assert!(state.evaluate(&next).expect("evaluate").is_none());
    }
}

fn decision_scope(decision: &ApprovalDecision) -> ApprovalScope {
    match decision {
        ApprovalDecision::Approved { scope, .. } | ApprovalDecision::Denied { scope, .. } => *scope,
    }
}

pub(crate) fn approval_request_fingerprint(request: &ApprovalRequest) -> String {
    request.fingerprint()
}
