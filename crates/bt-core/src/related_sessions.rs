use crate::{
    ArtifactRef, BranchId, EventId, MessageId, RelatedSessionMessageId, SessionId, TurnId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub const MAX_RELATED_SESSION_DEPTH: usize = 4;
pub const MAX_RELATED_SESSION_DESCENDANTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedSessionMessageKind {
    Instruction,
    Question,
    Answer,
    Progress,
    Result,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedSessionDeliveryMode {
    Notify,
    Wake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedSessionMessageDirection {
    Sent,
    Received,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedSessionMessageStatus {
    Delivered,
    Pending,
    Claimed,
    Dropped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RelatedSessionMessage {
    pub message_id: RelatedSessionMessageId,
    pub context_message_id: MessageId,
    pub source_session_id: SessionId,
    pub source_branch_id: BranchId,
    pub caused_by_turn_id: Option<TurnId>,
    pub destination_session_id: SessionId,
    pub destination_branch_id: BranchId,
    pub kind: RelatedSessionMessageKind,
    pub delivery_mode: RelatedSessionDeliveryMode,
    pub in_reply_to: Option<RelatedSessionMessageId>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactRef>,
    #[schemars(schema_with = "crate::schema_support::offset_date_time_schema")]
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedSessionMessageReceipt {
    pub message_id: RelatedSessionMessageId,
    pub sent_event_id: EventId,
    pub sent_seq_id: i64,
    pub received_event_id: EventId,
    pub received_seq_id: i64,
    pub status: RelatedSessionMessageStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RelatedSessionMessageSettlement {
    pub obligation_message_id: RelatedSessionMessageId,
    pub reply_message_id: RelatedSessionMessageId,
    pub reply_kind: RelatedSessionMessageKind,
    pub settling_turn_id: TurnId,
    pub settlement_event_id: EventId,
    pub settlement_seq_id: i64,
    #[schemars(schema_with = "crate::schema_support::offset_date_time_schema")]
    pub settled_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedSessionSettlementReceipt {
    pub settlement: RelatedSessionMessageSettlement,
    pub reply: RelatedSessionMessageReceipt,
    pub newly_committed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedSessionMessageRecord {
    pub message: RelatedSessionMessage,
    pub direction: RelatedSessionMessageDirection,
    pub status: RelatedSessionMessageStatus,
    pub event_id: EventId,
    pub counterpart_event_id: EventId,
    pub seq_id: i64,
    pub resulting_turn_id: Option<TurnId>,
    pub resolved_at: Option<OffsetDateTime>,
    pub settlement: Option<RelatedSessionMessageSettlement>,
}
