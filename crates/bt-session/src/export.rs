use crate::{RawChunkInsert, RawChunkRecord, SqliteSessionStore, StoredSessionEvent};
use bt_core::{
    BelltowerError, BranchId, BranchManifest, BranchRecord, BundleArtifactRef, ContentRef,
    EventEnvelope, EventId, EventPayload, EventRange, Message, PortableHash, ProducerInfo,
    RedactionPolicy, Result, SessionBundleArtifactMode, SessionBundleManifest, SessionId,
    SessionNodeRef, SessionRecord, SpanKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use time::OffsetDateTime;

pub const LEGACY_SESSION_EXPORT_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const SESSION_BT_SCHEMA_VERSION: u32 = 1;
pub const SESSION_BT_MANIFEST: &str = "manifest.json";
pub const SESSION_BT_EVENTS: &str = "events.jsonl";
pub const SESSION_BT_RAW_CHUNK_REFS: &str = "raw_chunk_refs.jsonl";
pub const SESSION_BT_CHECKSUMS: &str = "checksums.json";
pub const SESSION_BT_RAW_CHUNKS_DIR: &str = "raw_chunks";
pub const SESSION_BT_PAYLOADS_DIR: &str = "payloads";
pub const SESSION_BT_ARTIFACTS_DIR: &str = "artifacts";
pub const SESSION_BT_PARENT_NODE_REF_ATTR: &str = "belltower.session.parent_external_ref";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionBundleArtifactSource {
    File(PathBuf),
    Bytes {
        display_name: String,
        bytes: Vec<u8>,
        media_type: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBundleArtifactInput {
    pub artifact_id: Option<String>,
    pub kind: String,
    pub source: SessionBundleArtifactSource,
}

impl SessionBundleArtifactInput {
    #[must_use]
    pub fn file(path: impl Into<PathBuf>, kind: impl Into<String>) -> Self {
        Self {
            artifact_id: None,
            kind: kind.into(),
            source: SessionBundleArtifactSource::File(path.into()),
        }
    }

    #[must_use]
    pub fn bytes(
        display_name: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        kind: impl Into<String>,
        media_type: Option<String>,
    ) -> Self {
        Self {
            artifact_id: None,
            kind: kind.into(),
            source: SessionBundleArtifactSource::Bytes {
                display_name: display_name.into(),
                bytes: bytes.into(),
                media_type,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBundleExportOptions {
    pub artifact_mode: SessionBundleArtifactMode,
    pub artifacts: Vec<SessionBundleArtifactInput>,
}

impl Default for SessionBundleExportOptions {
    fn default() -> Self {
        Self {
            artifact_mode: SessionBundleArtifactMode::TraceOnly,
            artifacts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacySessionExportBundle {
    pub schema_version: u32,
    pub session: SessionRecord,
    pub branches: Vec<BranchRecord>,
    pub message_branch_id: Option<BranchId>,
    pub messages: Vec<Message>,
    pub events: Vec<EventEnvelope>,
    pub raw_chunks: Vec<RawChunkRecord>,
}

pub trait LegacySessionExporter {
    fn export_legacy_bundle(&self, session_id: SessionId) -> Result<LegacySessionExportBundle>;
}

impl LegacySessionExporter for SqliteSessionStore {
    fn export_legacy_bundle(&self, session_id: SessionId) -> Result<LegacySessionExportBundle> {
        let session = self
            .load_session(session_id)?
            .ok_or_else(|| BelltowerError::InvalidState("session not found".to_owned()))?;
        let branches = self.load_branches(session_id)?;
        let message_branch_id = branches
            .iter()
            .find(|branch| branch.is_default)
            .map(|branch| branch.branch_id);
        let messages = self.load_messages(session_id, message_branch_id)?;
        let events = self.load_all_events(session_id)?;
        let raw_chunks = self.load_all_raw_chunks(session_id)?;

        Ok(LegacySessionExportBundle {
            schema_version: LEGACY_SESSION_EXPORT_BUNDLE_SCHEMA_VERSION,
            session,
            branches,
            message_branch_id,
            messages,
            events,
            raw_chunks,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleEventRecord {
    pub bundle_event_ordinal: u64,
    pub event_kind: String,
    pub event_hash: PortableHash,
    pub event: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleRawChunkRef {
    pub chunk_ordinal: u64,
    pub session_id: SessionId,
    pub branch_id: Option<BranchId>,
    pub turn_id: Option<bt_core::TurnId>,
    pub llm_call_ordinal: Option<u32>,
    pub event_id: Option<String>,
    pub provider: String,
    pub stream_name: String,
    pub content: ContentRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleChecksumEntry {
    pub path: String,
    pub hash: PortableHash,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleChecksums {
    pub schema_version: u32,
    pub files: Vec<SessionBundleChecksumEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleValidationReport {
    pub bundle_hash: PortableHash,
    pub session_id: SessionId,
    pub event_count: usize,
    pub raw_chunk_count: usize,
    pub content_count: usize,
    pub artifact_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleImportReport {
    pub bundle_hash: PortableHash,
    pub session_id: SessionId,
    pub branch_count: usize,
    pub event_count: usize,
    pub raw_chunk_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionBundleDiffRelationship {
    Equivalent,
    SameLineageUpdate,
    ForkDivergence,
    DifferentLineage,
    RedactionDifference,
    StructuralDifference,
}

impl SessionBundleDiffRelationship {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Equivalent => "equivalent",
            Self::SameLineageUpdate => "same_lineage_update",
            Self::ForkDivergence => "fork_divergence",
            Self::DifferentLineage => "different_lineage",
            Self::RedactionDifference => "redaction_difference",
            Self::StructuralDifference => "structural_difference",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleDiffSide {
    pub bundle_hash: PortableHash,
    pub session_id: SessionId,
    pub event_count: usize,
    pub branch_count: usize,
    pub content_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleDiffReport {
    pub relationship: SessionBundleDiffRelationship,
    pub left: SessionBundleDiffSide,
    pub right: SessionBundleDiffSide,
    pub common_event_count: usize,
    pub left_only_event_count: usize,
    pub right_only_event_count: usize,
    pub left_only_branch_ids: Vec<BranchId>,
    pub right_only_branch_ids: Vec<BranchId>,
    pub left_only_content_hashes: Vec<PortableHash>,
    pub right_only_content_hashes: Vec<PortableHash>,
    pub left_only_artifact_ids: Vec<String>,
    pub right_only_artifact_ids: Vec<String>,
    pub redaction_changed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleContinuationReport {
    pub bundle_hash: PortableHash,
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub parent_branch_id: BranchId,
    pub parent_event_id: EventId,
    pub parent_event_hash: PortableHash,
    pub imported_session: bool,
}

pub trait PortableSessionBundleExporter {
    fn export_session_bundle_directory(
        &self,
        session_id: SessionId,
        output_dir: impl AsRef<Path>,
    ) -> Result<SessionBundleManifest>;

    fn export_session_bundle_directory_with_options(
        &self,
        session_id: SessionId,
        output_dir: impl AsRef<Path>,
        options: &SessionBundleExportOptions,
    ) -> Result<SessionBundleManifest>;
}

impl PortableSessionBundleExporter for SqliteSessionStore {
    fn export_session_bundle_directory(
        &self,
        session_id: SessionId,
        output_dir: impl AsRef<Path>,
    ) -> Result<SessionBundleManifest> {
        export_session_bundle_directory(
            self,
            session_id,
            output_dir.as_ref(),
            &SessionBundleExportOptions::default(),
        )
    }

    fn export_session_bundle_directory_with_options(
        &self,
        session_id: SessionId,
        output_dir: impl AsRef<Path>,
        options: &SessionBundleExportOptions,
    ) -> Result<SessionBundleManifest> {
        export_session_bundle_directory(self, session_id, output_dir.as_ref(), options)
    }
}

pub trait PortableSessionBundleImporter {
    fn import_session_bundle_directory(
        &mut self,
        bundle_dir: impl AsRef<Path>,
    ) -> Result<SessionBundleImportReport>;
}

impl PortableSessionBundleImporter for SqliteSessionStore {
    fn import_session_bundle_directory(
        &mut self,
        bundle_dir: impl AsRef<Path>,
    ) -> Result<SessionBundleImportReport> {
        import_session_bundle_directory(self, bundle_dir.as_ref())
    }
}

pub fn diff_session_bundle_directories(
    left_dir: impl AsRef<Path>,
    right_dir: impl AsRef<Path>,
) -> Result<SessionBundleDiffReport> {
    diff_session_bundle_directories_inner(left_dir.as_ref(), right_dir.as_ref())
}

pub fn continue_session_bundle_directory(
    store: &mut SqliteSessionStore,
    bundle_dir: impl AsRef<Path>,
    event_hash: Option<&PortableHash>,
) -> Result<SessionBundleContinuationReport> {
    continue_session_bundle_directory_inner(store, bundle_dir.as_ref(), event_hash)
}

#[derive(Clone, Debug)]
struct EventRecordInfo {
    ordinal: u64,
    event_id: EventId,
    branch_id: BranchId,
    event_hash: PortableHash,
    raw_chunk_content_refs: BTreeSet<PortableHash>,
    raw_chunk_refs: BTreeSet<RawChunkEventRef>,
}

#[derive(Clone, Debug)]
struct ExportedRawChunkInfo {
    chunk_ordinal: u64,
    session_id: SessionId,
    branch_id: Option<BranchId>,
    turn_id: Option<bt_core::TurnId>,
    llm_call_ordinal: Option<u32>,
    provider: String,
    stream_name: String,
    content: ContentRef,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RawChunkEventRef {
    local_chunk_id: i64,
    content_hash: PortableHash,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RawChunkImportKey {
    event_id: EventId,
    content_hash: PortableHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RawChunkRefIdentity {
    chunk_ordinal: u64,
    session_id: SessionId,
    branch_id: Option<BranchId>,
    turn_id: Option<bt_core::TurnId>,
    llm_call_ordinal: Option<u32>,
    provider: String,
    stream_name: String,
    content_hash: PortableHash,
    content_size_bytes: u64,
    content_path: Option<String>,
}

pub fn validate_session_bundle_directory(
    bundle_dir: impl AsRef<Path>,
) -> Result<SessionBundleValidationReport> {
    let bundle_dir = bundle_dir.as_ref();
    let manifest: SessionBundleManifest =
        read_json(&bundle_dir.join(SESSION_BT_MANIFEST), "manifest")?;
    if manifest.schema_version != SESSION_BT_SCHEMA_VERSION {
        return Err(invalid_bundle(format!(
            "unsupported manifest schema version {}",
            manifest.schema_version
        )));
    }

    let checksums_path = bundle_dir.join(SESSION_BT_CHECKSUMS);
    let checksums_bytes = fs::read(&checksums_path)?;
    let bundle_hash = hash_bytes(&checksums_bytes);
    if manifest.bundle_hash != bundle_hash {
        return Err(invalid_bundle(format!(
            "manifest bundle_hash {} does not match checksum tree {}",
            manifest.bundle_hash, bundle_hash
        )));
    }
    let checksums: SessionBundleChecksums =
        serde_json::from_slice(&checksums_bytes).map_err(BelltowerError::from)?;
    if checksums.schema_version != SESSION_BT_SCHEMA_VERSION {
        return Err(invalid_bundle(format!(
            "unsupported checksum schema version {}",
            checksums.schema_version
        )));
    }
    validate_bundle_inventory_is_closed(bundle_dir, &manifest, &checksums)?;

    for entry in &checksums.files {
        verify_file_checksum(bundle_dir, entry)?;
    }

    let raw_chunk_refs: Vec<SessionBundleRawChunkRef> = read_jsonl(
        &bundle_dir.join(SESSION_BT_RAW_CHUNK_REFS),
        "raw chunk refs",
    )?;
    let mut raw_chunk_content_hashes = BTreeSet::new();
    for raw_ref in &raw_chunk_refs {
        let Some(path) = raw_ref.content.path.as_deref() else {
            return Err(invalid_bundle(format!(
                "raw chunk ref {} has no content path",
                raw_ref.chunk_ordinal
            )));
        };
        validate_bundle_relative_path(path)?;
        let bytes = fs::read(bundle_dir.join(path))?;
        let hash = hash_bytes(&bytes);
        if hash != raw_ref.content.hash {
            return Err(invalid_bundle(format!(
                "raw chunk {} hash mismatch: expected {}, got {}",
                raw_ref.chunk_ordinal, raw_ref.content.hash, hash
            )));
        }
        if bytes.len() as u64 != raw_ref.content.size_bytes {
            return Err(invalid_bundle(format!(
                "raw chunk {} size mismatch: expected {}, got {}",
                raw_ref.chunk_ordinal,
                raw_ref.content.size_bytes,
                bytes.len()
            )));
        }
        raw_chunk_content_hashes.insert(raw_ref.content.hash.clone());
    }

    let mut manifest_content_hashes = BTreeSet::new();
    for content in &manifest.contents {
        manifest_content_hashes.insert(content.hash.clone());
        if let Some(path) = content.path.as_deref() {
            validate_bundle_relative_path(path)?;
            let bytes = fs::read(bundle_dir.join(path))?;
            let hash = hash_bytes(&bytes);
            if hash != content.hash {
                return Err(invalid_bundle(format!(
                    "content ref {} hash mismatch: got {}",
                    content.hash, hash
                )));
            }
            if bytes.len() as u64 != content.size_bytes {
                return Err(invalid_bundle(format!(
                    "content ref {} size mismatch: expected {}, got {}",
                    content.hash,
                    content.size_bytes,
                    bytes.len()
                )));
            }
        }
    }
    validate_manifest_raw_chunk_inventory(
        &raw_chunk_content_hashes,
        &manifest_content_hashes,
        &manifest.contents,
    )?;

    let event_records: Vec<SessionBundleEventRecord> =
        read_jsonl(&bundle_dir.join(SESSION_BT_EVENTS), "event records")?;
    let event_infos = validate_event_records(
        &event_records,
        &manifest_content_hashes,
        &raw_chunk_content_hashes,
        &manifest,
    )?;
    validate_raw_chunk_refs(&raw_chunk_refs, &manifest, &event_infos)?;
    validate_manifest_branches(&manifest, &event_infos)?;
    validate_event_ranges(&manifest, &event_infos)?;
    validate_manifest_artifacts(&manifest, &manifest_content_hashes, &event_infos)?;

    Ok(SessionBundleValidationReport {
        bundle_hash,
        session_id: manifest.session.session_id,
        event_count: event_records.len(),
        raw_chunk_count: raw_chunk_refs.len(),
        content_count: manifest_content_hashes.len(),
        artifact_count: manifest.artifacts.len(),
    })
}

fn import_session_bundle_directory(
    store: &mut SqliteSessionStore,
    bundle_dir: &Path,
) -> Result<SessionBundleImportReport> {
    let report = validate_session_bundle_directory(bundle_dir)?;
    let manifest: SessionBundleManifest =
        read_json(&bundle_dir.join(SESSION_BT_MANIFEST), "manifest")?;
    if store.load_session(manifest.session.session_id)?.is_some() {
        return Err(BelltowerError::InvalidState(format!(
            "session {} already exists",
            manifest.session.session_id
        )));
    }

    let branches = branch_records_from_manifest(&manifest)?;
    if branches.is_empty() {
        return Err(invalid_bundle("bundle has no branches to import"));
    }

    let raw_chunk_refs: Vec<SessionBundleRawChunkRef> = read_jsonl(
        &bundle_dir.join(SESSION_BT_RAW_CHUNK_REFS),
        "raw chunk refs",
    )?;
    let mut raw_chunk_inserts_by_ordinal = BTreeMap::<u64, RawChunkInsert>::new();
    let mut raw_chunk_import_refs =
        Vec::<(u64, RawChunkImportKey)>::with_capacity(raw_chunk_refs.len());
    for raw_ref in &raw_chunk_refs {
        let branch_id = raw_ref.branch_id.ok_or_else(|| {
            invalid_bundle(format!(
                "raw chunk {} does not include a branch_id",
                raw_ref.chunk_ordinal
            ))
        })?;
        let event_id = raw_ref
            .event_id
            .as_deref()
            .ok_or_else(|| {
                invalid_bundle(format!(
                    "raw chunk {} does not include an event_id",
                    raw_ref.chunk_ordinal
                ))
            })?
            .parse()
            .map_err(|error| {
                invalid_bundle(format!(
                    "raw chunk {} has invalid event_id: {error}",
                    raw_ref.chunk_ordinal
                ))
            })?;
        let path = raw_ref
            .content
            .path
            .as_deref()
            .ok_or_else(|| invalid_bundle("raw chunk content is missing a path"))?;
        validate_bundle_relative_path(path)?;
        let content = fs::read(bundle_dir.join(path))?;
        let key = RawChunkImportKey {
            event_id,
            content_hash: raw_ref.content.hash.clone(),
        };
        raw_chunk_import_refs.push((raw_ref.chunk_ordinal, key));
        let insert = RawChunkInsert {
            session_id: raw_ref.session_id,
            branch_id,
            turn_id: raw_ref.turn_id,
            llm_call_ordinal: raw_ref.llm_call_ordinal,
            event_id: raw_ref.event_id.clone(),
            provider: raw_ref.provider.clone(),
            stream_name: raw_ref.stream_name.clone(),
            content,
        };
        raw_chunk_inserts_by_ordinal
            .entry(raw_ref.chunk_ordinal)
            .or_insert(insert);
    }
    let raw_chunk_ordinals = raw_chunk_inserts_by_ordinal
        .keys()
        .copied()
        .collect::<Vec<_>>();
    let raw_chunk_inserts = raw_chunk_inserts_by_ordinal
        .into_values()
        .collect::<Vec<_>>();

    let event_records: Vec<SessionBundleEventRecord> =
        read_jsonl(&bundle_dir.join(SESSION_BT_EVENTS), "event records")?;
    let (_raw_chunk_ids, seq_ids) = store.import_session_with_raw_chunks_and_events(
        &manifest.session,
        &branches,
        &raw_chunk_inserts,
        |local_raw_chunk_ids| {
            let raw_chunk_local_ids_by_ordinal = raw_chunk_ordinals
                .iter()
                .copied()
                .zip(local_raw_chunk_ids.iter().copied())
                .collect::<HashMap<_, _>>();
            let mut raw_chunk_local_ids_by_ref =
                HashMap::<RawChunkImportKey, i64>::with_capacity(raw_chunk_import_refs.len());
            for (chunk_ordinal, key) in &raw_chunk_import_refs {
                let Some(local_id) = raw_chunk_local_ids_by_ordinal.get(chunk_ordinal).copied()
                else {
                    return Err(invalid_bundle(format!(
                        "raw chunk ordinal {} was not imported",
                        chunk_ordinal
                    )));
                };
                if raw_chunk_local_ids_by_ref
                    .insert(key.clone(), local_id)
                    .is_some()
                {
                    return Err(invalid_bundle(format!(
                        "duplicate raw chunk ref for event {} content {}",
                        key.event_id, key.content_hash
                    )));
                }
            }

            let mut events = Vec::with_capacity(event_records.len());
            for record in &event_records {
                events.push(import_event_value(
                    &record.event,
                    &raw_chunk_local_ids_by_ref,
                )?);
            }
            Ok(events)
        },
    )?;

    Ok(SessionBundleImportReport {
        bundle_hash: report.bundle_hash,
        session_id: manifest.session.session_id,
        branch_count: branches.len(),
        event_count: seq_ids.len(),
        raw_chunk_count: raw_chunk_refs.len(),
    })
}

fn continue_session_bundle_directory_inner(
    store: &mut SqliteSessionStore,
    bundle_dir: &Path,
    event_hash: Option<&PortableHash>,
) -> Result<SessionBundleContinuationReport> {
    let report = validate_session_bundle_directory(bundle_dir)?;
    let manifest: SessionBundleManifest =
        read_json(&bundle_dir.join(SESSION_BT_MANIFEST), "manifest")?;
    let event_records: Vec<SessionBundleEventRecord> =
        read_jsonl(&bundle_dir.join(SESSION_BT_EVENTS), "event records")?;
    let parent_node = resolve_continuation_parent_node(
        &manifest,
        &event_records,
        &report.bundle_hash,
        event_hash,
    )?;

    let imported_session = if store.load_session(manifest.session.session_id)?.is_some() {
        false
    } else {
        import_session_bundle_directory(store, bundle_dir)?;
        true
    };
    assert_parent_node_imported(store, &parent_node)?;

    let branch_id = BranchId::new();
    let branch = BranchRecord {
        branch_id,
        session_id: parent_node.session_id,
        parent_branch_id: Some(parent_node.branch_id),
        parent_event_id: Some(parent_node.event_id),
        head_event_id: None,
        summary: Some(format!(
            "Continuation from bundle {} event {}",
            report.bundle_hash, parent_node.event_hash
        )),
        created_at: OffsetDateTime::now_utc(),
        is_default: false,
    };
    let event = EventEnvelope::new(
        parent_node.session_id,
        branch_id,
        SpanKind::Chain,
        EventPayload::BranchCreated {
            parent_branch_id: Some(parent_node.branch_id),
            parent_event_id: Some(parent_node.event_id),
        },
    )
    .with_attribute(
        SESSION_BT_PARENT_NODE_REF_ATTR,
        serde_json::to_value(&parent_node)?,
    );
    store.create_branch_with_event_and_default(&branch, &event)?;

    Ok(SessionBundleContinuationReport {
        bundle_hash: report.bundle_hash,
        session_id: parent_node.session_id,
        branch_id,
        parent_branch_id: parent_node.branch_id,
        parent_event_id: parent_node.event_id,
        parent_event_hash: parent_node.event_hash,
        imported_session,
    })
}

fn resolve_continuation_parent_node(
    manifest: &SessionBundleManifest,
    event_records: &[SessionBundleEventRecord],
    bundle_hash: &PortableHash,
    event_hash: Option<&PortableHash>,
) -> Result<SessionNodeRef> {
    let record = if let Some(event_hash) = event_hash {
        event_records
            .iter()
            .find(|record| &record.event_hash == event_hash)
            .ok_or_else(|| invalid_bundle(format!("event hash {event_hash} not found")))?
    } else {
        let head_hash = manifest
            .branches
            .iter()
            .find(|branch| branch.is_default)
            .and_then(|branch| branch.head.as_ref())
            .or_else(|| {
                manifest
                    .branches
                    .iter()
                    .find_map(|branch| branch.head.as_ref())
            })
            .map(|head| &head.event_hash)
            .ok_or_else(|| invalid_bundle("bundle has no branch head to continue from"))?;
        event_records
            .iter()
            .find(|record| &record.event_hash == head_hash)
            .ok_or_else(|| {
                invalid_bundle(format!("branch head event hash {head_hash} not found"))
            })?
    };
    Ok(SessionNodeRef {
        bundle_hash: Some(bundle_hash.clone()),
        session_id: portable_session_id(&record.event)?,
        branch_id: portable_branch_id(&record.event)?,
        bundle_event_ordinal: record.bundle_event_ordinal,
        store_seq_id: None,
        event_id: portable_event_id(&record.event)?,
        event_hash: record.event_hash.clone(),
    })
}

fn assert_parent_node_imported(
    store: &SqliteSessionStore,
    parent_node: &SessionNodeRef,
) -> Result<()> {
    let branch = store
        .load_branch(parent_node.session_id, parent_node.branch_id)?
        .ok_or_else(|| {
            BelltowerError::InvalidState(format!(
                "bundle parent branch {} was not imported",
                parent_node.branch_id
            ))
        })?;
    let raw_chunk_hashes_by_local_id = store
        .load_all_raw_chunks(parent_node.session_id)?
        .into_iter()
        .map(|chunk| (chunk.chunk_id, hash_bytes(&chunk.content)))
        .collect::<HashMap<_, _>>();
    let matched_event = store
        .load_all_events(parent_node.session_id)?
        .into_iter()
        .find(|event| {
            event.branch_id == branch.branch_id && event.event_id == parent_node.event_id
        });
    let Some(event) = matched_event else {
        return Err(BelltowerError::InvalidState(format!(
            "bundle parent event {} was not imported",
            parent_node.event_id
        )));
    };
    let portable_event = portable_event_value(&event)?;
    let local_event_hash = canonical_event_hash(&portable_event, &raw_chunk_hashes_by_local_id)?;
    if local_event_hash != parent_node.event_hash {
        return Err(BelltowerError::InvalidState(format!(
            "local parent event {} hash {} does not match bundle parent hash {}",
            parent_node.event_id, local_event_hash, parent_node.event_hash
        )));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct DiffBundle {
    manifest: SessionBundleManifest,
    event_hashes: Vec<PortableHash>,
    branch_ids: BTreeSet<BranchId>,
    content_hashes: BTreeSet<PortableHash>,
    artifact_ids: BTreeSet<String>,
}

fn diff_session_bundle_directories_inner(
    left_dir: &Path,
    right_dir: &Path,
) -> Result<SessionBundleDiffReport> {
    let left_report = validate_session_bundle_directory(left_dir)?;
    let right_report = validate_session_bundle_directory(right_dir)?;
    let left = read_diff_bundle(left_dir)?;
    let right = read_diff_bundle(right_dir)?;

    let common_event_count = common_prefix_len(&left.event_hashes, &right.event_hashes);
    let left_only_event_count = left.event_hashes.len().saturating_sub(common_event_count);
    let right_only_event_count = right.event_hashes.len().saturating_sub(common_event_count);
    let left_only_branch_ids = set_difference(&left.branch_ids, &right.branch_ids);
    let right_only_branch_ids = set_difference(&right.branch_ids, &left.branch_ids);
    let left_only_content_hashes = set_difference(&left.content_hashes, &right.content_hashes);
    let right_only_content_hashes = set_difference(&right.content_hashes, &left.content_hashes);
    let left_only_artifact_ids = set_difference(&left.artifact_ids, &right.artifact_ids);
    let right_only_artifact_ids = set_difference(&right.artifact_ids, &left.artifact_ids);
    let redaction_changed = left.manifest.redaction != right.manifest.redaction;

    let relationship = classify_bundle_diff(
        &left.event_hashes,
        &right.event_hashes,
        common_event_count,
        redaction_changed,
        left_only_branch_ids.is_empty()
            && right_only_branch_ids.is_empty()
            && left_only_content_hashes.is_empty()
            && right_only_content_hashes.is_empty()
            && left_only_artifact_ids.is_empty()
            && right_only_artifact_ids.is_empty(),
    );

    Ok(SessionBundleDiffReport {
        relationship,
        left: SessionBundleDiffSide {
            bundle_hash: left_report.bundle_hash,
            session_id: left_report.session_id,
            event_count: left_report.event_count,
            branch_count: left.manifest.branches.len(),
            content_count: left_report.content_count,
        },
        right: SessionBundleDiffSide {
            bundle_hash: right_report.bundle_hash,
            session_id: right_report.session_id,
            event_count: right_report.event_count,
            branch_count: right.manifest.branches.len(),
            content_count: right_report.content_count,
        },
        common_event_count,
        left_only_event_count,
        right_only_event_count,
        left_only_branch_ids,
        right_only_branch_ids,
        left_only_content_hashes,
        right_only_content_hashes,
        left_only_artifact_ids,
        right_only_artifact_ids,
        redaction_changed,
    })
}

fn read_diff_bundle(bundle_dir: &Path) -> Result<DiffBundle> {
    let manifest: SessionBundleManifest =
        read_json(&bundle_dir.join(SESSION_BT_MANIFEST), "manifest")?;
    let event_records: Vec<SessionBundleEventRecord> =
        read_jsonl(&bundle_dir.join(SESSION_BT_EVENTS), "event records")?;
    let event_hashes = event_records
        .into_iter()
        .map(|record| record.event_hash)
        .collect::<Vec<_>>();
    let branch_ids = manifest
        .branches
        .iter()
        .map(|branch| branch.branch_id)
        .collect::<BTreeSet<_>>();
    let content_hashes = manifest
        .contents
        .iter()
        .map(|content| content.hash.clone())
        .collect::<BTreeSet<_>>();
    let artifact_ids = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.artifact_id.clone())
        .collect::<BTreeSet<_>>();
    Ok(DiffBundle {
        manifest,
        event_hashes,
        branch_ids,
        content_hashes,
        artifact_ids,
    })
}

fn common_prefix_len(left: &[PortableHash], right: &[PortableHash]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

fn set_difference<T: Clone + Ord>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> Vec<T> {
    left.difference(right).cloned().collect()
}

fn classify_bundle_diff(
    left_event_hashes: &[PortableHash],
    right_event_hashes: &[PortableHash],
    common_event_count: usize,
    redaction_changed: bool,
    same_non_event_inventory: bool,
) -> SessionBundleDiffRelationship {
    if left_event_hashes == right_event_hashes {
        if redaction_changed {
            return SessionBundleDiffRelationship::RedactionDifference;
        }
        if same_non_event_inventory {
            return SessionBundleDiffRelationship::Equivalent;
        }
        return SessionBundleDiffRelationship::StructuralDifference;
    }

    let shorter_len = left_event_hashes.len().min(right_event_hashes.len());
    if common_event_count == shorter_len {
        return SessionBundleDiffRelationship::SameLineageUpdate;
    }
    if common_event_count > 0 {
        return SessionBundleDiffRelationship::ForkDivergence;
    }
    SessionBundleDiffRelationship::DifferentLineage
}

fn export_session_bundle_directory(
    store: &SqliteSessionStore,
    session_id: SessionId,
    output_dir: &Path,
    options: &SessionBundleExportOptions,
) -> Result<SessionBundleManifest> {
    ensure_empty_output_dir(output_dir)?;
    fs::create_dir_all(output_dir.join(SESSION_BT_RAW_CHUNKS_DIR))?;
    fs::create_dir_all(output_dir.join(SESSION_BT_PAYLOADS_DIR))?;
    fs::create_dir_all(output_dir.join(SESSION_BT_ARTIFACTS_DIR))?;

    let session = store
        .load_session(session_id)?
        .ok_or_else(|| BelltowerError::NotFound(format!("session {session_id}")))?;
    let branches = store.load_branches(session_id)?;
    let stored_events = store.load_session_event_log(session_id)?;
    let raw_chunks = store.load_all_raw_chunks(session_id)?;

    let mut checksums = Vec::new();
    let (raw_chunks_by_local_id, raw_chunk_hashes_by_local_id) = prepare_raw_chunks(raw_chunks)?;
    let (event_records, event_infos) =
        build_event_records(&stored_events, &raw_chunk_hashes_by_local_id)?;
    let (raw_chunk_refs, contents) = build_raw_chunk_refs(
        output_dir,
        &raw_chunks_by_local_id,
        &event_infos,
        &mut checksums,
    )?;
    let exported_artifacts = export_artifacts(output_dir, &session, options, &mut checksums)?;

    let events_bytes = jsonl_bytes(&event_records)?;
    checksums.push(write_bundle_file(
        output_dir,
        SESSION_BT_EVENTS,
        &events_bytes,
    )?);
    let raw_refs_bytes = jsonl_bytes(&raw_chunk_refs)?;
    checksums.push(write_bundle_file(
        output_dir,
        SESSION_BT_RAW_CHUNK_REFS,
        &raw_refs_bytes,
    )?);

    checksums.sort_by(|left, right| left.path.cmp(&right.path));
    let checksum_tree = SessionBundleChecksums {
        schema_version: SESSION_BT_SCHEMA_VERSION,
        files: checksums,
    };
    let checksum_bytes = serde_json::to_vec_pretty(&checksum_tree)?;
    let bundle_hash = hash_bytes(&checksum_bytes);
    write_bundle_file(output_dir, SESSION_BT_CHECKSUMS, &checksum_bytes)?;

    let mut manifest = SessionBundleManifest {
        schema_version: SESSION_BT_SCHEMA_VERSION,
        bundle_hash: bundle_hash.clone(),
        session: session.clone(),
        branches: build_branch_manifests(&branches, &event_infos, Some(bundle_hash.clone())),
        event_ranges: build_event_ranges(&branches, &event_infos),
        producer: ProducerInfo {
            name: "belltower".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        redaction: RedactionPolicy::None,
        artifact_mode: options.artifact_mode.clone(),
        contents,
        artifacts: build_artifact_refs(
            &session,
            &branches,
            &event_infos,
            &exported_artifacts,
            bundle_hash.clone(),
        )?,
    };
    manifest.contents.extend(
        exported_artifacts
            .into_iter()
            .map(|artifact| artifact.content),
    );
    manifest
        .contents
        .sort_by(|left, right| left.hash.cmp(&right.hash));
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(output_dir.join(SESSION_BT_MANIFEST), manifest_bytes)?;
    Ok(manifest)
}

fn ensure_empty_output_dir(output_dir: &Path) -> Result<()> {
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(BelltowerError::InvalidState(format!(
                "bundle output path is not a directory: {}",
                output_dir.display()
            )));
        }
        if fs::read_dir(output_dir)?.next().is_some() {
            return Err(BelltowerError::InvalidState(format!(
                "bundle output directory must be empty: {}",
                output_dir.display()
            )));
        }
    } else {
        fs::create_dir_all(output_dir)?;
    }
    Ok(())
}

fn prepare_raw_chunks(
    raw_chunks: Vec<RawChunkRecord>,
) -> Result<(
    HashMap<i64, ExportedRawChunkInfo>,
    HashMap<i64, PortableHash>,
)> {
    let mut raw_chunks_by_local_id = HashMap::with_capacity(raw_chunks.len());
    let mut local_id_to_hash = HashMap::with_capacity(raw_chunks.len());

    for (chunk_ordinal, raw_chunk) in raw_chunks.into_iter().enumerate() {
        let hash = hash_bytes(&raw_chunk.content);
        let relative_path = format!("{SESSION_BT_RAW_CHUNKS_DIR}/{}.bin", hash.sha256_hex());
        let content = ContentRef {
            hash: hash.clone(),
            media_type: Some("application/octet-stream".to_owned()),
            size_bytes: raw_chunk.content.len() as u64,
            path: Some(relative_path.clone()),
        };
        local_id_to_hash.insert(raw_chunk.chunk_id, hash.clone());
        raw_chunks_by_local_id.insert(
            raw_chunk.chunk_id,
            ExportedRawChunkInfo {
                chunk_ordinal: chunk_ordinal as u64,
                session_id: raw_chunk.session_id,
                branch_id: raw_chunk.branch_id,
                turn_id: raw_chunk.turn_id,
                llm_call_ordinal: raw_chunk.llm_call_ordinal,
                provider: raw_chunk.provider,
                stream_name: raw_chunk.stream_name,
                content,
                bytes: raw_chunk.content,
            },
        );
    }

    Ok((raw_chunks_by_local_id, local_id_to_hash))
}

fn build_raw_chunk_refs(
    output_dir: &Path,
    raw_chunks_by_local_id: &HashMap<i64, ExportedRawChunkInfo>,
    event_infos: &[EventRecordInfo],
    checksums: &mut Vec<SessionBundleChecksumEntry>,
) -> Result<(Vec<SessionBundleRawChunkRef>, Vec<ContentRef>)> {
    let ref_count = event_infos
        .iter()
        .map(|info| info.raw_chunk_refs.len())
        .sum();
    let mut refs = Vec::with_capacity(ref_count);
    let mut written_hashes = BTreeSet::new();
    let mut contents_by_hash = BTreeMap::<PortableHash, ContentRef>::new();

    for event_info in event_infos {
        for event_ref in &event_info.raw_chunk_refs {
            let raw_chunk = raw_chunks_by_local_id
                .get(&event_ref.local_chunk_id)
                .ok_or_else(|| {
                    invalid_bundle(format!(
                        "event {} references missing raw chunk {}",
                        event_info.event_id, event_ref.local_chunk_id
                    ))
                })?;
            if raw_chunk.content.hash != event_ref.content_hash {
                return Err(invalid_bundle(format!(
                    "event {} raw chunk {} hash mismatch: expected {}, got {}",
                    event_info.event_id,
                    event_ref.local_chunk_id,
                    event_ref.content_hash,
                    raw_chunk.content.hash
                )));
            }
            if written_hashes.insert(raw_chunk.content.hash.clone()) {
                let Some(relative_path) = raw_chunk.content.path.as_deref() else {
                    return Err(invalid_bundle(format!(
                        "raw chunk {} is missing a content path",
                        event_ref.local_chunk_id
                    )));
                };
                checksums.push(write_bundle_file(
                    output_dir,
                    relative_path,
                    &raw_chunk.bytes,
                )?);
            }
            contents_by_hash
                .entry(raw_chunk.content.hash.clone())
                .or_insert_with(|| raw_chunk.content.clone());
            refs.push(SessionBundleRawChunkRef {
                chunk_ordinal: raw_chunk.chunk_ordinal,
                session_id: raw_chunk.session_id,
                branch_id: raw_chunk.branch_id,
                turn_id: raw_chunk.turn_id,
                llm_call_ordinal: raw_chunk.llm_call_ordinal,
                event_id: Some(event_info.event_id.to_string()),
                provider: raw_chunk.provider.clone(),
                stream_name: raw_chunk.stream_name.clone(),
                content: raw_chunk.content.clone(),
            });
        }
    }

    Ok((refs, contents_by_hash.into_values().collect()))
}

#[derive(Clone, Debug)]
struct ExportedArtifact {
    artifact_id: String,
    kind: String,
    content: ContentRef,
}

fn export_artifacts(
    output_dir: &Path,
    session: &SessionRecord,
    options: &SessionBundleExportOptions,
    checksums: &mut Vec<SessionBundleChecksumEntry>,
) -> Result<Vec<ExportedArtifact>> {
    if matches!(options.artifact_mode, SessionBundleArtifactMode::TraceOnly)
        && !options.artifacts.is_empty()
    {
        return Err(BelltowerError::Config(
            "trace-only session bundle export cannot include artifacts".to_owned(),
        ));
    }

    let mut exported = Vec::with_capacity(options.artifacts.len());
    let mut artifact_ids = BTreeSet::new();
    for artifact in &options.artifacts {
        if artifact.kind.trim().is_empty() {
            return Err(BelltowerError::Config(
                "session bundle artifact kind must not be empty".to_owned(),
            ));
        }
        let (display_name, bytes, media_type) = artifact_source_bytes(session, &artifact.source)?;
        let content_hash = hash_bytes(&bytes);
        let artifact_id = artifact.artifact_id.clone().unwrap_or_else(|| {
            format!(
                "{}-{}",
                sanitize_bundle_filename(&artifact.kind),
                &content_hash.sha256_hex()[..12]
            )
        });
        if !artifact_ids.insert(artifact_id.clone()) {
            return Err(BelltowerError::Config(format!(
                "duplicate session bundle artifact id {artifact_id}"
            )));
        }
        let relative_path = format!(
            "{SESSION_BT_ARTIFACTS_DIR}/{}-{}",
            sanitize_bundle_filename(&artifact_id),
            sanitize_bundle_filename(&display_name)
        );
        checksums.push(write_bundle_file(output_dir, &relative_path, &bytes)?);
        exported.push(ExportedArtifact {
            artifact_id,
            kind: artifact.kind.clone(),
            content: ContentRef {
                hash: content_hash,
                media_type,
                size_bytes: bytes.len() as u64,
                path: Some(relative_path),
            },
        });
    }
    Ok(exported)
}

fn artifact_source_bytes(
    session: &SessionRecord,
    source: &SessionBundleArtifactSource,
) -> Result<(String, Vec<u8>, Option<String>)> {
    match source {
        SessionBundleArtifactSource::Bytes {
            display_name,
            bytes,
            media_type,
        } => Ok((
            display_name.clone(),
            bytes.clone(),
            media_type
                .clone()
                .or_else(|| infer_media_type(display_name)),
        )),
        SessionBundleArtifactSource::File(path) => {
            let resolved = resolve_artifact_path(session, path)?;
            let metadata = fs::symlink_metadata(&resolved)?;
            if !metadata.is_file() {
                return Err(BelltowerError::Config(format!(
                    "session bundle artifact is not a file: {}",
                    resolved.display()
                )));
            }
            let bytes = fs::read(&resolved)?;
            let display_name = resolved
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("artifact")
                .to_owned();
            let media_type = infer_media_type(&display_name);
            Ok((display_name, bytes, media_type))
        }
    }
}

fn resolve_artifact_path(session: &SessionRecord, path: &Path) -> Result<PathBuf> {
    let project_root = Path::new(&session.project_root)
        .canonicalize()
        .map_err(|error| {
            BelltowerError::Config(format!(
                "cannot resolve session project root {}: {error}",
                session.project_root
            ))
        })?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let resolved = candidate.canonicalize().map_err(|error| {
        BelltowerError::Config(format!(
            "cannot resolve session bundle artifact {}: {error}",
            candidate.display()
        ))
    })?;
    if !resolved.starts_with(&project_root) {
        return Err(BelltowerError::Config(format!(
            "session bundle artifact {} escapes project root {}",
            resolved.display(),
            project_root.display()
        )));
    }
    Ok(resolved)
}

fn build_artifact_refs(
    session: &SessionRecord,
    branches: &[BranchRecord],
    event_infos: &[EventRecordInfo],
    artifacts: &[ExportedArtifact],
    bundle_hash: PortableHash,
) -> Result<Vec<BundleArtifactRef>> {
    if artifacts.is_empty() {
        return Ok(Vec::new());
    }
    let source_node = default_artifact_source_node(session, branches, event_infos, bundle_hash)?;
    Ok(artifacts
        .iter()
        .map(|artifact| BundleArtifactRef {
            artifact_id: artifact.artifact_id.clone(),
            kind: artifact.kind.clone(),
            source_node: source_node.clone(),
            content: artifact.content.clone(),
        })
        .collect())
}

fn default_artifact_source_node(
    session: &SessionRecord,
    branches: &[BranchRecord],
    event_infos: &[EventRecordInfo],
    bundle_hash: PortableHash,
) -> Result<SessionNodeRef> {
    let default_branch = branches
        .iter()
        .find(|branch| branch.is_default)
        .or_else(|| branches.first())
        .ok_or_else(|| invalid_bundle("cannot attach artifact without a branch"))?;
    let selected = default_branch
        .head_event_id
        .and_then(|event_id| event_infos.iter().find(|info| info.event_id == event_id))
        .or_else(|| {
            event_infos
                .iter()
                .rev()
                .find(|info| info.branch_id == default_branch.branch_id)
        })
        .or_else(|| event_infos.last())
        .ok_or_else(|| invalid_bundle("cannot attach artifact without an event source node"))?;
    Ok(SessionNodeRef {
        bundle_hash: Some(bundle_hash),
        session_id: session.session_id,
        branch_id: selected.branch_id,
        bundle_event_ordinal: selected.ordinal,
        store_seq_id: None,
        event_id: selected.event_id,
        event_hash: selected.event_hash.clone(),
    })
}

fn infer_media_type(name: &str) -> Option<String> {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    let media_type = match extension.as_str() {
        "json" => "application/json",
        "jsonl" | "ndjson" => "application/x-ndjson",
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "patch" | "diff" => "text/x-patch",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    };
    Some(media_type.to_owned())
}

fn sanitize_bundle_filename(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .to_owned();
    if sanitized.is_empty() {
        "artifact".to_owned()
    } else {
        sanitized
    }
}

fn build_event_records(
    stored_events: &[StoredSessionEvent],
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<(Vec<SessionBundleEventRecord>, Vec<EventRecordInfo>)> {
    let mut records = Vec::with_capacity(stored_events.len());
    let mut infos = Vec::with_capacity(stored_events.len());

    for (ordinal, stored) in stored_events.iter().enumerate() {
        let portable_event = portable_event_value(&stored.event)?;
        let raw_chunk_refs =
            raw_chunk_refs_in_event(&portable_event, raw_chunk_hashes_by_local_id)?;
        let event_value = canonical_event_value(&portable_event, raw_chunk_hashes_by_local_id)?;
        let event_hash = hash_canonical_event_value(&event_value)?;
        let ordinal = ordinal as u64;
        records.push(SessionBundleEventRecord {
            bundle_event_ordinal: ordinal,
            event_kind: stored.event.kind().to_owned(),
            event_hash: event_hash.clone(),
            event: event_value.clone(),
        });
        infos.push(EventRecordInfo {
            ordinal,
            event_id: stored.event.event_id,
            branch_id: stored.event.branch_id,
            event_hash,
            raw_chunk_content_refs: raw_chunk_content_refs_in_event(&event_value)?,
            raw_chunk_refs,
        });
    }

    Ok((records, infos))
}

fn portable_event_value(event: &EventEnvelope) -> Result<Value> {
    let mut value = serde_json::to_value(event)?;
    remove_seq_id(&mut value)?;
    Ok(value)
}

fn remove_seq_id(value: &mut Value) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid_bundle("event envelope did not serialize as an object"))?;
    object.remove("seq_id");
    Ok(())
}

fn canonical_event_hash(
    portable_event: &Value,
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<PortableHash> {
    let value = canonical_event_value(portable_event, raw_chunk_hashes_by_local_id)?;
    hash_canonical_event_value(&value)
}

fn canonical_event_value(
    portable_event: &Value,
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<Value> {
    let mut value = portable_event.clone();
    normalize_raw_chunk_local_ids(&mut value, raw_chunk_hashes_by_local_id)?;
    remove_local_sequence_fields(&mut value);
    Ok(value)
}

fn remove_local_sequence_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("context_boundary_seq_id");
            object.remove("source_seq_id");
            object.remove("first_kept_seq_id");
            object.remove("requested_seq_id");
            object.remove("completed_seq_id");
            for child in object.values_mut() {
                remove_local_sequence_fields(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                remove_local_sequence_fields(child);
            }
        }
        _ => {}
    }
}

fn raw_chunk_refs_in_event(
    portable_event: &Value,
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<BTreeSet<RawChunkEventRef>> {
    let mut local_ids = BTreeSet::new();
    let Some(payload) = portable_event
        .as_object()
        .and_then(|object| object.get("payload"))
        .and_then(Value::as_object)
    else {
        return Ok(BTreeSet::new());
    };

    collect_raw_chunk_local_id(
        payload,
        "CompletionChunk",
        "raw_chunk_index",
        &mut local_ids,
    )?;
    collect_raw_chunk_local_id(payload, "RawChunkPersisted", "chunk_index", &mut local_ids)?;

    local_ids
        .into_iter()
        .map(|local_chunk_id| {
            let content_hash = raw_chunk_hashes_by_local_id
                .get(&local_chunk_id)
                .cloned()
                .ok_or_else(|| {
                    invalid_bundle(format!(
                        "event references missing raw chunk {local_chunk_id}"
                    ))
                })?;
            Ok(RawChunkEventRef {
                local_chunk_id,
                content_hash,
            })
        })
        .collect()
}

fn collect_raw_chunk_local_id(
    payload: &Map<String, Value>,
    variant: &str,
    field: &str,
    local_ids: &mut BTreeSet<i64>,
) -> Result<()> {
    let Some(object) = payload.get(variant).and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(value) = object.get(field) else {
        return Ok(());
    };
    let Some(local_id) = value.as_i64() else {
        return Err(invalid_bundle(format!("{variant} {field} is not an i64")));
    };
    local_ids.insert(local_id);
    Ok(())
}

fn hash_canonical_event_value(value: &Value) -> Result<PortableHash> {
    let bytes = serde_json::to_vec(&value)?;
    Ok(hash_bytes(&bytes))
}

fn normalize_raw_chunk_local_ids(
    portable_event: &mut Value,
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<()> {
    let Some(payload) = portable_event
        .as_object_mut()
        .and_then(|object| object.get_mut("payload"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };

    normalize_completion_chunk_payload(payload, raw_chunk_hashes_by_local_id)?;
    normalize_raw_chunk_persisted_payload(payload, raw_chunk_hashes_by_local_id)?;
    Ok(())
}

fn normalize_completion_chunk_payload(
    payload: &mut Map<String, Value>,
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<()> {
    let Some(chunk) = payload
        .get_mut("CompletionChunk")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let raw_chunk_index = chunk.remove("raw_chunk_index");
    if let Some(Value::Number(number)) = raw_chunk_index {
        let Some(index) = number.as_i64() else {
            return Err(invalid_bundle("completion raw_chunk_index is not an i64"));
        };
        let Some(hash) = raw_chunk_hashes_by_local_id.get(&index) else {
            return Err(invalid_bundle(format!(
                "completion chunk references missing raw chunk {index}"
            )));
        };
        chunk.insert(
            "raw_chunk_content_ref".to_owned(),
            Value::String(hash.to_string()),
        );
    }
    Ok(())
}

fn normalize_raw_chunk_persisted_payload(
    payload: &mut Map<String, Value>,
    raw_chunk_hashes_by_local_id: &HashMap<i64, PortableHash>,
) -> Result<()> {
    let Some(raw) = payload
        .get_mut("RawChunkPersisted")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    let chunk_index = raw.remove("chunk_index");
    if let Some(Value::Number(number)) = chunk_index {
        let Some(index) = number.as_i64() else {
            return Err(invalid_bundle(
                "raw chunk persisted chunk_index is not an i64",
            ));
        };
        let Some(hash) = raw_chunk_hashes_by_local_id.get(&index) else {
            return Err(invalid_bundle(format!(
                "raw chunk persisted event references missing raw chunk {index}"
            )));
        };
        raw.insert(
            "raw_chunk_content_ref".to_owned(),
            Value::String(hash.to_string()),
        );
    }
    Ok(())
}

fn build_branch_manifests(
    branches: &[BranchRecord],
    event_infos: &[EventRecordInfo],
    bundle_hash: Option<PortableHash>,
) -> Vec<BranchManifest> {
    branches
        .iter()
        .map(|branch| {
            let head = branch
                .head_event_id
                .and_then(|event_id| event_infos.iter().find(|info| info.event_id == event_id))
                .or_else(|| {
                    event_infos
                        .iter()
                        .rev()
                        .find(|info| info.branch_id == branch.branch_id)
                })
                .map(|info| SessionNodeRef {
                    bundle_hash: bundle_hash.clone(),
                    session_id: branch.session_id,
                    branch_id: branch.branch_id,
                    bundle_event_ordinal: info.ordinal,
                    store_seq_id: None,
                    event_id: info.event_id,
                    event_hash: info.event_hash.clone(),
                });
            BranchManifest {
                branch_id: branch.branch_id,
                parent_branch_id: branch.parent_branch_id,
                parent_event_id: branch.parent_event_id,
                head,
                is_default: branch.is_default,
                summary: branch.summary.clone(),
            }
        })
        .collect()
}

fn build_event_ranges(
    branches: &[BranchRecord],
    event_infos: &[EventRecordInfo],
) -> Vec<EventRange> {
    let mut ranges = Vec::new();
    let branch_ids = branches
        .iter()
        .map(|branch| branch.branch_id)
        .collect::<BTreeSet<_>>();
    let mut ordered_infos = event_infos.iter().collect::<Vec<_>>();
    ordered_infos.sort_by_key(|info| info.ordinal);
    let Some(mut first) = ordered_infos.first().copied() else {
        return ranges;
    };
    let mut last = first;
    for info in ordered_infos.iter().copied().skip(1) {
        if info.branch_id != first.branch_id || info.ordinal != last.ordinal + 1 {
            if branch_ids.contains(&first.branch_id) {
                ranges.push(event_range_from_run(first, last));
            }
            first = info;
        }
        last = info;
    }
    if branch_ids.contains(&first.branch_id) {
        ranges.push(event_range_from_run(first, last));
    }
    ranges
}

fn event_range_from_run(first: &EventRecordInfo, last: &EventRecordInfo) -> EventRange {
    EventRange {
        branch_id: first.branch_id,
        ordinal_start: first.ordinal,
        ordinal_end: last.ordinal,
        first_event_hash: first.event_hash.clone(),
        last_event_hash: last.event_hash.clone(),
        exported_store_seq_start: None,
        exported_store_seq_end: None,
    }
}

fn branch_records_from_manifest(manifest: &SessionBundleManifest) -> Result<Vec<BranchRecord>> {
    let mut branches = Vec::with_capacity(manifest.branches.len());
    for branch in &manifest.branches {
        branches.push(BranchRecord {
            branch_id: branch.branch_id,
            session_id: manifest.session.session_id,
            parent_branch_id: branch.parent_branch_id,
            parent_event_id: branch.parent_event_id,
            head_event_id: branch.head.as_ref().map(|head| head.event_id),
            summary: branch.summary.clone(),
            created_at: manifest.session.created_at,
            is_default: branch.is_default,
        });
    }
    Ok(branches)
}

fn validate_event_records(
    event_records: &[SessionBundleEventRecord],
    manifest_content_hashes: &BTreeSet<PortableHash>,
    raw_chunk_content_hashes: &BTreeSet<PortableHash>,
    manifest: &SessionBundleManifest,
) -> Result<Vec<EventRecordInfo>> {
    let mut infos = Vec::with_capacity(event_records.len());
    let mut event_ids = BTreeSet::new();
    let raw_chunk_hashes_by_local_id = HashMap::new();
    for (expected_ordinal, record) in event_records.iter().enumerate() {
        let expected_ordinal = expected_ordinal as u64;
        if record.bundle_event_ordinal != expected_ordinal {
            return Err(invalid_bundle(format!(
                "event ordinal gap: expected {}, got {}",
                expected_ordinal, record.bundle_event_ordinal
            )));
        }
        let event_hash = canonical_event_hash(&record.event, &raw_chunk_hashes_by_local_id)?;
        if event_hash != record.event_hash {
            return Err(invalid_bundle(format!(
                "event {} hash mismatch: expected {}, got {}",
                record.bundle_event_ordinal, record.event_hash, event_hash
            )));
        }
        let payload_kind = portable_event_kind(&record.event)?;
        if record.event_kind != payload_kind {
            return Err(invalid_bundle(format!(
                "event {} kind mismatch: declared {}, payload {}",
                record.bundle_event_ordinal, record.event_kind, payload_kind
            )));
        }
        let content_refs = raw_chunk_content_refs_in_event(&record.event)?;
        for content_ref in &content_refs {
            if !manifest_content_hashes.contains(&content_ref) {
                return Err(invalid_bundle(format!(
                    "event {} references raw chunk content {} missing from manifest contents",
                    record.bundle_event_ordinal, content_ref
                )));
            }
            if !raw_chunk_content_hashes.contains(&content_ref) {
                return Err(invalid_bundle(format!(
                    "event {} references raw chunk content {} missing from raw_chunk_refs",
                    record.bundle_event_ordinal, content_ref
                )));
            }
        }
        let event_id = portable_event_id(&record.event)?;
        if !event_ids.insert(event_id) {
            return Err(invalid_bundle(format!("duplicate event_id {}", event_id)));
        }
        let event_session_id = portable_session_id(&record.event)?;
        let branch_id = portable_branch_id(&record.event)?;
        if event_session_id != manifest.session.session_id {
            return Err(invalid_bundle(format!(
                "event {} belongs to session {}, expected {}",
                record.bundle_event_ordinal, event_session_id, manifest.session.session_id
            )));
        }
        infos.push(EventRecordInfo {
            ordinal: record.bundle_event_ordinal,
            event_id,
            branch_id,
            event_hash,
            raw_chunk_content_refs: content_refs,
            raw_chunk_refs: BTreeSet::new(),
        });
    }
    Ok(infos)
}

fn raw_chunk_content_refs_in_event(value: &Value) -> Result<BTreeSet<PortableHash>> {
    let mut refs = BTreeSet::new();
    collect_raw_chunk_content_refs(value, &mut refs)?;
    Ok(refs)
}

fn collect_raw_chunk_content_refs(value: &Value, refs: &mut BTreeSet<PortableHash>) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if key == "raw_chunk_content_ref" {
                    let Some(raw_ref) = child.as_str() else {
                        return Err(invalid_bundle("raw_chunk_content_ref is not a string"));
                    };
                    refs.insert(PortableHash::new(raw_ref.to_owned()).map_err(|error| {
                        invalid_bundle(format!("invalid raw_chunk_content_ref {raw_ref}: {error}"))
                    })?);
                }
                collect_raw_chunk_content_refs(child, refs)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_raw_chunk_content_refs(child, refs)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_portable_event_value(value: &Value) -> Result<EventEnvelope> {
    let mut value = value.clone();
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid_bundle("event record is not an object"))?;
    object.insert("seq_id".to_owned(), Value::Null);
    serde_json::from_value(value).map_err(BelltowerError::from)
}

fn import_event_value(
    value: &Value,
    raw_chunk_local_ids_by_ref: &HashMap<RawChunkImportKey, i64>,
) -> Result<EventEnvelope> {
    let event_id = portable_event_id(value)?;
    let mut value = value.clone();
    denormalize_raw_chunk_content_refs(&mut value, event_id, raw_chunk_local_ids_by_ref)?;
    parse_portable_event_value(&value)
}

fn denormalize_raw_chunk_content_refs(
    portable_event: &mut Value,
    event_id: EventId,
    raw_chunk_local_ids_by_ref: &HashMap<RawChunkImportKey, i64>,
) -> Result<()> {
    let Some(payload) = portable_event
        .as_object_mut()
        .and_then(|object| object.get_mut("payload"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };

    denormalize_completion_chunk_payload(payload, event_id, raw_chunk_local_ids_by_ref)?;
    denormalize_raw_chunk_persisted_payload(payload, event_id, raw_chunk_local_ids_by_ref)?;
    Ok(())
}

fn denormalize_completion_chunk_payload(
    payload: &mut Map<String, Value>,
    event_id: EventId,
    raw_chunk_local_ids_by_ref: &HashMap<RawChunkImportKey, i64>,
) -> Result<()> {
    let Some(chunk) = payload
        .get_mut("CompletionChunk")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    if let Some(chunk_id) = take_raw_chunk_content_ref(
        chunk,
        event_id,
        raw_chunk_local_ids_by_ref,
        "completion chunk",
    )? {
        chunk.insert(
            "raw_chunk_index".to_owned(),
            Value::Number(serde_json::Number::from(chunk_id)),
        );
    }
    Ok(())
}

fn denormalize_raw_chunk_persisted_payload(
    payload: &mut Map<String, Value>,
    event_id: EventId,
    raw_chunk_local_ids_by_ref: &HashMap<RawChunkImportKey, i64>,
) -> Result<()> {
    let Some(raw) = payload
        .get_mut("RawChunkPersisted")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    if let Some(chunk_id) = take_raw_chunk_content_ref(
        raw,
        event_id,
        raw_chunk_local_ids_by_ref,
        "raw chunk persisted event",
    )? {
        raw.insert(
            "chunk_index".to_owned(),
            Value::Number(serde_json::Number::from(chunk_id)),
        );
    }
    Ok(())
}

fn take_raw_chunk_content_ref(
    object: &mut Map<String, Value>,
    event_id: EventId,
    raw_chunk_local_ids_by_ref: &HashMap<RawChunkImportKey, i64>,
    label: &str,
) -> Result<Option<i64>> {
    let Some(value) = object.remove("raw_chunk_content_ref") else {
        return Ok(None);
    };
    let Some(raw_ref) = value.as_str() else {
        return Err(invalid_bundle(format!(
            "{label} raw_chunk_content_ref is not a string"
        )));
    };
    let hash = PortableHash::new(raw_ref.to_owned()).map_err(|error| {
        invalid_bundle(format!("invalid raw_chunk_content_ref {raw_ref}: {error}"))
    })?;
    let key = RawChunkImportKey {
        event_id,
        content_hash: hash.clone(),
    };
    let Some(chunk_id) = raw_chunk_local_ids_by_ref.get(&key).copied() else {
        return Err(invalid_bundle(format!(
            "{label} event {event_id} references raw chunk content {hash} that was not imported"
        )));
    };
    Ok(Some(chunk_id))
}

fn portable_event_id(value: &Value) -> Result<EventId> {
    let raw_event_id = portable_string_field(value, "event_id")?;
    raw_event_id.parse().map_err(|error| {
        invalid_bundle(format!(
            "portable event has invalid event_id {raw_event_id}: {error}"
        ))
    })
}

fn portable_session_id(value: &Value) -> Result<SessionId> {
    let raw_session_id = portable_string_field(value, "session_id")?;
    raw_session_id.parse().map_err(|error| {
        invalid_bundle(format!(
            "portable event has invalid session_id {raw_session_id}: {error}"
        ))
    })
}

fn portable_branch_id(value: &Value) -> Result<BranchId> {
    let raw_branch_id = portable_string_field(value, "branch_id")?;
    raw_branch_id.parse().map_err(|error| {
        invalid_bundle(format!(
            "portable event has invalid branch_id {raw_branch_id}: {error}"
        ))
    })
}

fn portable_event_kind(value: &Value) -> Result<&'static str> {
    let payload = value
        .as_object()
        .and_then(|object| object.get("payload"))
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_bundle("portable event is missing payload"))?;
    if payload.len() != 1 {
        return Err(invalid_bundle(
            "portable event payload must contain exactly one variant",
        ));
    }
    let variant = payload.keys().next().expect("payload has one key").as_str();
    let kind = match variant {
        "SessionStarted" => "session.started",
        "SessionSpawnRequested" => "session.spawn.requested",
        "SessionSpawned" => "session.spawned",
        "SessionHandoffRecorded" => "session.handoff.recorded",
        "SessionResultImported" => "session.result.imported",
        "SessionResultRejected" => "session.result.rejected",
        "RelatedSessionMessageRecorded" => "session.related_message.recorded",
        "RelatedSessionMessageResolved" => "session.related_message.resolved",
        "SessionSettingsUpdated" => "session.settings.updated",
        "SessionEnded" => "session.ended",
        "BranchCreated" => "branch.created",
        "BranchActivated" => "branch.activated",
        "BranchSummarized" => "branch.summarized",
        "MessageAppended" => "message.appended",
        "TurnStarted" => "turn.started",
        "TurnInstructionProvenanceRecorded" => "turn.instructions.recorded",
        "TurnContextManifestRecorded" => "turn.context_manifest.recorded",
        "CompletionRequested" => "completion.requested",
        "CompletionChunk" => "completion.chunk",
        "CompletionFinished" => "completion.finished",
        "SessionError" => "session.error",
        "TurnFinished" => "turn.finished",
        "ToolCallRequested" => "tool.call.requested",
        "ToolOperationRecorded" => "tool.operation.recorded",
        "ToolApprovalRequested" => "tool.approval.requested",
        "ToolApprovalResolved" => "tool.approval.resolved",
        "ToolExecutionFinished" => "tool.execution.finished",
        "PlanUpdated" => "plan.updated",
        "BudgetConfigured" => "budget.configured",
        "ContextCompacted" => "context.compacted",
        "BudgetCheckpoint" => "budget.checkpoint",
        "SessionQueuedMessageEnqueued" => "session.queued_message.enqueued",
        "SessionQueuedMessageResolved" => "session.queued_message.resolved",
        "SessionCancelled" => "session.cancelled",
        "SessionCancelCleared" => "session.cancel.cleared",
        "SessionSteered" => "session.steered",
        "SessionSteersResolved" => "session.steers.resolved",
        "OperatorCommandRecorded" => "operator.command.recorded",
        "RawChunkPersisted" => "raw_chunk.persisted",
        _ => {
            return Err(invalid_bundle(format!(
                "unknown event payload variant {variant}"
            )));
        }
    };
    Ok(kind)
}

fn portable_string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_bundle(format!("portable event is missing {field}")))
}

fn validate_raw_chunk_refs(
    raw_chunk_refs: &[SessionBundleRawChunkRef],
    manifest: &SessionBundleManifest,
    event_infos: &[EventRecordInfo],
) -> Result<()> {
    let branch_ids = manifest
        .branches
        .iter()
        .map(|branch| branch.branch_id)
        .collect::<BTreeSet<_>>();
    let event_infos_by_id = event_infos
        .iter()
        .map(|info| (info.event_id, info))
        .collect::<BTreeMap<_, _>>();
    let mut raw_chunk_identities = BTreeMap::<u64, RawChunkRefIdentity>::new();
    let mut refs_by_event_content = BTreeSet::new();

    for raw_ref in raw_chunk_refs {
        if raw_ref.session_id != manifest.session.session_id {
            return Err(invalid_bundle(format!(
                "raw chunk {} belongs to session {}, expected {}",
                raw_ref.chunk_ordinal, raw_ref.session_id, manifest.session.session_id
            )));
        }
        let Some(branch_id) = raw_ref.branch_id else {
            return Err(invalid_bundle(format!(
                "raw chunk {} does not include a branch_id",
                raw_ref.chunk_ordinal
            )));
        };
        if !branch_ids.contains(&branch_id) {
            return Err(invalid_bundle(format!(
                "raw chunk {} references missing branch {}",
                raw_ref.chunk_ordinal, branch_id
            )));
        }
        let identity = RawChunkRefIdentity {
            chunk_ordinal: raw_ref.chunk_ordinal,
            session_id: raw_ref.session_id,
            branch_id: raw_ref.branch_id,
            turn_id: raw_ref.turn_id,
            llm_call_ordinal: raw_ref.llm_call_ordinal,
            provider: raw_ref.provider.clone(),
            stream_name: raw_ref.stream_name.clone(),
            content_hash: raw_ref.content.hash.clone(),
            content_size_bytes: raw_ref.content.size_bytes,
            content_path: raw_ref.content.path.clone(),
        };
        if let Some(existing) = raw_chunk_identities.insert(raw_ref.chunk_ordinal, identity.clone())
            && existing != identity
        {
            return Err(invalid_bundle(format!(
                "raw chunk {} has inconsistent refs",
                raw_ref.chunk_ordinal
            )));
        }
        let event_id: EventId = raw_ref
            .event_id
            .as_deref()
            .ok_or_else(|| {
                invalid_bundle(format!(
                    "raw chunk {} does not include an event_id",
                    raw_ref.chunk_ordinal
                ))
            })?
            .parse()
            .map_err(|error| {
                invalid_bundle(format!(
                    "raw chunk {} has invalid event_id: {error}",
                    raw_ref.chunk_ordinal
                ))
            })?;
        let Some(event_info) = event_infos_by_id.get(&event_id) else {
            return Err(invalid_bundle(format!(
                "raw chunk {} references missing event {}",
                raw_ref.chunk_ordinal, event_id
            )));
        };
        if event_info.branch_id != branch_id {
            return Err(invalid_bundle(format!(
                "raw chunk {} branch {} does not match event {} branch {}",
                raw_ref.chunk_ordinal, branch_id, event_id, event_info.branch_id
            )));
        }
        if !event_info
            .raw_chunk_content_refs
            .contains(&raw_ref.content.hash)
        {
            return Err(invalid_bundle(format!(
                "raw chunk {} content {} is not referenced by event {}",
                raw_ref.chunk_ordinal, raw_ref.content.hash, event_id
            )));
        }
        if !refs_by_event_content.insert((event_id, raw_ref.content.hash.clone())) {
            return Err(invalid_bundle(format!(
                "duplicate raw chunk ref for event {} content {}",
                event_id, raw_ref.content.hash
            )));
        }
    }
    for event_info in event_infos {
        for content_hash in &event_info.raw_chunk_content_refs {
            if !refs_by_event_content.contains(&(event_info.event_id, content_hash.clone())) {
                return Err(invalid_bundle(format!(
                    "event {} references raw chunk content {} missing from raw_chunk_refs for that event",
                    event_info.event_id, content_hash
                )));
            }
        }
    }
    Ok(())
}

fn validate_manifest_branches(
    manifest: &SessionBundleManifest,
    event_infos: &[EventRecordInfo],
) -> Result<()> {
    let mut branch_ids = BTreeSet::new();
    let mut default_branch_count = 0usize;
    for branch in &manifest.branches {
        if !branch_ids.insert(branch.branch_id) {
            return Err(invalid_bundle(format!(
                "duplicate branch_id {}",
                branch.branch_id
            )));
        }
        if branch.is_default {
            default_branch_count += 1;
        }
    }
    if default_branch_count != 1 {
        return Err(invalid_bundle(format!(
            "bundle must have exactly one default branch, found {}",
            default_branch_count
        )));
    }
    let branch_ids = manifest
        .branches
        .iter()
        .map(|branch| branch.branch_id)
        .collect::<BTreeSet<_>>();
    let event_infos_by_id = event_infos
        .iter()
        .map(|info| (info.event_id, info))
        .collect::<BTreeMap<_, _>>();
    let mut latest_info_by_branch = BTreeMap::<BranchId, &EventRecordInfo>::new();
    for info in event_infos {
        latest_info_by_branch.insert(info.branch_id, info);
    }

    for branch in &manifest.branches {
        if let Some(parent_branch_id) = branch.parent_branch_id
            && !branch_ids.contains(&parent_branch_id)
        {
            return Err(invalid_bundle(format!(
                "branch {} references missing parent branch {}",
                branch.branch_id, parent_branch_id
            )));
        }
        if let Some(parent_event_id) = branch.parent_event_id {
            let Some(parent_info) = event_infos_by_id.get(&parent_event_id) else {
                return Err(invalid_bundle(format!(
                    "branch {} references missing parent event {}",
                    branch.branch_id, parent_event_id
                )));
            };
            if let Some(parent_branch_id) = branch.parent_branch_id
                && parent_info.branch_id != parent_branch_id
            {
                return Err(invalid_bundle(format!(
                    "branch {} parent event {} belongs to branch {}, expected {}",
                    branch.branch_id, parent_event_id, parent_info.branch_id, parent_branch_id
                )));
            }
        }
        if let Some(head) = &branch.head {
            if head.session_id != manifest.session.session_id {
                return Err(invalid_bundle(format!(
                    "branch {} head points to session {}",
                    branch.branch_id, head.session_id
                )));
            }
            if head.branch_id != branch.branch_id {
                return Err(invalid_bundle(format!(
                    "branch {} head points to branch {}",
                    branch.branch_id, head.branch_id
                )));
            }
            if head.bundle_hash.as_ref() != Some(&manifest.bundle_hash) {
                return Err(invalid_bundle(format!(
                    "branch {} head has wrong bundle hash",
                    branch.branch_id
                )));
            }
            if head.store_seq_id.is_some() {
                return Err(invalid_bundle(format!(
                    "branch {} head carries a local store_seq_id",
                    branch.branch_id
                )));
            }
            let Some(head_info) = event_infos_by_id.get(&head.event_id) else {
                return Err(invalid_bundle(format!(
                    "branch {} head references missing event {}",
                    branch.branch_id, head.event_id
                )));
            };
            if head_info.branch_id != branch.branch_id
                || head_info.ordinal != head.bundle_event_ordinal
                || head_info.event_hash != head.event_hash
            {
                return Err(invalid_bundle(format!(
                    "branch {} head does not match event {} tuple",
                    branch.branch_id, head.event_id
                )));
            }
            if latest_info_by_branch
                .get(&branch.branch_id)
                .is_some_and(|latest| latest.event_id != head.event_id)
            {
                return Err(invalid_bundle(format!(
                    "branch {} head is not the branch tip",
                    branch.branch_id
                )));
            }
        } else if latest_info_by_branch.contains_key(&branch.branch_id) {
            return Err(invalid_bundle(format!(
                "branch {} has events but no head",
                branch.branch_id
            )));
        }
    }
    for info in event_infos {
        if !branch_ids.contains(&info.branch_id) {
            return Err(invalid_bundle(format!(
                "event {} references missing branch {}",
                info.ordinal, info.branch_id
            )));
        }
    }
    Ok(())
}

fn validate_event_ranges(
    manifest: &SessionBundleManifest,
    event_infos: &[EventRecordInfo],
) -> Result<()> {
    let info_by_ordinal = event_infos
        .iter()
        .map(|info| (info.ordinal, info))
        .collect::<BTreeMap<_, _>>();

    let mut covered_ordinals = BTreeSet::new();
    for range in &manifest.event_ranges {
        if range.ordinal_start > range.ordinal_end {
            return Err(invalid_bundle(format!(
                "range for branch {} has invalid ordinal bounds {}..{}",
                range.branch_id, range.ordinal_start, range.ordinal_end
            )));
        }
        let Some(first) = info_by_ordinal.get(&range.ordinal_start) else {
            return Err(invalid_bundle(format!(
                "range for branch {} starts at missing ordinal {}",
                range.branch_id, range.ordinal_start
            )));
        };
        let Some(last) = info_by_ordinal.get(&range.ordinal_end) else {
            return Err(invalid_bundle(format!(
                "range for branch {} ends at missing ordinal {}",
                range.branch_id, range.ordinal_end
            )));
        };
        for ordinal in range.ordinal_start..=range.ordinal_end {
            let Some(info) = info_by_ordinal.get(&ordinal) else {
                return Err(invalid_bundle(format!(
                    "range for branch {} includes missing ordinal {}",
                    range.branch_id, ordinal
                )));
            };
            if info.branch_id != range.branch_id {
                return Err(invalid_bundle(format!(
                    "range for branch {} crosses branch boundary at ordinal {}",
                    range.branch_id, ordinal
                )));
            }
            if !covered_ordinals.insert(ordinal) {
                return Err(invalid_bundle(format!(
                    "event ordinal {} is covered by more than one range",
                    ordinal
                )));
            }
        }
        if first.event_hash != range.first_event_hash || last.event_hash != range.last_event_hash {
            return Err(invalid_bundle(format!(
                "range for branch {} has mismatched boundary hashes",
                range.branch_id
            )));
        }
        if range.exported_store_seq_start.is_some() || range.exported_store_seq_end.is_some() {
            return Err(invalid_bundle(format!(
                "range for branch {} carries local exported seq ids",
                range.branch_id
            )));
        }
    }
    for info in event_infos {
        if !covered_ordinals.contains(&info.ordinal) {
            return Err(invalid_bundle(format!(
                "event ordinal {} is not covered by an event range",
                info.ordinal
            )));
        }
    }
    Ok(())
}

fn validate_manifest_raw_chunk_inventory(
    raw_chunk_content_hashes: &BTreeSet<PortableHash>,
    manifest_content_hashes: &BTreeSet<PortableHash>,
    contents: &[ContentRef],
) -> Result<()> {
    for hash in raw_chunk_content_hashes {
        if !manifest_content_hashes.contains(hash) {
            return Err(invalid_bundle(format!(
                "raw chunk content {hash} is missing from manifest contents"
            )));
        }
    }
    for content in contents {
        if let Some(path) = content.path.as_deref()
            && path.split('/').next() == Some(SESSION_BT_RAW_CHUNKS_DIR)
            && !raw_chunk_content_hashes.contains(&content.hash)
        {
            return Err(invalid_bundle(format!(
                "manifest raw chunk content {} is missing from raw_chunk_refs",
                content.hash
            )));
        }
    }
    Ok(())
}

fn validate_manifest_artifacts(
    manifest: &SessionBundleManifest,
    manifest_content_hashes: &BTreeSet<PortableHash>,
    event_infos: &[EventRecordInfo],
) -> Result<()> {
    if matches!(manifest.artifact_mode, SessionBundleArtifactMode::TraceOnly)
        && !manifest.artifacts.is_empty()
    {
        return Err(invalid_bundle(
            "trace_only artifact mode must not include artifact refs",
        ));
    }

    let event_keys = event_infos
        .iter()
        .map(|info| {
            (
                info.branch_id,
                info.event_id,
                info.ordinal,
                info.event_hash.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let mut artifact_ids = BTreeSet::new();
    for artifact in &manifest.artifacts {
        if artifact.artifact_id.trim().is_empty() {
            return Err(invalid_bundle("artifact id must not be empty"));
        }
        if artifact.kind.trim().is_empty() {
            return Err(invalid_bundle(format!(
                "artifact {} kind must not be empty",
                artifact.artifact_id
            )));
        }
        if !artifact_ids.insert(artifact.artifact_id.clone()) {
            return Err(invalid_bundle(format!(
                "duplicate artifact id {}",
                artifact.artifact_id
            )));
        }
        if !manifest_content_hashes.contains(&artifact.content.hash) {
            return Err(invalid_bundle(format!(
                "artifact {} content {} missing from manifest contents",
                artifact.artifact_id, artifact.content.hash
            )));
        }
        let path = artifact.content.path.as_deref().ok_or_else(|| {
            invalid_bundle(format!(
                "artifact {} has no content path",
                artifact.artifact_id
            ))
        })?;
        validate_bundle_relative_path(path)?;
        if path.split('/').next() != Some(SESSION_BT_ARTIFACTS_DIR) {
            return Err(invalid_bundle(format!(
                "artifact {} content path must be under {SESSION_BT_ARTIFACTS_DIR}/",
                artifact.artifact_id
            )));
        }
        let Some(content) = manifest.contents.iter().find(|content| {
            content.hash == artifact.content.hash && content.path == artifact.content.path
        }) else {
            return Err(invalid_bundle(format!(
                "artifact {} content {} missing from manifest contents",
                artifact.artifact_id, artifact.content.hash
            )));
        };
        if content.path != artifact.content.path
            || content.size_bytes != artifact.content.size_bytes
        {
            return Err(invalid_bundle(format!(
                "artifact {} content ref does not match manifest contents",
                artifact.artifact_id
            )));
        }
        let source = &artifact.source_node;
        if source.bundle_hash.as_ref() != Some(&manifest.bundle_hash) {
            return Err(invalid_bundle(format!(
                "artifact {} source node has wrong bundle hash",
                artifact.artifact_id
            )));
        }
        if source.session_id != manifest.session.session_id {
            return Err(invalid_bundle(format!(
                "artifact {} source node points to session {}",
                artifact.artifact_id, source.session_id
            )));
        }
        if source.store_seq_id.is_some() {
            return Err(invalid_bundle(format!(
                "artifact {} source node carries a local store_seq_id",
                artifact.artifact_id
            )));
        }
        let key = (
            source.branch_id,
            source.event_id,
            source.bundle_event_ordinal,
            source.event_hash.clone(),
        );
        if !event_keys.contains(&key) {
            return Err(invalid_bundle(format!(
                "artifact {} source node does not resolve to an event",
                artifact.artifact_id
            )));
        }
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, label: &str) -> Result<T> {
    let bytes = fs::read(path).map_err(|error| {
        BelltowerError::Protocol(format!(
            "failed to read {label} at {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(BelltowerError::from)
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path, label: &str) -> Result<Vec<T>> {
    let content = fs::read_to_string(path).map_err(|error| {
        BelltowerError::Protocol(format!(
            "failed to read {label} at {}: {error}",
            path.display()
        ))
    })?;
    let mut items = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let item = serde_json::from_str(line).map_err(|error| {
            BelltowerError::Protocol(format!(
                "failed to parse {label} line {} in {}: {error}",
                index + 1,
                path.display()
            ))
        })?;
        items.push(item);
    }
    Ok(items)
}

fn jsonl_bytes<T: Serialize>(items: &[T]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for item in items {
        serde_json::to_writer(&mut bytes, item)?;
        bytes.write_all(b"\n")?;
    }
    Ok(bytes)
}

fn write_bundle_file(
    output_dir: &Path,
    relative_path: &str,
    bytes: &[u8],
) -> Result<SessionBundleChecksumEntry> {
    validate_bundle_relative_path(relative_path)?;
    let path = output_dir.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, bytes)?;
    Ok(SessionBundleChecksumEntry {
        path: relative_path.to_owned(),
        hash: hash_bytes(bytes),
        size_bytes: bytes.len() as u64,
    })
}

fn verify_file_checksum(bundle_dir: &Path, entry: &SessionBundleChecksumEntry) -> Result<()> {
    validate_bundle_relative_path(&entry.path)?;
    let path = bundle_dir.join(&entry.path);
    let bytes = fs::read(&path).map_err(|error| {
        invalid_bundle(format!(
            "checksum entry {} cannot be read: {error}",
            entry.path
        ))
    })?;
    let hash = hash_bytes(&bytes);
    if hash != entry.hash {
        return Err(invalid_bundle(format!(
            "checksum mismatch for {}: expected {}, got {}",
            entry.path, entry.hash, hash
        )));
    }
    if bytes.len() as u64 != entry.size_bytes {
        return Err(invalid_bundle(format!(
            "size mismatch for {}: expected {}, got {}",
            entry.path,
            entry.size_bytes,
            bytes.len()
        )));
    }
    Ok(())
}

fn validate_bundle_inventory_is_closed(
    bundle_dir: &Path,
    manifest: &SessionBundleManifest,
    checksums: &SessionBundleChecksums,
) -> Result<()> {
    let mut expected = BTreeSet::from([
        SESSION_BT_EVENTS.to_owned(),
        SESSION_BT_RAW_CHUNK_REFS.to_owned(),
    ]);
    for content in &manifest.contents {
        if let Some(path) = content.path.as_deref() {
            validate_bundle_relative_path(path)?;
            expected.insert(path.to_owned());
        }
    }

    let mut checksum_paths = BTreeSet::new();
    for entry in &checksums.files {
        validate_bundle_relative_path(&entry.path)?;
        if !checksum_paths.insert(entry.path.clone()) {
            return Err(invalid_bundle(format!(
                "duplicate checksum entry {}",
                entry.path
            )));
        }
    }
    for path in &expected {
        if !checksum_paths.contains(path) {
            return Err(invalid_bundle(format!(
                "checksum manifest is missing required path {}",
                path
            )));
        }
    }
    for path in &checksum_paths {
        if !expected.contains(path) {
            return Err(invalid_bundle(format!(
                "checksum manifest contains unreferenced path {}",
                path
            )));
        }
    }

    let mut actual_files = BTreeSet::new();
    collect_bundle_files(bundle_dir, bundle_dir, &mut actual_files)?;
    actual_files.remove(SESSION_BT_MANIFEST);
    actual_files.remove(SESSION_BT_CHECKSUMS);
    for path in &actual_files {
        if !checksum_paths.contains(path) {
            return Err(invalid_bundle(format!(
                "bundle contains untracked file {}",
                path
            )));
        }
    }
    for path in &checksum_paths {
        if !actual_files.contains(path) {
            return Err(invalid_bundle(format!(
                "checksum path {} does not exist in bundle",
                path
            )));
        }
    }
    Ok(())
}

fn collect_bundle_files(
    bundle_dir: &Path,
    current_dir: &Path,
    files: &mut BTreeSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(current_dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_bundle_files(bundle_dir, &path, files)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(bundle_dir).map_err(|error| {
                invalid_bundle(format!(
                    "bundle file {} is outside bundle root: {error}",
                    path.display()
                ))
            })?;
            let relative = relative.to_str().ok_or_else(|| {
                invalid_bundle(format!(
                    "bundle file path is not utf-8: {}",
                    relative.display()
                ))
            })?;
            let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
            validate_bundle_relative_path(&relative)?;
            files.insert(relative);
        } else {
            return Err(invalid_bundle(format!(
                "bundle path is not a regular file or directory: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_bundle_relative_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(invalid_bundle(format!(
            "bundle path must be relative: {}",
            path.display()
        )));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(invalid_bundle(format!(
            "bundle path must not escape bundle root: {}",
            path.display()
        )));
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> PortableHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    PortableHash::from_sha256_hex(to_lower_hex(&hasher.finalize()))
}

fn to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn invalid_bundle(message: impl Into<String>) -> BelltowerError {
    BelltowerError::Protocol(format!("invalid session bundle: {}", message.into()))
}
