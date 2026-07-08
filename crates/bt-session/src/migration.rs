use rusqlite::{Connection, params};
use serde_json::Value;

pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE IF NOT EXISTS sessions (
    session_id      TEXT PRIMARY KEY,
    project_root    TEXT NOT NULL,
    connection_id   TEXT NOT NULL,
    model_id        TEXT,
    tool_mode       TEXT NOT NULL DEFAULT 'extended',
    settings_revision_id INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    status          TEXT NOT NULL,
    display_name    TEXT,
    objective       TEXT,
    parent_session_id TEXT,
    parent_branch_id  TEXT,
    parent_turn_id    TEXT
);

CREATE TABLE IF NOT EXISTS branches (
    branch_id         TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL,
    parent_branch_id  TEXT,
    parent_event_id   TEXT,
    head_event_id     TEXT,
    summary           TEXT,
    created_at        TEXT NOT NULL,
    is_default        INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE TABLE IF NOT EXISTS events (
    seq_id           INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id         TEXT NOT NULL UNIQUE,
    session_id       TEXT NOT NULL,
    branch_id        TEXT NOT NULL,
    span_id          TEXT NOT NULL,
    parent_span_id   TEXT,
    span_kind        TEXT NOT NULL,
    event_kind       TEXT NOT NULL,
    event_json       TEXT NOT NULL,
    occurred_at      TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id),
    FOREIGN KEY (branch_id) REFERENCES branches(branch_id)
);

CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq_id);
CREATE INDEX IF NOT EXISTS idx_events_branch_seq ON events(branch_id, seq_id);

CREATE TABLE IF NOT EXISTS raw_chunks (
    chunk_id        INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL,
    branch_id       TEXT,
    turn_id         TEXT,
    llm_call_ordinal INTEGER,
    event_id        TEXT,
    provider        TEXT NOT NULL,
    stream_name     TEXT NOT NULL,
    content         BLOB NOT NULL,
    received_at     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_raw_chunks_session ON raw_chunks(session_id, chunk_id);

CREATE TABLE IF NOT EXISTS message_projection (
    event_id        TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    branch_id       TEXT NOT NULL,
    message_id      TEXT NOT NULL,
    role            TEXT NOT NULL,
    content_json    TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS approval_projection (
    call_id         TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    tool_name       TEXT NOT NULL,
    status          TEXT NOT NULL,
    request_fingerprint TEXT,
    request_snapshot_json TEXT,
    decision_json   TEXT,
    resolution_json TEXT,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_run_projection (
    call_id         TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    tool_name       TEXT NOT NULL,
    status          TEXT NOT NULL,
    arguments_json  TEXT,
    result_json     TEXT,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS context_manifest_projection (
    session_id              TEXT NOT NULL,
    branch_id               TEXT NOT NULL,
    turn_id                 TEXT NOT NULL,
    llm_call_ordinal        INTEGER NOT NULL,
    provider                TEXT NOT NULL,
    model                   TEXT NOT NULL,
    settings_revision_id    INTEGER NOT NULL DEFAULT 1,
    context_boundary_seq_id INTEGER,
    message_count           INTEGER NOT NULL,
    tool_count              INTEGER NOT NULL,
    attachment_count        INTEGER NOT NULL DEFAULT 0,
    compacted               INTEGER NOT NULL DEFAULT 0,
    manifest_json           TEXT NOT NULL,
    recorded_event_id       TEXT NOT NULL,
    recorded_seq_id         INTEGER NOT NULL,
    recorded_at             TEXT NOT NULL,
    PRIMARY KEY (session_id, turn_id, llm_call_ordinal)
);
CREATE INDEX IF NOT EXISTS idx_context_manifest_projection_session_branch_seq
    ON context_manifest_projection(session_id, branch_id, recorded_seq_id);

CREATE TABLE IF NOT EXISTS branch_heads (
    branch_id       TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    head_event_seq  INTEGER NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cost_summary_projection (
    session_id            TEXT PRIMARY KEY,
    prompt_tokens         INTEGER NOT NULL DEFAULT 0,
    completion_tokens     INTEGER NOT NULL DEFAULT 0,
    total_tokens          INTEGER NOT NULL DEFAULT 0,
    total_cost_usd        REAL NOT NULL DEFAULT 0,
    unpriced_completion_count INTEGER NOT NULL DEFAULT 0,
    updated_at            TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session_budget_projection (
    session_id               TEXT PRIMARY KEY,
    max_wall_clock_seconds   INTEGER,
    max_tokens               INTEGER,
    max_turns                INTEGER,
    max_cost_usd             REAL,
    tokens_used              INTEGER NOT NULL DEFAULT 0,
    turns_used               INTEGER NOT NULL DEFAULT 0,
    elapsed_seconds          INTEGER NOT NULL DEFAULT 0,
    cost_used_usd            REAL,
    updated_at               TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS plan_projection (
    session_id      TEXT NOT NULL,
    branch_id       TEXT NOT NULL,
    items_json      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (session_id, branch_id)
);

CREATE TABLE IF NOT EXISTS session_control_projection (
    session_id      TEXT PRIMARY KEY,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS queued_message_projection (
    queue_event_id  TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    branch_id       TEXT NOT NULL,
    message_json    TEXT NOT NULL,
    settings_revision_id INTEGER NOT NULL DEFAULT 1,
    status          TEXT NOT NULL,
    source_seq      INTEGER NOT NULL,
    enqueued_at     TEXT NOT NULL,
    resolved_at     TEXT
);

CREATE TABLE IF NOT EXISTS steer_projection (
    steer_event_id  TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    branch_id       TEXT NOT NULL,
    message         TEXT NOT NULL,
    settings_revision_id INTEGER NOT NULL DEFAULT 1,
    status          TEXT NOT NULL,
    source_seq      INTEGER NOT NULL,
    enqueued_at     TEXT NOT NULL,
    resolved_at     TEXT
);

CREATE INDEX IF NOT EXISTS idx_queued_message_projection_pending
    ON queued_message_projection(session_id, status, source_seq);
CREATE INDEX IF NOT EXISTS idx_steer_projection_pending
    ON steer_projection(session_id, status, source_seq);

CREATE TABLE IF NOT EXISTS session_settings_revision_projection (
    session_id      TEXT NOT NULL,
    settings_revision_id INTEGER NOT NULL,
    connection_id   TEXT NOT NULL,
    model_id        TEXT,
    tool_mode       TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (session_id, settings_revision_id)
);
CREATE INDEX IF NOT EXISTS idx_session_settings_revision_projection_lookup
    ON session_settings_revision_projection(session_id, settings_revision_id);
"#];

pub fn apply_migrations(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    for migration in MIGRATIONS {
        connection.execute_batch(migration)?;
    }
    ensure_column(connection, "sessions", "model_id", "TEXT")?;
    ensure_column(
        connection,
        "sessions",
        "tool_mode",
        "TEXT NOT NULL DEFAULT 'extended'",
    )?;
    ensure_column(
        connection,
        "sessions",
        "settings_revision_id",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    ensure_column(connection, "sessions", "parent_session_id", "TEXT")?;
    ensure_column(connection, "sessions", "parent_branch_id", "TEXT")?;
    ensure_column(connection, "sessions", "parent_turn_id", "TEXT")?;
    ensure_column(
        connection,
        "approval_projection",
        "request_fingerprint",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "approval_projection",
        "request_snapshot_json",
        "TEXT",
    )?;
    ensure_column(connection, "approval_projection", "resolution_json", "TEXT")?;
    ensure_column(
        connection,
        "queued_message_projection",
        "settings_revision_id",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    ensure_column(
        connection,
        "steer_projection",
        "settings_revision_id",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    ensure_column(connection, "raw_chunks", "branch_id", "TEXT")?;
    ensure_column(connection, "raw_chunks", "turn_id", "TEXT")?;
    ensure_column(connection, "raw_chunks", "llm_call_ordinal", "INTEGER")?;
    ensure_column(
        connection,
        "cost_summary_projection",
        "unpriced_completion_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_raw_chunks_session ON raw_chunks(session_id, chunk_id);
         CREATE INDEX IF NOT EXISTS idx_raw_chunks_session_branch_turn ON raw_chunks(session_id, branch_id, turn_id, chunk_id);
         CREATE INDEX IF NOT EXISTS idx_raw_chunks_session_branch_turn_call ON raw_chunks(session_id, branch_id, turn_id, llm_call_ordinal, chunk_id);
         CREATE INDEX IF NOT EXISTS idx_queued_message_projection_pending ON queued_message_projection(session_id, status, source_seq);
         CREATE INDEX IF NOT EXISTS idx_steer_projection_pending ON steer_projection(session_id, status, source_seq);
         CREATE INDEX IF NOT EXISTS idx_session_settings_revision_projection_lookup ON session_settings_revision_projection(session_id, settings_revision_id);
         CREATE INDEX IF NOT EXISTS idx_context_manifest_projection_session_branch_seq
             ON context_manifest_projection(session_id, branch_id, recorded_seq_id);
         CREATE TABLE IF NOT EXISTS session_budget_projection (
             session_id TEXT PRIMARY KEY,
             max_wall_clock_seconds INTEGER,
             max_tokens INTEGER,
             max_turns INTEGER,
             max_cost_usd REAL,
             tokens_used INTEGER NOT NULL DEFAULT 0,
             turns_used INTEGER NOT NULL DEFAULT 0,
             elapsed_seconds INTEGER NOT NULL DEFAULT 0,
             cost_used_usd REAL,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS session_settings_revision_projection (
             session_id TEXT NOT NULL,
             settings_revision_id INTEGER NOT NULL,
             connection_id TEXT NOT NULL,
             model_id TEXT,
             tool_mode TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             PRIMARY KEY (session_id, settings_revision_id)
         );
         CREATE TABLE IF NOT EXISTS context_manifest_projection (
             session_id TEXT NOT NULL,
             branch_id TEXT NOT NULL,
             turn_id TEXT NOT NULL,
             llm_call_ordinal INTEGER NOT NULL,
             provider TEXT NOT NULL,
             model TEXT NOT NULL,
             settings_revision_id INTEGER NOT NULL DEFAULT 1,
             context_boundary_seq_id INTEGER,
             message_count INTEGER NOT NULL,
             tool_count INTEGER NOT NULL,
             attachment_count INTEGER NOT NULL DEFAULT 0,
             compacted INTEGER NOT NULL DEFAULT 0,
             manifest_json TEXT NOT NULL,
             recorded_event_id TEXT NOT NULL,
             recorded_seq_id INTEGER NOT NULL,
             recorded_at TEXT NOT NULL,
             PRIMARY KEY (session_id, turn_id, llm_call_ordinal)
         );",
    )?;
    connection.execute(
        "UPDATE sessions SET settings_revision_id = 1 WHERE settings_revision_id <= 0",
        [],
    )?;
    connection.execute(
        "UPDATE queued_message_projection
         SET settings_revision_id = 1
         WHERE settings_revision_id <= 0",
        [],
    )?;
    connection.execute(
        "UPDATE steer_projection
         SET settings_revision_id = 1
         WHERE settings_revision_id <= 0",
        [],
    )?;
    connection.execute(
        "DELETE FROM session_settings_revision_projection
         WHERE settings_revision_id <= 0",
        [],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO session_settings_revision_projection (
             session_id, settings_revision_id, connection_id, model_id, tool_mode, updated_at
         )
         SELECT session_id, settings_revision_id, connection_id, model_id, tool_mode, updated_at
         FROM sessions",
        [],
    )?;
    drop_column_if_exists(connection, "sessions", "trace_id")?;
    drop_column_if_exists(connection, "events", "trace_id")?;
    rewrite_legacy_event_json_without_trace_id(connection)?;
    Ok(())
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    if table_has_column(connection, table, column)? {
        return Ok(());
    }

    connection.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

fn drop_column_if_exists(
    connection: &Connection,
    table: &str,
    column: &str,
) -> rusqlite::Result<()> {
    if table_has_column(connection, table, column)? {
        connection.execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), [])?;
    }
    Ok(())
}

fn rewrite_legacy_event_json_without_trace_id(connection: &Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare(
        "SELECT event_id, event_json
         FROM events
         WHERE instr(event_json, '\"trace_id\"') > 0",
    )?;
    let rows = statement.query_map([], |row| {
        let event_id: String = row.get(0)?;
        let event_json: String = row.get(1)?;
        Ok((event_id, event_json))
    })?;

    let mut updates = Vec::new();
    for row in rows {
        let (event_id, event_json) = row?;
        let mut value: Value = serde_json::from_str(&event_json)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let Some(object) = value.as_object_mut() else {
            continue;
        };
        if object.remove("trace_id").is_some() {
            let rewritten = serde_json::to_string(&value)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            updates.push((event_id, rewritten));
        }
    }

    for (event_id, event_json) in updates {
        connection.execute(
            "UPDATE events SET event_json = ?1 WHERE event_id = ?2",
            params![event_json, event_id],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_event_trace_rewrite_skips_events_without_trace_id_marker() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .execute_batch(
                "CREATE TABLE events (
                    event_id TEXT PRIMARY KEY,
                    event_json TEXT NOT NULL
                );",
            )
            .expect("schema");
        connection
            .execute(
                "INSERT INTO events (event_id, event_json) VALUES (?1, ?2)",
                params![
                    "event-1",
                    r#"{"event_id":"event-1","payload":{"MessageAppended":{"message":"trace_id in content"}}}"#
                ],
            )
            .expect("insert content marker");
        connection
            .execute(
                "INSERT INTO events (event_id, event_json) VALUES (?1, ?2)",
                params![
                    "event-2",
                    r#"{"event_id":"event-2","trace_id":"legacy","payload":{"SessionStarted":{"project_root":"/tmp","connection_id":"local"}}}"#
                ],
            )
            .expect("insert legacy marker");

        rewrite_legacy_event_json_without_trace_id(&connection).expect("rewrite");

        let rewritten: String = connection
            .query_row(
                "SELECT event_json FROM events WHERE event_id = 'event-2'",
                [],
                |row| row.get(0),
            )
            .expect("rewritten event");
        let value: Value = serde_json::from_str(&rewritten).expect("json");
        assert!(value.get("trace_id").is_none());

        let untouched: String = connection
            .query_row(
                "SELECT event_json FROM events WHERE event_id = 'event-1'",
                [],
                |row| row.get(0),
            )
            .expect("untouched event");
        assert!(untouched.contains("trace_id in content"));
    }
}
