// context.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;
use std::time::Instant;

fn related_message_context_text(message: &RelatedSessionMessage) -> String {
    format!(
        "[related-session message from {} ({:?})]\n{}",
        message.source_session_id, message.kind, message.text
    )
}

/// Observed-context input for a prepare call: compute from the store now, or
/// reuse a value already computed by the plan phase so the trigger decision
/// stays consistent across plan -> summarize -> build (the recorded
/// summarization completion must not shift the observation).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum ObservedContextTokensInput {
    #[default]
    Compute,
    Provided(Option<u64>),
}

/// Compaction inputs threaded from the orchestrator preflight into the final
/// context build.
#[derive(Clone, Debug, Default)]
pub(crate) struct TurnContextCompactionInputs {
    pub(crate) observed_context_tokens: ObservedContextTokensInput,
    /// Model-written summary body from the recorded summarization
    /// completion; `None` falls back to the deterministic digest.
    pub(crate) summary_override: Option<String>,
    /// Appended to the recorded compaction reason (summarizer provenance or
    /// fallback cause).
    pub(crate) reason_suffix: Option<String>,
}

/// Plan-phase output for one preflight: the frozen observation plus the
/// summarization work (when the trigger fires and there is content to
/// summarize).
#[derive(Clone, Debug)]
pub(crate) struct TurnContextPlan {
    pub(crate) observed_context_tokens: Option<u64>,
    pub(crate) summarization: Option<TurnContextSummarizationPlan>,
}

#[derive(Clone, Debug)]
pub(crate) struct TurnContextSummarizationPlan {
    pub(crate) transcript: String,
    pub(crate) summary_budget_tokens: u64,
    pub(crate) summarizer_model: String,
}

/// Chain state of the most recent `context.compacted` event on a branch.
struct PreviousCompactionState {
    /// Number of `context.compacted` events already on the branch.
    count: u64,
    last_compaction_id: bt_core::CompactionId,
    first_compaction_id: bt_core::CompactionId,
    summary: String,
}

impl BelltowerRuntime {
    pub(crate) fn context_message_sources_for_request(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        request: &bt_core::CompletionRequest,
    ) -> Result<(Option<i64>, Vec<bt_core::ContextMessageSourceRef>)> {
        let context_messages =
            self.load_context_messages_with_related_sessions(session_id, branch_id)?;
        let context_boundary_seq_id = context_messages
            .iter()
            .filter_map(|record| record.source_seq_id)
            .max();
        let message_sources = context_messages
            .iter()
            .filter(|record| {
                request
                    .messages
                    .iter()
                    .any(|message| message.message_id == record.message.message_id)
            })
            .filter_map(|record| {
                Some(bt_core::ContextMessageSourceRef {
                    message_id: record.message.message_id,
                    branch_id: record.source_branch_id?,
                    seq_id: record.source_seq_id?,
                })
            })
            .collect();
        Ok((context_boundary_seq_id, message_sources))
    }

    pub fn messages(
        &self,
        session_id: SessionId,
        branch_id: Option<bt_core::BranchId>,
    ) -> Result<Vec<Message>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_messages(session_id, branch_id)
    }

    pub fn prepare_turn_context(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        turn_id: Option<TurnId>,
        settings_revision_id: u64,
    ) -> Result<PreparedTurnContext> {
        self.prepare_turn_context_with_options(
            session,
            branch,
            system_prompt,
            tools,
            thinking,
            turn_id,
            settings_revision_id,
            false,
        )
    }

    pub fn compact_branch_context(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
    ) -> Result<PreparedTurnContext> {
        let settings_revision_id = self.current_settings_revision(session.session_id)?;
        self.prepare_turn_context_with_options(
            session,
            branch,
            system_prompt,
            tools,
            thinking,
            None,
            settings_revision_id,
            true,
        )
    }

    pub fn local_branch_messages(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Vec<Message>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_local_branch_messages(session_id, branch_id)
    }

    pub fn resolve_instructions(
        &self,
        project_root: Option<&camino::Utf8Path>,
    ) -> Result<Vec<bt_core::InstructionDocument>> {
        self.instructions.resolve(project_root)
    }

    #[must_use]
    pub fn core_prompt_document(&self) -> bt_core::InstructionDocument {
        self.instructions.core_prompt()
    }

    #[must_use]
    pub fn provider_prompt_overlay(
        &self,
        provider_family: &str,
    ) -> Option<bt_core::InstructionDocument> {
        self.instructions.provider_overlay(provider_family)
    }

    pub(super) fn prepare_turn_context_with_options(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        turn_id: Option<TurnId>,
        settings_revision_id: u64,
        force_compaction: bool,
    ) -> Result<PreparedTurnContext> {
        let (connection, model_id) =
            self.resolve_turn_settings(session.session_id, settings_revision_id)?;
        self.prepare_turn_context_for_resolved_settings(
            session,
            branch,
            connection,
            model_id,
            system_prompt,
            tools,
            thinking,
            turn_id,
            settings_revision_id,
            force_compaction,
            TurnContextCompactionInputs::default(),
        )
    }

    /// Provider-observed context size for the branch: the usage total of the
    /// last real (non-maintenance) `completion.finished` on this branch plus
    /// an estimate of the messages appended after it. `None` when no prior
    /// real completion exists.
    fn observed_context_tokens(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        context_messages: &[ContextMessageRecord],
    ) -> Result<Option<u64>> {
        let finished = self.events_of_kind(session_id, "completion.finished", Some(branch_id))?;
        let last = finished
            .iter()
            .rev()
            .find_map(|event| match &event.payload {
                EventPayload::CompletionFinished {
                    llm_call_ordinal,
                    usage,
                    ..
                } if *llm_call_ordinal != bt_core::CONTEXT_MAINTENANCE_LLM_CALL_ORDINAL => {
                    Some((event.seq_id, usage.total_tokens))
                }
                _ => None,
            });
        let Some((boundary_seq_id, base_tokens)) = last else {
            return Ok(None);
        };
        let Some(boundary_seq_id) = boundary_seq_id else {
            return Ok(Some(base_tokens));
        };
        let appended_tokens: u64 = context_messages
            .iter()
            .filter(|record| {
                record
                    .source_seq_id
                    .is_some_and(|seq_id| seq_id > boundary_seq_id)
            })
            .map(|record| {
                bt_context::estimate_messages_tokens(std::slice::from_ref(&record.message))
            })
            .sum();
        Ok(Some(base_tokens.saturating_add(appended_tokens)))
    }

    /// Chain state from the most recent `context.compacted` event on the
    /// branch (kind-indexed lookup, never a full log replay).
    fn previous_compaction_state(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Option<PreviousCompactionState>> {
        let events = self.events_of_kind(session_id, "context.compacted", Some(branch_id))?;
        let mut compactions = events.iter().filter_map(|event| match &event.payload {
            EventPayload::ContextCompacted {
                compaction_id,
                first_compaction_id,
                summary,
                ..
            } => Some((*compaction_id, *first_compaction_id, summary.clone())),
            _ => None,
        });
        let Some(first) = compactions.next() else {
            return Ok(None);
        };
        let count = 1 + compactions.clone().count() as u64;
        let last = compactions.last().unwrap_or_else(|| first.clone());
        Ok(Some(PreviousCompactionState {
            count,
            last_compaction_id: last.0,
            // Legacy events predate the chain fields; the branch's first
            // compaction is the chain root by construction.
            first_compaction_id: last.1.unwrap_or(first.0),
            summary: last.2,
        }))
    }

    /// Plan phase of a turn preflight: loads the branch context once,
    /// computes the provider-observed size, and — when the compaction
    /// trigger fires — describes the summarization completion the
    /// orchestrator should run. Pure with respect to the store: nothing is
    /// recorded here.
    pub(crate) fn plan_turn_context_compaction(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        connection: &ConnectionDescriptor,
        model_id: &str,
        system_prompt: Option<&str>,
        force_compaction: bool,
    ) -> Result<TurnContextPlan> {
        let context_messages =
            self.load_context_messages_with_related_sessions(session.session_id, branch.branch_id)?;
        let observed =
            self.observed_context_tokens(session.session_id, branch.branch_id, &context_messages)?;
        let previous = self.previous_compaction_state(session.session_id, branch.branch_id)?;
        let messages = context_messages
            .into_iter()
            .map(|record| record.message)
            .collect::<Vec<_>>();
        let assembler = ContextAssembler::new(self.config.clone())?;
        let summarizer_model = self
            .config
            .context
            .summarizer_model
            .clone()
            .unwrap_or_else(|| model_id.to_owned());
        let plan = assembler.plan_compaction(
            &connection.provider,
            model_id,
            &summarizer_model,
            system_prompt,
            &messages,
            force_compaction,
            observed,
            previous.as_ref().map(|state| state.summary.as_str()),
        );
        Ok(TurnContextPlan {
            observed_context_tokens: observed,
            summarization: plan.map(|plan| TurnContextSummarizationPlan {
                transcript: plan.summarization_transcript,
                summary_budget_tokens: plan.summary_budget_tokens,
                summarizer_model,
            }),
        })
    }

    pub(crate) fn prepare_turn_context_for_resolved_settings(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        connection: ConnectionDescriptor,
        model_id: String,
        system_prompt: Option<String>,
        tools: Vec<ToolSpec>,
        thinking: Option<ThinkingConfig>,
        turn_id: Option<TurnId>,
        settings_revision_id: u64,
        force_compaction: bool,
        compaction_inputs: TurnContextCompactionInputs,
    ) -> Result<PreparedTurnContext> {
        let context_messages =
            self.load_context_messages_with_related_sessions(session.session_id, branch.branch_id)?;
        let observed_context_tokens = match compaction_inputs.observed_context_tokens {
            ObservedContextTokensInput::Compute => self.observed_context_tokens(
                session.session_id,
                branch.branch_id,
                &context_messages,
            )?,
            ObservedContextTokensInput::Provided(value) => value,
        };
        let previous_compaction =
            self.previous_compaction_state(session.session_id, branch.branch_id)?;
        let context_boundary_seq_id = context_messages
            .iter()
            .filter_map(|record| record.source_seq_id)
            .max();
        let message_sources = context_messages
            .iter()
            .filter_map(|record| {
                Some(bt_core::ContextMessageSourceRef {
                    message_id: record.message.message_id,
                    branch_id: record.source_branch_id?,
                    seq_id: record.source_seq_id?,
                })
            })
            .collect::<Vec<_>>();
        let messages = context_messages
            .into_iter()
            .map(|record| record.message)
            .collect::<Vec<_>>();
        let context = ContextAssembler::new(self.config.clone())?;
        let build_started = Instant::now();
        let output = context.build_request_with_options(
            connection.id.clone(),
            &connection.provider,
            &model_id,
            system_prompt,
            messages,
            tools,
            thinking,
            bt_context::ContextBuildOptions {
                force_compaction,
                observed_context_tokens,
                previous_summary: previous_compaction
                    .as_ref()
                    .map(|state| state.summary.clone()),
                summary_override: compaction_inputs.summary_override,
            },
        );
        let build_latency_ms =
            u64::try_from(build_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut compaction = output.compaction.map(|compaction| {
            let mut report = compaction_report_from_context(compaction);
            report.phase = if force_compaction {
                bt_core::ContextCompactionPhase::Manual
            } else {
                bt_core::ContextCompactionPhase::PreTurn
            };
            report.provider = Some(connection.provider.clone());
            report.model = Some(model_id.clone());
            report.context_boundary_seq_id = context_boundary_seq_id;
            report.latency_ms = Some(build_latency_ms);
            report.window_number = Some(
                previous_compaction
                    .as_ref()
                    .map(|state| state.count)
                    .unwrap_or(0)
                    + 1,
            );
            report.previous_compaction_id = previous_compaction
                .as_ref()
                .map(|state| state.last_compaction_id);
            report.first_compaction_id = Some(
                previous_compaction
                    .as_ref()
                    .map(|state| state.first_compaction_id)
                    .unwrap_or(report.compaction_id),
            );
            if let Some(message_id) = report.first_kept_message_id
                && let Some(source) = message_sources
                    .iter()
                    .find(|source| source.message_id == message_id)
            {
                report.first_kept_branch_id = Some(source.branch_id);
                report.first_kept_seq_id = Some(source.seq_id);
            }
            // Portable identity lives in message ids; the branch ref must
            // always accompany them (seq ids are stripped from bundles).
            if report.first_kept_branch_id.is_none() {
                report.first_kept_branch_id = Some(branch.branch_id);
            }
            if let Some(suffix) = &compaction_inputs.reason_suffix {
                report.reason = Some(match report.reason.take() {
                    Some(base) => format!("{base}; {suffix}"),
                    None => suffix.clone(),
                });
            }
            report
        });
        if let Some(compaction) = &mut compaction {
            let (_, event_id) = self.record_context_compacted(
                session.session_id,
                branch.branch_id,
                compaction,
                turn_id,
            )?;
            compaction.source_event_id = Some(event_id);
        }
        Ok(PreparedTurnContext {
            settings_revision_id,
            connection,
            model_id,
            context_boundary_seq_id,
            message_sources,
            request: output.request,
            compaction,
        })
    }

    fn load_context_messages_with_related_sessions(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Vec<ContextMessageRecord>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let mut records = store.load_context_messages(session_id, branch_id)?;
        records.extend(
            store
                .load_related_session_messages(session_id)?
                .into_iter()
                .filter(|record| {
                    record.direction == RelatedSessionMessageDirection::Received
                        && matches!(
                            record.status,
                            RelatedSessionMessageStatus::Delivered
                                | RelatedSessionMessageStatus::Claimed
                        )
                        && record.message.destination_branch_id == branch_id
                })
                .map(|record| {
                    let mut message =
                        Message::text(Role::User, related_message_context_text(&record.message));
                    message.message_id = record.message.context_message_id;
                    message.created_at = record.message.created_at;
                    ContextMessageRecord {
                        message,
                        source_branch_id: Some(branch_id),
                        source_seq_id: Some(record.seq_id),
                    }
                }),
        );
        records.sort_by_key(|record| record.source_seq_id.unwrap_or(i64::MIN));
        Ok(records)
    }
}
