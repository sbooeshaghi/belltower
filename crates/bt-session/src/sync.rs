use bt_core::{BelltowerError, BranchId, PortableHash, Result, SessionBundleManifest, SessionId};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use crate::{SESSION_BT_MANIFEST, SESSION_BT_SCHEMA_VERSION, validate_session_bundle_directory};

const SESSION_BT_REMOTE_HEADS: &str = "heads.json";
const SESSION_BT_REMOTE_LOCK: &str = ".heads.lock";
const SESSION_BT_REMOTE_BUNDLES_DIR: &str = "bundles";
const SESSION_BT_REMOTE_SESSIONS_DIR: &str = "sessions";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBundleRemoteHead {
    pub branch_id: BranchId,
    pub head_event_hash: PortableHash,
    pub bundle_hash: PortableHash,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundleRemoteState {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub default_branch_id: Option<BranchId>,
    pub branch_heads: Vec<SessionBundleRemoteHead>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundlePushReport {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub previous_head: Option<PortableHash>,
    pub new_head: PortableHash,
    pub bundle_hash: PortableHash,
    pub copied_bundle: bool,
    pub changed_remote_head: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBundlePullReport {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub head_event_hash: PortableHash,
    pub bundle_hash: PortableHash,
    pub output_dir: PathBuf,
}

pub fn push_session_bundle_directory(
    bundle_dir: impl AsRef<Path>,
    remote_dir: impl AsRef<Path>,
    branch_id: Option<BranchId>,
    expected_head: Option<&PortableHash>,
) -> Result<SessionBundlePushReport> {
    push_session_bundle_directory_inner(
        bundle_dir.as_ref(),
        remote_dir.as_ref(),
        branch_id,
        expected_head,
    )
}

pub fn pull_session_bundle_directory(
    remote_dir: impl AsRef<Path>,
    session_id: SessionId,
    output_dir: impl AsRef<Path>,
    branch_id: Option<BranchId>,
) -> Result<SessionBundlePullReport> {
    pull_session_bundle_directory_inner(
        remote_dir.as_ref(),
        session_id,
        output_dir.as_ref(),
        branch_id,
    )
}

fn push_session_bundle_directory_inner(
    bundle_dir: &Path,
    remote_dir: &Path,
    branch_id: Option<BranchId>,
    expected_head: Option<&PortableHash>,
) -> Result<SessionBundlePushReport> {
    let validation = validate_session_bundle_directory(bundle_dir)?;
    let manifest = read_manifest(bundle_dir)?;
    let branch = select_push_branch(&manifest, branch_id)?;
    let head = branch
        .head
        .as_ref()
        .ok_or_else(|| invalid_sync("cannot push branch without a head event"))?;
    let session_dir = remote_session_dir(remote_dir, manifest.session.session_id);
    let bundles_dir = session_dir.join(SESSION_BT_REMOTE_BUNDLES_DIR);
    fs::create_dir_all(&bundles_dir)?;
    let _lock = acquire_remote_state_lock(&session_dir)?;

    let mut state = read_remote_state(&session_dir, manifest.session.session_id)?;
    let existing_index = state
        .branch_heads
        .iter()
        .position(|remote_head| remote_head.branch_id == branch.branch_id);
    let previous_head = existing_index
        .and_then(|index| state.branch_heads.get(index))
        .map(|remote_head| remote_head.head_event_hash.clone());
    enforce_cas(
        branch.branch_id,
        previous_head.as_ref(),
        expected_head,
        &head.event_hash,
    )?;

    let changed_remote_head = previous_head.as_ref() != Some(&head.event_hash);
    if changed_remote_head && let Some(index) = existing_index {
        ensure_pushed_bundle_extends_remote_head(
            bundle_dir,
            &bundles_dir,
            &state.branch_heads[index],
        )?;
    }
    // Copy only after every integrity check has passed, so a rejected push
    // cannot leave an orphaned bundle directory on the remote.
    let copied_bundle = copy_bundle_to_remote(bundle_dir, &bundles_dir, &validation.bundle_hash)?;
    if changed_remote_head {
        let remote_head = SessionBundleRemoteHead {
            branch_id: branch.branch_id,
            head_event_hash: head.event_hash.clone(),
            bundle_hash: validation.bundle_hash.clone(),
        };
        if let Some(index) = existing_index {
            state.branch_heads[index] = remote_head;
        } else {
            state.branch_heads.push(remote_head);
        }
        state.branch_heads.sort_by_key(|head| head.branch_id);
    }
    if state.default_branch_id.is_none() || branch.is_default {
        state.default_branch_id = Some(branch.branch_id);
    }
    write_remote_state(&session_dir, &state)?;

    Ok(SessionBundlePushReport {
        session_id: manifest.session.session_id,
        branch_id: branch.branch_id,
        previous_head,
        new_head: head.event_hash.clone(),
        bundle_hash: validation.bundle_hash,
        copied_bundle,
        changed_remote_head,
    })
}

fn pull_session_bundle_directory_inner(
    remote_dir: &Path,
    session_id: SessionId,
    output_dir: &Path,
    branch_id: Option<BranchId>,
) -> Result<SessionBundlePullReport> {
    let session_dir = remote_session_dir(remote_dir, session_id);
    let state = read_existing_remote_state(&session_dir, session_id)?;
    let head = select_pull_head(&state, branch_id)?;
    let source_bundle_dir = session_dir
        .join(SESSION_BT_REMOTE_BUNDLES_DIR)
        .join(head.bundle_hash.sha256_hex());
    if !source_bundle_dir.is_dir() {
        return Err(invalid_sync(format!(
            "remote bundle {} is missing",
            head.bundle_hash
        )));
    }
    prepare_empty_output_dir(output_dir)?;
    copy_dir_contents(&source_bundle_dir, output_dir)?;
    validate_session_bundle_directory(output_dir)?;
    Ok(SessionBundlePullReport {
        session_id,
        branch_id: head.branch_id,
        head_event_hash: head.head_event_hash.clone(),
        bundle_hash: head.bundle_hash.clone(),
        output_dir: output_dir.to_path_buf(),
    })
}

fn select_push_branch(
    manifest: &SessionBundleManifest,
    branch_id: Option<BranchId>,
) -> Result<&bt_core::BranchManifest> {
    if let Some(branch_id) = branch_id {
        return manifest
            .branches
            .iter()
            .find(|branch| branch.branch_id == branch_id)
            .ok_or_else(|| invalid_sync(format!("branch {branch_id} is not in bundle")));
    }
    manifest
        .branches
        .iter()
        .find(|branch| branch.is_default)
        .or_else(|| manifest.branches.first())
        .ok_or_else(|| invalid_sync("bundle has no branches"))
}

fn select_pull_head(
    state: &SessionBundleRemoteState,
    branch_id: Option<BranchId>,
) -> Result<&SessionBundleRemoteHead> {
    if let Some(branch_id) = branch_id {
        return state
            .branch_heads
            .iter()
            .find(|head| head.branch_id == branch_id)
            .ok_or_else(|| invalid_sync(format!("remote branch {branch_id} not found")));
    }
    if let Some(default_branch_id) = state.default_branch_id {
        return state
            .branch_heads
            .iter()
            .find(|head| head.branch_id == default_branch_id)
            .ok_or_else(|| invalid_sync(format!("default branch {default_branch_id} not found")));
    }
    if state.branch_heads.len() == 1 {
        return Ok(&state.branch_heads[0]);
    }
    Err(invalid_sync(
        "remote has multiple heads; pass --branch to select one",
    ))
}

/// Append-only registry integrity: advancing an existing branch head is only
/// legal when the pushed bundle extends the stored head bundle's history —
/// its `events.jsonl` must begin byte-for-byte with the stored one (prefix
/// stability makes this exact). Head-equality CAS alone would let a
/// fabricated divergent lineage echo the current head and silently rebind
/// it; divergent histories must fork, never rewrite.
fn ensure_pushed_bundle_extends_remote_head(
    bundle_dir: &Path,
    bundles_dir: &Path,
    previous: &SessionBundleRemoteHead,
) -> Result<()> {
    let previous_events = bundles_dir
        .join(previous.bundle_hash.sha256_hex())
        .join(crate::SESSION_BT_EVENTS);
    if !previous_events.is_file() {
        return Err(invalid_sync(format!(
            "remote head bundle {} is missing; refusing to advance an unverifiable head",
            previous.bundle_hash
        )));
    }
    let previous_bytes = fs::read(&previous_events)?;
    let pushed_bytes = fs::read(bundle_dir.join(crate::SESSION_BT_EVENTS))?;
    if !pushed_bytes.starts_with(&previous_bytes) {
        return Err(invalid_sync(
            "pushed bundle does not extend the remote head's history; append-only registries fork divergent histories instead of rebinding heads"
                .to_owned(),
        ));
    }
    Ok(())
}

fn enforce_cas(
    branch_id: BranchId,
    previous_head: Option<&PortableHash>,
    expected_head: Option<&PortableHash>,
    new_head: &PortableHash,
) -> Result<()> {
    match (previous_head, expected_head) {
        (Some(previous), Some(expected)) if previous != expected => {
            Err(BelltowerError::InvalidState(format!(
                "remote branch {branch_id} head conflict: expected {expected}, found {previous}"
            )))
        }
        (Some(previous), None) if previous != new_head => {
            Err(BelltowerError::InvalidState(format!(
                "remote branch {branch_id} already has head {previous}; pass --expected-head to advance it"
            )))
        }
        (None, Some(expected)) => Err(BelltowerError::InvalidState(format!(
            "remote branch {branch_id} does not exist; expected head {expected} cannot match"
        ))),
        _ => Ok(()),
    }
}

fn read_remote_state(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<SessionBundleRemoteState> {
    if session_dir.join(SESSION_BT_REMOTE_HEADS).exists() {
        read_existing_remote_state(session_dir, session_id)
    } else {
        Ok(SessionBundleRemoteState {
            schema_version: SESSION_BT_SCHEMA_VERSION,
            session_id,
            default_branch_id: None,
            branch_heads: Vec::new(),
        })
    }
}

fn read_existing_remote_state(
    session_dir: &Path,
    session_id: SessionId,
) -> Result<SessionBundleRemoteState> {
    let state_path = session_dir.join(SESSION_BT_REMOTE_HEADS);
    let bytes = fs::read(&state_path)?;
    let state: SessionBundleRemoteState = serde_json::from_slice(&bytes)?;
    if state.schema_version != SESSION_BT_SCHEMA_VERSION {
        return Err(invalid_sync(format!(
            "unsupported remote schema version {}",
            state.schema_version
        )));
    }
    if state.session_id != session_id {
        return Err(invalid_sync(format!(
            "remote state session {} does not match requested session {}",
            state.session_id, session_id
        )));
    }
    Ok(state)
}

fn write_remote_state(session_dir: &Path, state: &SessionBundleRemoteState) -> Result<()> {
    fs::create_dir_all(session_dir)?;
    let state_path = session_dir.join(SESSION_BT_REMOTE_HEADS);
    let temp_path = session_dir.join(format!(
        ".heads-{}.tmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| BelltowerError::InvalidState(error.to_string()))?
            .as_nanos()
    ));
    let bytes = serde_json::to_vec_pretty(state)?;
    fs::write(&temp_path, bytes)?;
    fs::rename(temp_path, state_path)?;
    Ok(())
}

struct RemoteStateLock {
    path: PathBuf,
}

impl Drop for RemoteStateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_remote_state_lock(session_dir: &Path) -> Result<RemoteStateLock> {
    fs::create_dir_all(session_dir)?;
    let path = session_dir.join(SESSION_BT_REMOTE_LOCK);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(RemoteStateLock { path }),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(invalid_sync(format!(
                "remote state lock {} is already held; pull or retry after the active push completes",
                path.display()
            )))
        }
        Err(error) => Err(BelltowerError::InvalidState(format!(
            "failed to acquire remote state lock {}: {error}",
            path.display()
        ))),
    }
}

fn copy_bundle_to_remote(
    bundle_dir: &Path,
    bundles_dir: &Path,
    bundle_hash: &PortableHash,
) -> Result<bool> {
    let dest = bundles_dir.join(bundle_hash.sha256_hex());
    if dest.exists() {
        validate_session_bundle_directory(&dest)?;
        return Ok(false);
    }
    let temp = bundles_dir.join(format!(
        ".bundle-{}-{}.tmp",
        bundle_hash.sha256_hex(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| BelltowerError::InvalidState(error.to_string()))?
            .as_nanos()
    ));
    copy_dir_contents(bundle_dir, &temp)?;
    fs::rename(temp, dest)?;
    Ok(true)
}

fn copy_dir_contents(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_contents(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(invalid_sync(format!(
                "unsupported bundle path {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn prepare_empty_output_dir(output_dir: &Path) -> Result<()> {
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(invalid_sync(format!(
                "output path {} is not a directory",
                output_dir.display()
            )));
        }
        if fs::read_dir(output_dir)?.next().is_some() {
            return Err(invalid_sync(format!(
                "output directory {} is not empty",
                output_dir.display()
            )));
        }
        Ok(())
    } else {
        fs::create_dir_all(output_dir)?;
        Ok(())
    }
}

fn read_manifest(bundle_dir: &Path) -> Result<SessionBundleManifest> {
    let bytes = fs::read(bundle_dir.join(SESSION_BT_MANIFEST))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn remote_session_dir(remote_dir: &Path, session_id: SessionId) -> PathBuf {
    remote_dir
        .join(SESSION_BT_REMOTE_SESSIONS_DIR)
        .join(session_id.to_string())
}

fn invalid_sync(message: impl Into<String>) -> BelltowerError {
    BelltowerError::InvalidState(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PortableSessionBundleExporter, SqliteSessionStore};
    use bt_core::{
        ConnectionId, EventEnvelope, EventPayload, Message, Role, SessionRecord, SessionStatus,
        SessionToolMode, SpanKind, default_settings_revision_id,
    };
    use tempfile::{NamedTempFile, tempdir};

    #[test]
    fn session_bundle_push_and_pull_round_trip_remote_head() {
        let (_store_file, bundle_dir, session_id, branch_id, head) = export_bundle_fixture();
        let remote_dir = tempdir().expect("remote");
        let pushed =
            push_session_bundle_directory(bundle_dir.path(), remote_dir.path(), None, None)
                .expect("push");
        assert_eq!(pushed.session_id, session_id);
        assert_eq!(pushed.branch_id, branch_id);
        assert_eq!(pushed.previous_head, None);
        assert_eq!(pushed.new_head, head);
        assert!(pushed.copied_bundle);
        assert!(pushed.changed_remote_head);

        let pulled_dir = tempdir().expect("pulled");
        let pulled =
            pull_session_bundle_directory(remote_dir.path(), session_id, pulled_dir.path(), None)
                .expect("pull");
        assert_eq!(pulled.branch_id, branch_id);
        assert_eq!(pulled.head_event_hash, head);
        validate_session_bundle_directory(pulled_dir.path()).expect("pulled bundle validates");
    }

    #[test]
    fn session_bundle_push_rejects_stale_expected_head() {
        let (store_file, first_bundle, session_id, branch_id, first_head) = export_bundle_fixture();
        let remote_dir = tempdir().expect("remote");
        push_session_bundle_directory(first_bundle.path(), remote_dir.path(), None, None)
            .expect("initial push");

        let (second_bundle, _, _, second_head) =
            export_bundle_with_extra_message(&store_file, session_id, branch_id);
        assert_ne!(first_head, second_head);
        let stale = PortableHash::new(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("hash");
        let error = push_session_bundle_directory(
            second_bundle.path(),
            remote_dir.path(),
            Some(branch_id),
            Some(&stale),
        )
        .expect_err("stale expected head should fail");
        assert!(error.to_string().contains("head conflict"));

        let advanced = push_session_bundle_directory(
            second_bundle.path(),
            remote_dir.path(),
            Some(branch_id),
            Some(&first_head),
        )
        .expect("advance with matching expected head");
        assert_eq!(advanced.previous_head, Some(first_head));
        assert_eq!(advanced.new_head, second_head);
        assert!(advanced.changed_remote_head);
    }

    #[test]
    fn session_bundle_push_rejects_held_remote_state_lock() {
        let (_store_file, bundle_dir, session_id, _, _) = export_bundle_fixture();
        let remote_dir = tempdir().expect("remote");
        let session_dir = remote_session_dir(remote_dir.path(), session_id);
        fs::create_dir_all(&session_dir).expect("session dir");
        fs::write(session_dir.join(SESSION_BT_REMOTE_LOCK), b"held").expect("lock");

        let error = push_session_bundle_directory(bundle_dir.path(), remote_dir.path(), None, None)
            .expect_err("held lock should fail");
        assert!(error.to_string().contains("remote state lock"));
    }

    fn export_bundle_fixture() -> (
        NamedTempFile,
        tempfile::TempDir,
        SessionId,
        BranchId,
        PortableHash,
    ) {
        let file = NamedTempFile::new().expect("tempfile");
        let mut store = SqliteSessionStore::open(file.path()).expect("open store");
        let session = SessionRecord {
            session_id: SessionId::new(),
            project_root: "/tmp/belltower-sync".into(),
            connection_id: ConnectionId::new("local"),
            model_id: None,
            display_name: Some("sync fixture".to_owned()),
            objective: None,
            status: SessionStatus::Active,
            settings_revision_id: default_settings_revision_id(),
            tool_mode: SessionToolMode::Standard,
            parent_session_id: None,
            parent_branch_id: None,
            parent_turn_id: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
        };
        let branch = bt_core::BranchRecord {
            branch_id: BranchId::new(),
            session_id: session.session_id,
            parent_branch_id: None,
            parent_event_id: None,
            head_event_id: None,
            summary: None,
            created_at: time::OffsetDateTime::now_utc(),
            is_default: true,
        };
        store
            .create_session(&session, &branch)
            .expect("create session");
        store
            .append_event(&EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::User, "hello"),
                },
            ))
            .expect("append message");
        drop(store);

        let reopened = SqliteSessionStore::open(file.path()).expect("reopen store");
        let bundle_dir = tempdir().expect("bundle");
        let manifest = reopened
            .export_session_bundle_directory(session.session_id, bundle_dir.path())
            .expect("export bundle");
        let head = manifest
            .branches
            .iter()
            .find(|candidate| candidate.branch_id == branch.branch_id)
            .and_then(|branch| branch.head.as_ref())
            .expect("head")
            .event_hash
            .clone();
        (file, bundle_dir, session.session_id, branch.branch_id, head)
    }

    /// Grows the SAME backing store by one message and re-exports, so the
    /// second bundle is a true descendant of the first (the push descendancy
    /// check rejects rebuilt same-id histories by design).
    fn export_bundle_with_extra_message(
        store_file: &NamedTempFile,
        session_id: SessionId,
        branch_id: BranchId,
    ) -> (tempfile::TempDir, SessionId, BranchId, PortableHash) {
        let mut store = SqliteSessionStore::open(store_file.path()).expect("open store");
        store
            .append_event(&EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::User, "again"),
                },
            ))
            .expect("append message");
        drop(store);

        let reopened = SqliteSessionStore::open(store_file.path()).expect("reopen store");
        let bundle_dir = tempdir().expect("bundle");
        let manifest = reopened
            .export_session_bundle_directory(session_id, bundle_dir.path())
            .expect("export bundle");
        let head = manifest
            .branches
            .iter()
            .find(|branch| branch.branch_id == branch_id)
            .and_then(|branch| branch.head.as_ref())
            .expect("head")
            .event_hash
            .clone();
        (bundle_dir, session_id, branch_id, head)
    }
}
