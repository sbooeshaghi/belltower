// lifecycle.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn open(
        config: BelltowerConfig,
        database_path: impl AsRef<std::path::Path>,
    ) -> Result<Self> {
        let mut trace = StartupTrace::from_env("bt-runtime");
        trace.mark("runtime.open.start");
        trace.mark("store.open.start");
        let store = SqliteSessionStore::open(database_path)?;
        trace.mark("store.open.done");
        let (event_bus, _) = broadcast::channel(1024);
        let global_skills = bt_core::config_dir().join("skills");
        let prompt_assets = bt_core::data_dir().join("prompts");
        let approvals = Arc::new(ApprovalState::default());
        trace.mark("approval.rehydrate.start");
        rehydrate_approval_state(&store, &approvals)?;
        trace.mark("approval.rehydrate.done");
        let approval_patterns = config.approval.auto_approve_patterns.clone();
        trace.mark("registries.construct.start");
        let runtime = Self {
            connections: ConnectionRegistry::from_config(&config),
            mcp: McpRegistry::from_config(&config),
            models: LocalModelManager::from_config(&config)?,
            instructions: MarkdownInstructionResolver::new(global_skills, prompt_assets)?,
            config,
            store: Mutex::new(store),
            #[cfg(feature = "test-support")]
            store_append_fault: Mutex::new(None),
            approval_evaluator: Arc::new(PolicyApprovalEvaluator::new(
                approvals.clone(),
                approval_patterns,
            )),
            approvals,
            event_bus,
        };
        trace.mark("registries.construct.done");
        trace.mark("turn_recovery.start");
        runtime.recover_interrupted_turns()?;
        trace.mark("turn_recovery.done");
        trace.mark("runtime.open.done");
        Ok(runtime)
    }

    fn recover_interrupted_turns(&self) -> Result<()> {
        let interrupted_turns = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_unfinished_turns()?;

        for turn in interrupted_turns {
            let (message, finish_reason) = match turn.source {
                TurnStartSource::ApprovalResume | TurnStartSource::InputResume => (
                    "resumed turn interrupted before completion; recovered as failed during runtime startup",
                    "interrupted_after_resume",
                ),
                _ => (
                    "turn interrupted before completion; recovered as failed during runtime startup",
                    "interrupted_after_restart",
                ),
            };
            let error = bt_core::BelltowerError::Runtime(message.to_owned());
            self.append_events(vec![
                EventEnvelope::new(
                    turn.session_id,
                    turn.branch_id,
                    SpanKind::Agent,
                    EventPayload::SessionError {
                        class: error.class(),
                        code: error.code().to_owned(),
                        message: error.to_string(),
                        retryable: error.retryable(),
                    },
                )
                .with_attribute(
                    "error.class",
                    serde_json::Value::String(error.class().to_string()),
                )
                .with_attribute(
                    "error.code",
                    serde_json::Value::String(error.code().to_owned()),
                )
                .with_attribute(
                    "error.retryable",
                    serde_json::Value::Bool(error.retryable()),
                )
                .with_turn_id(turn.turn_id),
                EventEnvelope::new(
                    turn.session_id,
                    turn.branch_id,
                    SpanKind::Agent,
                    EventPayload::TurnFinished {
                        turn_id: turn.turn_id,
                        provider: turn.provider.clone(),
                        model: turn.model.clone(),
                        status: "failed".to_owned(),
                        finish_reason: Some(finish_reason.to_owned()),
                        latency_ms: 0,
                    },
                )
                .with_turn_id(turn.turn_id)
                .with_attribute(
                    bt_core::oi_attrs::LLM_PROVIDER,
                    serde_json::Value::String(turn.provider),
                )
                .with_attribute(
                    bt_core::oi_attrs::LLM_MODEL_NAME,
                    serde_json::Value::String(turn.model),
                )
                .with_attribute(
                    "turn.status",
                    serde_json::Value::String("failed".to_owned()),
                ),
            ])?;
        }

        Ok(())
    }
}
