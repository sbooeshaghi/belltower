use super::*;
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
