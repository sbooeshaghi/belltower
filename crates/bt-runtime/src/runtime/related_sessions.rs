use super::*;
use crate::TurnRunStopReason;
use std::collections::HashSet;

impl BelltowerRuntime {
    pub fn validate_subagent_spawn(&self, parent_session_id: SessionId) -> Result<()> {
        let mut cursor = self.load_session(parent_session_id)?.ok_or_else(|| {
            bt_core::BelltowerError::NotFound(format!("session `{parent_session_id}`"))
        })?;
        let mut ancestors = HashSet::new();
        ancestors.insert(cursor.session_id);
        let mut depth = 0usize;
        while let Some(parent_id) = cursor.parent_session_id {
            if !ancestors.insert(parent_id) {
                return Err(bt_core::BelltowerError::InvalidState(
                    "session lineage contains a cycle".to_owned(),
                ));
            }
            depth = depth.saturating_add(1);
            cursor = self.load_session(parent_id)?.ok_or_else(|| {
                bt_core::BelltowerError::InvalidState(format!(
                    "session lineage references missing parent `{parent_id}`"
                ))
            })?;
        }
        if depth >= MAX_RELATED_SESSION_DEPTH {
            return Err(bt_core::BelltowerError::Protocol(format!(
                "subagent depth limit of {MAX_RELATED_SESSION_DEPTH} reached"
            )));
        }

        let root_id = cursor.session_id;
        let mut stack = vec![root_id];
        let mut visited = HashSet::new();
        let mut descendants = 0usize;
        while let Some(session_id) = stack.pop() {
            if !visited.insert(session_id) {
                return Err(bt_core::BelltowerError::InvalidState(
                    "session lineage contains a cycle".to_owned(),
                ));
            }
            let children = self
                .store
                .lock()
                .map_err(|_| {
                    bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
                })?
                .load_child_sessions(session_id)?;
            descendants = descendants.saturating_add(children.len());
            stack.extend(children.into_iter().map(|child| child.session_id));
        }
        if descendants >= MAX_RELATED_SESSION_DESCENDANTS {
            return Err(bt_core::BelltowerError::Protocol(format!(
                "subagent lineage limit of {MAX_RELATED_SESSION_DESCENDANTS} reached"
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_related_session_message(
        &self,
        source_session_id: SessionId,
        source_branch_id: BranchId,
        caused_by_turn_id: Option<TurnId>,
        destination_session_id: SessionId,
        destination_branch_id: BranchId,
        kind: RelatedSessionMessageKind,
        delivery_mode: RelatedSessionDeliveryMode,
        in_reply_to: Option<RelatedSessionMessageId>,
        text: String,
        artifact_refs: Vec<bt_core::ArtifactRef>,
    ) -> Result<RelatedSessionMessageReceipt> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(bt_core::BelltowerError::Protocol(
                "related-session message cannot be empty".to_owned(),
            ));
        }
        let destination_branch_id = self.resolve_related_session_destination_branch(
            source_session_id,
            source_branch_id,
            destination_session_id,
            destination_branch_id,
            in_reply_to,
        )?;
        let (message, sent, received) = Self::related_session_message_events(
            source_session_id,
            source_branch_id,
            caused_by_turn_id,
            destination_session_id,
            destination_branch_id,
            kind,
            delivery_mode,
            in_reply_to,
            text,
            artifact_refs,
        )?;

        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let receipt = store.commit_related_session_message(&sent, &received)?;
        self.publish_committed_event(sent, receipt.sent_seq_id);
        self.publish_committed_event(received, receipt.received_seq_id);
        debug_assert_eq!(receipt.message_id, message.message_id);
        Ok(receipt)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn related_session_message_events(
        source_session_id: SessionId,
        source_branch_id: BranchId,
        caused_by_turn_id: Option<TurnId>,
        destination_session_id: SessionId,
        destination_branch_id: BranchId,
        kind: RelatedSessionMessageKind,
        delivery_mode: RelatedSessionDeliveryMode,
        in_reply_to: Option<RelatedSessionMessageId>,
        text: String,
        artifact_refs: Vec<bt_core::ArtifactRef>,
    ) -> Result<(RelatedSessionMessage, EventEnvelope, EventEnvelope)> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err(bt_core::BelltowerError::Protocol(
                "related-session message cannot be empty".to_owned(),
            ));
        }
        let message = RelatedSessionMessage {
            message_id: RelatedSessionMessageId::new(),
            context_message_id: bt_core::MessageId::new(),
            source_session_id,
            source_branch_id,
            caused_by_turn_id,
            destination_session_id,
            destination_branch_id,
            kind,
            delivery_mode,
            in_reply_to,
            text,
            artifact_refs,
            created_at: time::OffsetDateTime::now_utc(),
        };
        let sent_event_id = bt_core::EventId::new();
        let received_event_id = bt_core::EventId::new();
        let mut sent = EventEnvelope::new(
            source_session_id,
            source_branch_id,
            SpanKind::Chain,
            EventPayload::RelatedSessionMessageRecorded {
                direction: RelatedSessionMessageDirection::Sent,
                counterpart_event_id: received_event_id,
                message: message.clone(),
            },
        );
        sent.event_id = sent_event_id;
        if let Some(turn_id) = caused_by_turn_id {
            sent = sent.with_turn_id(turn_id);
        }
        let mut received = EventEnvelope::new(
            destination_session_id,
            destination_branch_id,
            SpanKind::Chain,
            EventPayload::RelatedSessionMessageRecorded {
                direction: RelatedSessionMessageDirection::Received,
                counterpart_event_id: sent_event_id,
                message: message.clone(),
            },
        );
        received.event_id = received_event_id;

        Ok((message, sent, received))
    }

    pub fn related_session_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<RelatedSessionMessageRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_related_session_messages(session_id)
    }

    /// Resolve the destination branch for a new related-session message.
    ///
    /// New conversations must name their destination branch explicitly. A
    /// reply instead derives the exact reverse edge from the immutable
    /// received message; an explicit branch may confirm that edge but cannot
    /// redirect it after a session's default branch changes.
    fn resolve_related_session_destination_branch(
        &self,
        source_session_id: SessionId,
        source_branch_id: BranchId,
        destination_session_id: SessionId,
        requested_destination_branch_id: BranchId,
        in_reply_to: Option<RelatedSessionMessageId>,
    ) -> Result<BranchId> {
        let destination_branch_id = match in_reply_to {
            Some(reply_id) => {
                let original = self
                    .related_session_messages(source_session_id)?
                    .into_iter()
                    .find(|record| {
                        record.direction == RelatedSessionMessageDirection::Received
                            && record.message.message_id == reply_id
                    })
                    .ok_or_else(|| {
                        bt_core::BelltowerError::Protocol(
                            "related-session reply target was not received by the sender"
                                .to_owned(),
                        )
                    })?
                    .message;
                if original.destination_session_id != source_session_id
                    || original.destination_branch_id != source_branch_id
                    || original.source_session_id != destination_session_id
                {
                    return Err(bt_core::BelltowerError::Protocol(
                        "related-session reply must reverse the original session and branch edge"
                            .to_owned(),
                    ));
                }
                if requested_destination_branch_id != original.source_branch_id {
                    return Err(bt_core::BelltowerError::Protocol(
                        "target_branch_id conflicts with the original related-session message"
                            .to_owned(),
                    ));
                }
                original.source_branch_id
            }
            None => requested_destination_branch_id,
        };
        self.load_branch(destination_session_id, destination_branch_id)?
            .ok_or_else(|| {
                bt_core::BelltowerError::NotFound(format!(
                    "branch `{destination_branch_id}` in session `{destination_session_id}`"
                ))
            })?;
        Ok(destination_branch_id)
    }

    pub fn pending_related_session_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<RelatedSessionMessageRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_pending_related_session_messages(session_id)
    }

    pub fn all_pending_related_session_messages(&self) -> Result<Vec<RelatedSessionMessageRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_all_pending_related_session_messages()
    }

    pub fn all_unsettled_related_session_obligations(
        &self,
    ) -> Result<Vec<RelatedSessionMessageRecord>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_all_unsettled_related_session_obligations()
    }

    pub fn record_related_turn_outcome(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        stop_reason: TurnRunStopReason,
    ) -> Result<usize> {
        let (kind, text) = match stop_reason {
            TurnRunStopReason::Complete => (
                RelatedSessionMessageKind::Result,
                self.store
                    .lock()
                    .map_err(|_| {
                        bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
                    })?
                    .load_messages_for_turn(session_id, branch_id, turn_id)?
                    .into_iter()
                    .rev()
                    .find(|message| message.role == Role::Assistant)
                    .map(|message| message.text_parts().collect::<Vec<_>>().join("\n"))
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "Related-session turn completed.".to_owned()),
            ),
            TurnRunStopReason::AwaitingApproval => (
                RelatedSessionMessageKind::Progress,
                "Related-session turn is waiting for tool approval.".to_owned(),
            ),
            TurnRunStopReason::AwaitingInput => (
                RelatedSessionMessageKind::Progress,
                "Related-session turn is waiting for operator input.".to_owned(),
            ),
            TurnRunStopReason::BudgetExhausted => (
                RelatedSessionMessageKind::Error,
                "Related-session turn stopped because its budget was exhausted.".to_owned(),
            ),
            TurnRunStopReason::Cancelled => (
                RelatedSessionMessageKind::Error,
                "Related-session turn was cancelled.".to_owned(),
            ),
        };
        self.record_related_reply_obligations(session_id, branch_id, turn_id, kind, text)
    }

    pub fn record_related_turn_failure(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        error: &bt_core::BelltowerError,
    ) -> Result<usize> {
        self.record_related_reply_obligations(
            session_id,
            branch_id,
            turn_id,
            RelatedSessionMessageKind::Error,
            format!("Related-session turn failed: {error}"),
        )
    }

    pub fn fail_admitted_related_turn(
        &self,
        admitted: &AdmittedTurn,
        error: &bt_core::BelltowerError,
    ) -> Result<()> {
        let (connection, model) =
            self.resolve_turn_settings(admitted.session_id(), admitted.settings_revision_id())?;
        self.record_turn_failure_transition(
            admitted.session_id(),
            admitted.branch_id(),
            &connection.provider,
            &model,
            admitted.turn_id(),
            Vec::new(),
            error,
            0,
        )?;

        if self
            .record_related_turn_failure(
                admitted.session_id(),
                admitted.branch_id(),
                admitted.turn_id(),
                error,
            )
            .is_err()
        {
            // The turn is already terminal. Reconcile the idempotent atomic
            // reply-pair settlement once in-process; startup recovery remains
            // the backstop for a persistent storage failure.
            self.record_related_turn_failure(
                admitted.session_id(),
                admitted.branch_id(),
                admitted.turn_id(),
                error,
            )?;
        }
        Ok(())
    }

    fn record_related_reply_obligations(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        kind: RelatedSessionMessageKind,
        text: String,
    ) -> Result<usize> {
        let records = self.related_session_messages(session_id)?;
        let obligations = records
            .iter()
            .filter(|record| {
                record.direction == RelatedSessionMessageDirection::Received
                    && record.status == RelatedSessionMessageStatus::Claimed
                    && record.settlement.is_none()
                    && record.message.destination_branch_id == branch_id
                    && matches!(
                        record.message.kind,
                        RelatedSessionMessageKind::Instruction
                            | RelatedSessionMessageKind::Question
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        let terminal = matches!(
            kind,
            RelatedSessionMessageKind::Result | RelatedSessionMessageKind::Error
        );
        let mut recorded = 0usize;
        for obligation in obligations {
            if !terminal {
                let duplicate = records.iter().any(|candidate| {
                    candidate.direction == RelatedSessionMessageDirection::Sent
                        && candidate.message.in_reply_to == Some(obligation.message.message_id)
                        && candidate.message.kind == kind
                        && candidate.message.text == text
                });
                if !duplicate {
                    self.send_related_session_message(
                        session_id,
                        branch_id,
                        Some(turn_id),
                        obligation.message.source_session_id,
                        obligation.message.source_branch_id,
                        kind,
                        RelatedSessionDeliveryMode::Notify,
                        Some(obligation.message.message_id),
                        text.clone(),
                        Vec::new(),
                    )?;
                    recorded = recorded.saturating_add(1);
                }
                continue;
            }

            let (_, sent, received) = Self::related_session_message_events(
                session_id,
                branch_id,
                Some(turn_id),
                obligation.message.source_session_id,
                obligation.message.source_branch_id,
                kind,
                RelatedSessionDeliveryMode::Wake,
                Some(obligation.message.message_id),
                text.clone(),
                Vec::new(),
            )?;
            let reply_message_id = match &sent.payload {
                EventPayload::RelatedSessionMessageRecorded { message, .. } => message.message_id,
                _ => unreachable!("related-session event constructor returns a message event"),
            };
            let settlement_event = EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Chain,
                EventPayload::RelatedSessionMessageSettled {
                    message_id: obligation.message.message_id,
                    reply_message_id,
                    reply_kind: kind,
                    settling_turn_id: turn_id,
                },
            )
            .with_turn_id(turn_id);
            self.take_store_append_fault_for_test()?;
            let receipt = self
                .store
                .lock()
                .map_err(|_| {
                    bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
                })?
                .settle_related_session_message(
                    obligation.message.message_id,
                    &sent,
                    &received,
                    &settlement_event,
                )?;
            if receipt.newly_committed {
                self.publish_committed_event(sent, receipt.reply.sent_seq_id);
                self.publish_committed_event(received, receipt.reply.received_seq_id);
                self.publish_committed_event(
                    settlement_event,
                    receipt.settlement.settlement_seq_id,
                );
                recorded = recorded.saturating_add(1);
            }
        }
        Ok(recorded)
    }

    pub fn claim_related_messages_for_active_turn(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        turn_id: TurnId,
        message_ids: &[RelatedSessionMessageId],
    ) -> Result<bool> {
        let events = message_ids
            .iter()
            .map(|message_id| {
                EventEnvelope::new(
                    session_id,
                    branch_id,
                    SpanKind::Chain,
                    EventPayload::RelatedSessionMessageResolved {
                        message_id: *message_id,
                        status: RelatedSessionMessageStatus::Claimed,
                        resulting_turn_id: Some(turn_id),
                        reason: Some("Observed by wait_agent in the active turn.".to_owned()),
                    },
                )
                .with_turn_id(turn_id)
            })
            .collect::<Vec<_>>();
        let claim = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .claim_related_messages_for_active_turn(
                session_id,
                branch_id,
                turn_id,
                message_ids,
                &events,
            )?;
        match claim {
            ContinuationClaim::Claimed { seq_ids } => {
                for (event, seq_id) in events.into_iter().zip(seq_ids) {
                    self.publish_committed_event(event, seq_id);
                }
                Ok(true)
            }
            ContinuationClaim::Stale => Ok(false),
            ContinuationClaim::Busy => Err(bt_core::BelltowerError::InvalidState(
                "wait_agent caller no longer owns the active turn".to_owned(),
            )),
            ContinuationClaim::BudgetExhausted | ContinuationClaim::CancelPending => {
                Err(bt_core::BelltowerError::InvalidState(
                    "active-turn related-message claim returned an impossible control state"
                        .to_owned(),
                ))
            }
        }
    }

    pub fn claim_next_related_session_message(
        &self,
        session_id: SessionId,
    ) -> Result<Option<AdmittedTurn>> {
        const MAX_STALE_RETRIES: usize = 8;

        for _ in 0..MAX_STALE_RETRIES {
            let Some(pending) = self
                .pending_related_session_messages(session_id)?
                .into_iter()
                .next()
            else {
                return Ok(None);
            };
            let session = self.load_session(session_id)?.ok_or_else(|| {
                bt_core::BelltowerError::NotFound(format!("session `{session_id}`"))
            })?;
            let settings_revision_id = session.settings_revision_id;
            let (connection, model_id) =
                self.resolve_turn_settings(session_id, settings_revision_id)?;
            let turn_id = TurnId::new();
            let branch_id = pending.message.destination_branch_id;
            let message_count = self
                .messages(session_id, Some(branch_id))?
                .len()
                .saturating_add(
                    self.related_session_messages(session_id)?
                        .into_iter()
                        .filter(|record| {
                            record.direction == RelatedSessionMessageDirection::Received
                                && record.message.destination_branch_id == branch_id
                        })
                        .count(),
                ) as u32;
            let events = vec![
                EventEnvelope::new(
                    session_id,
                    branch_id,
                    SpanKind::Chain,
                    EventPayload::RelatedSessionMessageResolved {
                        message_id: pending.message.message_id,
                        status: RelatedSessionMessageStatus::Claimed,
                        resulting_turn_id: Some(turn_id),
                        reason: Some("Dispatched to the destination session.".to_owned()),
                    },
                ),
                self.turn_started_event(
                    session_id,
                    branch_id,
                    turn_id,
                    connection.provider,
                    model_id,
                    message_count,
                    settings_revision_id,
                    TurnStartSource::RelatedSessionMessage,
                    None,
                ),
            ];
            self.take_store_append_fault_for_test()?;
            let claim = self
                .store
                .lock()
                .map_err(|_| {
                    bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
                })?
                .claim_related_message_continuation(
                    session_id,
                    pending.message.message_id,
                    settings_revision_id,
                    &events,
                )?;
            match claim {
                ContinuationClaim::Claimed { seq_ids } => {
                    for (event, seq_id) in events.into_iter().zip(seq_ids) {
                        self.publish_committed_event(event, seq_id);
                    }
                    return Ok(Some(AdmittedTurn::new(
                        session_id,
                        branch_id,
                        turn_id,
                        settings_revision_id,
                    )));
                }
                ContinuationClaim::Stale => continue,
                ContinuationClaim::Busy
                | ContinuationClaim::BudgetExhausted
                | ContinuationClaim::CancelPending => return Ok(None),
            }
        }

        Err(bt_core::BelltowerError::InvalidState(format!(
            "related-session wake for `{session_id}` could not stabilize its settings revision"
        )))
    }
}
