//! Runtime-owned turn orchestration.
//!
//! This module owns turn-loop sequencing while callers provide adapters for
//! provider/auth/tool construction that are not yet runtime-owned.

use crate::{
    BelltowerRuntime, BudgetEnforcementOutcome, ContextCompactionReport, PostTurnControlAction,
};
use bt_agent::{
    ToolLifecycleObserver, TurnApproval, TurnApprovalRequest, TurnLoop, TurnRequest,
    TurnToolCallRequest,
};
use bt_context::{SystemPromptBuilder, SystemPromptInput};
use bt_core::{
    BelltowerError, BranchId, BranchRecord, CompletionChunk, ConnectionDescriptor,
    InstructionDocument, PlanInspection, Result, SessionId, SessionRecord, SpanKind, ToolCallId,
    ToolResultEnvelope, ToolSpec, TurnId, TurnInstructionProvenance, TurnStartSource,
    traits::Provider,
};
use bt_tools::BuiltInToolRegistry;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tracing::{Instrument, info_span};

pub type TurnAdapterFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub trait TurnExecutionAdapters: Send + Sync {
    fn build_tool_registry<'a>(
        &'a self,
        session: &'a SessionRecord,
        branch_id: BranchId,
    ) -> TurnAdapterFuture<'a, BuiltInToolRegistry>;

    fn provider_for_connection<'a>(
        &'a self,
        connection: &'a ConnectionDescriptor,
    ) -> TurnAdapterFuture<'a, Arc<dyn Provider>>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnRunRequest {
    pub session: SessionRecord,
    pub branch: BranchRecord,
    pub initial_turn_id: Option<TurnId>,
    pub initial_turn_started: bool,
    pub initial_settings_revision_id: u64,
    pub initial_source: TurnStartSource,
    pub resumed_from_call_id: Option<ToolCallId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRunOutcome {
    pub session_id: SessionId,
    pub final_branch_id: BranchId,
    pub stop_reason: TurnRunStopReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnRunStopReason {
    Complete,
    AwaitingApproval,
    AwaitingInput,
    BudgetExhausted,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnResultPersistenceStatus {
    Completed,
    AwaitingApproval,
    AwaitingInput,
}

impl TurnResultPersistenceStatus {
    #[must_use]
    pub fn awaits_operator(&self) -> bool {
        matches!(self, Self::AwaitingApproval | Self::AwaitingInput)
    }
}

pub struct TurnOrchestrator<'runtime> {
    runtime: &'runtime BelltowerRuntime,
}

struct RuntimeToolLifecycleObserver<'runtime> {
    runtime: &'runtime BelltowerRuntime,
    session_id: SessionId,
    branch_id: BranchId,
    turn_id: TurnId,
    pending_terminal: Option<ToolResultEnvelope>,
}

impl ToolLifecycleObserver for RuntimeToolLifecycleObserver<'_> {
    fn tool_call_requested(
        &mut self,
        request: &TurnToolCallRequest,
        message: &bt_core::Message,
    ) -> Result<()> {
        self.runtime.record_tool_request_transition(
            self.session_id,
            self.branch_id,
            self.turn_id,
            request.call_id.clone(),
            request.tool_name.clone(),
            request.arguments.clone(),
            bt_core::ToolOperationContext {
                initiator: bt_core::ToolOperationInitiator::Agent,
                risk_class: Some(request.metadata.risk_class),
                is_read_only: Some(request.metadata.is_read_only),
                execution_mode: Some(request.metadata.execution_mode),
                artifact_refs: Vec::new(),
            },
            message.clone(),
        )?;
        Ok(())
    }

    fn tool_approval_requested(&mut self, request: &TurnApprovalRequest) -> Result<()> {
        self.runtime.record_approval_requested_for_request(
            self.session_id,
            self.branch_id,
            &request.request,
            Some(self.turn_id),
        )?;
        Ok(())
    }

    fn tool_approval_resolved(&mut self, approval: &TurnApproval) -> Result<()> {
        self.runtime.record_evaluated_approval_for_request(
            self.session_id,
            self.branch_id,
            &approval.request,
            approval.decision.clone(),
            Some(self.turn_id),
        )?;
        Ok(())
    }

    fn tool_execution_finished(
        &mut self,
        result: &ToolResultEnvelope,
        message: &bt_core::Message,
    ) -> Result<()> {
        self.pending_terminal = Some(result.clone());
        self.runtime.record_tool_terminal_transition(
            self.session_id,
            self.branch_id,
            self.turn_id,
            result.clone(),
            message.clone(),
        )?;
        self.pending_terminal = None;
        Ok(())
    }
}

impl BelltowerRuntime {
    #[must_use]
    pub fn turn_orchestrator(&self) -> TurnOrchestrator<'_> {
        TurnOrchestrator { runtime: self }
    }

    fn record_turn_result(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        connection: &ConnectionDescriptor,
        model_id: &str,
        turn_id: TurnId,
        result: bt_agent::TurnResult,
        latency_ms: u64,
    ) -> Result<TurnResultPersistenceStatus> {
        let bt_agent::TurnResult {
            messages,
            tool_results: _,
            completion_chunks: _completion_chunks,
            usage: _,
            approvals: _,
            approval_requests,
            input_requests,
            finish_reason,
        } = result;
        let awaiting_input = !input_requests.is_empty();
        let awaiting_approval = !approval_requests.is_empty();
        let finish_reason_label = format!("{finish_reason:?}");

        for message in messages
            .into_iter()
            .filter(|message| message.tool_call().is_none() && message.tool_result().is_none())
        {
            self.append_raw_message(session, branch, message, Some(turn_id))?;
        }

        self.record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_id,
            connection.provider.clone(),
            model_id.to_owned(),
            if awaiting_input {
                "awaiting_input".to_owned()
            } else if awaiting_approval {
                "awaiting_approval".to_owned()
            } else {
                "completed".to_owned()
            },
            Some(finish_reason_label),
            latency_ms,
        )?;

        Ok(if awaiting_input {
            TurnResultPersistenceStatus::AwaitingInput
        } else if awaiting_approval {
            TurnResultPersistenceStatus::AwaitingApproval
        } else {
            TurnResultPersistenceStatus::Completed
        })
    }

    pub fn record_live_completion_chunk(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        connection: &ConnectionDescriptor,
        turn_id: TurnId,
        chunk: &CompletionChunk,
    ) -> Result<()> {
        let raw_chunk_index = if let Some(raw) = &chunk.raw {
            let content = serde_json::to_vec(raw)?;
            Some(self.record_raw_chunk(
                session.session_id,
                branch.branch_id,
                connection.provider.clone(),
                "completion".to_owned(),
                chunk.llm_call_ordinal,
                &content,
                Some(turn_id),
            )?)
        } else {
            None
        };

        self.record_completion_chunk(
            session.session_id,
            branch.branch_id,
            chunk.llm_call_ordinal,
            chunk.deltas.clone(),
            raw_chunk_index,
            Some(turn_id),
        )?;

        Ok(())
    }
}

impl TurnOrchestrator<'_> {
    fn record_pre_turn_failure(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        error: &BelltowerError,
    ) -> Result<()> {
        self.runtime.record_session_error(
            session_id,
            branch_id,
            error,
            Some(turn_id),
            SpanKind::Agent,
        )?;
        Ok(())
    }

    pub async fn run_session_turns<A>(
        &self,
        adapters: &A,
        request: TurnRunRequest,
    ) -> Result<TurnRunOutcome>
    where
        A: TurnExecutionAdapters,
    {
        let session_span = session_scope_span(&request.session, request.branch.branch_id);
        async {
            let mut current_branch = request.branch;
            let mut next_turn_id = request.initial_turn_id;
            let mut next_turn_source = request.initial_source;
            let mut next_resumed_from_call_id = request.resumed_from_call_id;
            let mut next_settings_revision_id = request.initial_settings_revision_id;
            let mut initial_turn_started = request.initial_turn_started;

            loop {
                let turn_id = next_turn_id.take().unwrap_or_default();
                let settings_revision_id = next_settings_revision_id;
                let turn_started_already = initial_turn_started;
                initial_turn_started = false;

                let preflight = async {
                    let turn_session = self
                        .runtime
                        .session_for_settings_revision(&request.session, settings_revision_id)?;
                    let tools = adapters
                        .build_tool_registry(&turn_session, current_branch.branch_id)
                        .await?;
                    let instructions = self
                        .runtime
                        .resolve_instructions(Some(&turn_session.project_root))?;
                    let plan = self
                        .runtime
                        .current_plan(turn_session.session_id, current_branch.branch_id)?;
                    let tool_specs = tools.specs();
                    let built_system_prompt = build_turn_system_prompt(
                        self.runtime,
                        &turn_session,
                        &tool_specs,
                        &instructions,
                        plan.as_ref(),
                    )?;
                    let (connection, model_id) = self
                        .runtime
                        .resolve_turn_settings(turn_session.session_id, settings_revision_id)?;
                    let provider = adapters.provider_for_connection(&connection).await?;
                    let prepared = self.runtime.prepare_turn_context_for_resolved_settings(
                        &turn_session,
                        &current_branch,
                        connection,
                        model_id,
                        Some(built_system_prompt.prompt.clone()),
                        tool_specs,
                        None,
                        Some(turn_id),
                        settings_revision_id,
                        false,
                    )?;
                    let connection = prepared.connection;
                    let model_id = prepared.model_id;
                    let context_compaction = prepared.compaction.clone();
                    let completion_request = prepared.request;
                    Ok((
                        turn_session,
                        tools,
                        instructions,
                        built_system_prompt,
                        prepared.settings_revision_id,
                        context_compaction,
                        connection,
                        model_id,
                        completion_request,
                        provider,
                    ))
                }
                .await;
                let (
                    turn_session,
                    tools,
                    instructions,
                    built_system_prompt,
                    prepared_settings_revision_id,
                    context_compaction,
                    connection,
                    model_id,
                    completion_request,
                    provider,
                ) = match preflight {
                    Ok(preflight) => preflight,
                    Err(error) => {
                        self.record_pre_turn_failure(
                            request.session.session_id,
                            current_branch.branch_id,
                            turn_id,
                            &error,
                        )?;
                        return Err(error);
                    }
                };
                let turn_span = turn_scope_span(
                    &turn_session,
                    &current_branch,
                    turn_id,
                    &connection,
                    &model_id,
                );
                let post_turn_action = async {
                    if !turn_started_already {
                        self.runtime.record_turn_started(
                            turn_session.session_id,
                            current_branch.branch_id,
                            turn_id,
                            connection.provider.clone(),
                            model_id.clone(),
                            completion_request.messages.len() as u32,
                            prepared_settings_revision_id,
                            next_turn_source.clone(),
                            next_resumed_from_call_id.take(),
                        )?;
                    }
                    self.runtime.record_turn_instruction_provenance(
                        turn_session.session_id,
                        current_branch.branch_id,
                        TurnInstructionProvenance {
                            turn_id,
                            provider: connection.provider.clone(),
                            model: model_id.clone(),
                            settings_revision_id: prepared_settings_revision_id,
                            core_prompt: built_system_prompt.core_prompt.clone(),
                            provider_overlay: built_system_prompt.provider_overlay.clone(),
                            instructions: instructions.clone(),
                            rendered_system_prompt: built_system_prompt.prompt.clone(),
                        },
                    )?;

                    let started = Instant::now();
                    let turn_loop =
                        TurnLoop::new(provider.clone(), tools, self.runtime.approval_evaluator());
                    let mut tool_lifecycle = RuntimeToolLifecycleObserver {
                        runtime: self.runtime,
                        session_id: turn_session.session_id,
                        branch_id: current_branch.branch_id,
                        turn_id,
                        pending_terminal: None,
                    };
                    let result = match turn_loop
                        .run_turn_with_tool_observer(
                            TurnRequest {
                                session_id: turn_session.session_id,
                                project_root: turn_session.project_root.clone(),
                                request: completion_request,
                            },
                            |llm_call| {
                                let (context_boundary_seq_id, message_sources) =
                                    self.runtime.context_message_sources_for_request(
                                        turn_session.session_id,
                                        current_branch.branch_id,
                                        &llm_call.request,
                                    )?;
                                let mut manifest =
                                    bt_core::ContextManifest::from_completion_request(
                                        turn_id,
                                        current_branch.branch_id,
                                        llm_call.ordinal,
                                        connection.provider.clone(),
                                        llm_call.model.clone(),
                                        prepared_settings_revision_id,
                                        context_boundary_seq_id,
                                        &llm_call.request,
                                        context_compaction.is_some(),
                                        &message_sources,
                                    );
                                if let Some(compaction) = &context_compaction {
                                    manifest
                                        .attachments
                                        .push(compaction_context_attachment(compaction));
                                }
                                self.runtime.record_context_manifest(
                                    turn_session.session_id,
                                    current_branch.branch_id,
                                    manifest,
                                )?;
                                self.runtime.record_completion_requested(
                                    turn_session.session_id,
                                    current_branch.branch_id,
                                    llm_call.ordinal,
                                    connection.provider.clone(),
                                    llm_call.model.clone(),
                                    llm_call.message_count,
                                    turn_id,
                                )?;
                                Ok(())
                            },
                            |chunk| {
                                self.runtime.record_live_completion_chunk(
                                    &turn_session,
                                    &current_branch,
                                    &connection,
                                    turn_id,
                                    chunk,
                                )
                            },
                            |llm_call| {
                                let mut summary = llm_call.summary.clone();
                                summary.provider = connection.provider.clone();
                                self.runtime.record_completion_finished(
                                    turn_session.session_id,
                                    current_branch.branch_id,
                                    summary,
                                    llm_call.ordinal,
                                    turn_id,
                                )?;
                                Ok(())
                            },
                            &mut tool_lifecycle,
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            let latency_ms = started.elapsed().as_millis() as u64;
                            self.runtime.record_turn_failure_transition(
                                turn_session.session_id,
                                current_branch.branch_id,
                                &connection.provider,
                                &model_id,
                                turn_id,
                                tool_lifecycle.pending_terminal.take().into_iter().collect(),
                                &error,
                                latency_ms,
                            )?;
                            let _ = self.runtime.checkpoint_session_budget(
                                turn_session.session_id,
                                current_branch.branch_id,
                                turn_id,
                            )?;
                            return Err(error);
                        }
                    };

                    let latency_ms = started.elapsed().as_millis() as u64;
                    let persistence = self.runtime.record_turn_result(
                        &turn_session,
                        &current_branch,
                        &connection,
                        &model_id,
                        turn_id,
                        result,
                        latency_ms,
                    )?;
                    let budget_outcome = self.runtime.checkpoint_session_budget(
                        turn_session.session_id,
                        current_branch.branch_id,
                        turn_id,
                    )?;

                    if persistence.awaits_operator() {
                        return Ok((
                            PostTurnControlAction::Stop,
                            match persistence {
                                TurnResultPersistenceStatus::AwaitingApproval => {
                                    TurnRunStopReason::AwaitingApproval
                                }
                                TurnResultPersistenceStatus::AwaitingInput => {
                                    TurnRunStopReason::AwaitingInput
                                }
                                TurnResultPersistenceStatus::Completed => {
                                    TurnRunStopReason::Complete
                                }
                            },
                        ));
                    }
                    if budget_outcome == BudgetEnforcementOutcome::Exhausted {
                        return Ok((
                            PostTurnControlAction::Stop,
                            TurnRunStopReason::BudgetExhausted,
                        ));
                    }

                    let was_cancelled = self.runtime.is_cancelled(turn_session.session_id)?;
                    self.runtime
                        .consume_post_turn_controls(&turn_session, &current_branch)
                        .map(|action| {
                            let stop_reason = if was_cancelled {
                                TurnRunStopReason::Cancelled
                            } else {
                                TurnRunStopReason::Complete
                            };
                            (action, stop_reason)
                        })
                }
                .instrument(turn_span)
                .await?;

                match post_turn_action.0 {
                    PostTurnControlAction::ContinueQueuedBranch(dispatch) => {
                        next_turn_source = TurnStartSource::QueuedFollowUp;
                        next_settings_revision_id = dispatch.settings_revision_id;
                        current_branch = self
                            .runtime
                            .load_branch(turn_session.session_id, dispatch.branch_id)?
                            .ok_or_else(|| {
                                BelltowerError::InvalidState("queued branch not found".to_owned())
                            })?;
                        continue;
                    }
                    PostTurnControlAction::ContinueCurrentBranch(settings_revision_id) => {
                        next_turn_source = TurnStartSource::SteerFollowUp;
                        next_settings_revision_id = settings_revision_id;
                        continue;
                    }
                    PostTurnControlAction::Stop => {
                        return Ok(TurnRunOutcome {
                            session_id: request.session.session_id,
                            final_branch_id: current_branch.branch_id,
                            stop_reason: post_turn_action.1,
                        });
                    }
                }
            }
        }
        .instrument(session_span)
        .await
    }
}

#[derive(Clone, Debug)]
struct BuiltTurnSystemPrompt {
    prompt: String,
    core_prompt: InstructionDocument,
    provider_overlay: Option<InstructionDocument>,
}

fn build_turn_system_prompt(
    runtime: &BelltowerRuntime,
    session: &SessionRecord,
    tools: &[ToolSpec],
    instructions: &[InstructionDocument],
    plan: Option<&PlanInspection>,
) -> Result<BuiltTurnSystemPrompt> {
    let connection = runtime.connection(&session.connection_id).ok_or_else(|| {
        BelltowerError::InvalidState(format!("connection `{}` not found", session.connection_id))
    })?;
    let core_prompt = runtime.core_prompt_document();
    let provider_overlay = runtime.provider_prompt_overlay(&connection.provider);

    let prompt = SystemPromptBuilder::build(SystemPromptInput {
        session,
        provider_family: &connection.provider,
        core_prompt: &core_prompt,
        provider_overlay: provider_overlay.as_ref(),
        instructions,
        plan,
        tools,
    });

    Ok(BuiltTurnSystemPrompt {
        prompt,
        core_prompt,
        provider_overlay,
    })
}

fn compaction_context_attachment(
    compaction: &ContextCompactionReport,
) -> bt_core::ContextAttachment {
    bt_core::ContextAttachment {
        id: compaction.compaction_id.to_string(),
        attachment_type: bt_core::ContextAttachmentType::Compaction,
        producer: bt_core::ContextAttachmentProducer::Runtime,
        source_event_id: compaction.source_event_id,
        source_tool_call_id: None,
        source_uri: compaction
            .summary_message_id
            .map(|message_id| format!("message:{message_id}")),
        digest: None,
        visibility: bt_core::ContextAttachmentVisibility {
            model_visible: true,
            operator_visible: true,
            exportable: true,
        },
        placement: bt_core::ContextPromptPlacement::MessageHistory,
        token_estimate: None,
        truncation: bt_core::ContextTruncationStatus::None,
    }
}

fn session_scope_span(session: &SessionRecord, branch_id: BranchId) -> tracing::Span {
    info_span!(
        "session",
        session_id = %session.session_id,
        branch_id = %branch_id,
        connection_id = %session.connection_id,
        project_root = %session.project_root,
    )
}

fn turn_scope_span(
    session: &SessionRecord,
    branch: &BranchRecord,
    turn_id: TurnId,
    connection: &ConnectionDescriptor,
    model_id: &str,
) -> tracing::Span {
    info_span!(
        "turn",
        session_id = %session.session_id,
        branch_id = %branch.branch_id,
        turn_id = %turn_id,
        connection_id = %connection.id,
        provider = %connection.provider,
        model = %model_id,
    )
}
