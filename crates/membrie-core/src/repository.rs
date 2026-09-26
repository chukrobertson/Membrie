use crate::model::{
    ActivityRecordResult, ActivitySnapshot, CaptureCandidate, CaptureDecision, CaptureRule,
    CaptureStatus, EmbeddedChunk, IntelligenceSettings, IntelligenceStatus, LocalModel,
    NewRemembrie, PauseMode, ProcessingJob, Remembrie, ScreenAnalysis, ScreenCaptureCandidate,
    ScreenCaptureResult, SearchHit, SemanticCaptureCandidate, SemanticCaptureResult,
    TimelineActivityObservation, TimelineActivitySummary, TimelineEntry, TimelineHistorySpan,
};
use crate::policy::{MAX_AUTOMATIC_CONTENT_BYTES, exclusion_reason, sensitive_reason};
use rusqlite::{Connection, DatabaseName, OptionalExtension, Row, params};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 6;
const DUPLICATE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const MIN_SEMANTIC_SIMILARITY: f64 = 0.25;
const SEMANTIC_SCORE_WINDOW: f64 = 0.20;

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS remembries (
    id                TEXT PRIMARY KEY NOT NULL,
    kind              TEXT NOT NULL,
    occurred_at_ms    INTEGER NOT NULL,
    ended_at_ms       INTEGER,
    source_app        TEXT,
    window_title      TEXT,
    title             TEXT NOT NULL,
    summary           TEXT,
    sensitivity       TEXT NOT NULL DEFAULT 'normal'
                      CHECK (sensitivity IN ('normal', 'sensitive', 'private')),
    importance        REAL NOT NULL DEFAULT 0.5
                      CHECK (importance >= 0.0 AND importance <= 1.0),
    pinned            INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL,
    deleted_at_ms     INTEGER
);

CREATE INDEX IF NOT EXISTS idx_remembries_occurred_at
    ON remembries(occurred_at_ms DESC)
    WHERE deleted_at_ms IS NULL;
CREATE INDEX IF NOT EXISTS idx_remembries_source_app
    ON remembries(source_app)
    WHERE deleted_at_ms IS NULL;

CREATE TABLE IF NOT EXISTS remembrie_contents (
    id                TEXT PRIMARY KEY NOT NULL,
    remembrie_id      TEXT NOT NULL REFERENCES remembries(id) ON DELETE CASCADE,
    role              TEXT NOT NULL,
    mime_type         TEXT NOT NULL DEFAULT 'text/plain',
    text_content      TEXT,
    blob_hash         TEXT,
    created_at_ms     INTEGER NOT NULL,
    CHECK (text_content IS NOT NULL OR blob_hash IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_contents_remembrie
    ON remembrie_contents(remembrie_id);

CREATE VIRTUAL TABLE IF NOT EXISTS remembrie_fts USING fts5(
    remembrie_id UNINDEXED,
    title,
    body,
    summary,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS remembrie_chunks (
    id                TEXT PRIMARY KEY NOT NULL,
    remembrie_id      TEXT NOT NULL REFERENCES remembries(id) ON DELETE CASCADE,
    content_id        TEXT REFERENCES remembrie_contents(id) ON DELETE CASCADE,
    ordinal           INTEGER NOT NULL,
    start_offset      INTEGER NOT NULL,
    end_offset        INTEGER NOT NULL,
    text_content      TEXT NOT NULL,
    token_count       INTEGER,
    UNIQUE(remembrie_id, content_id, ordinal)
);

CREATE TABLE IF NOT EXISTS embeddings (
    chunk_id          TEXT NOT NULL REFERENCES remembrie_chunks(id) ON DELETE CASCADE,
    model             TEXT NOT NULL,
    dimensions        INTEGER NOT NULL,
    vector            BLOB NOT NULL,
    created_at_ms     INTEGER NOT NULL,
    PRIMARY KEY(chunk_id, model)
);

CREATE TABLE IF NOT EXISTS derived_artifacts (
    id                TEXT PRIMARY KEY NOT NULL,
    remembrie_id      TEXT NOT NULL REFERENCES remembries(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL,
    text_content      TEXT NOT NULL,
    model             TEXT,
    prompt_version    TEXT,
    created_at_ms     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS remembrie_links (
    from_id           TEXT NOT NULL REFERENCES remembries(id) ON DELETE CASCADE,
    to_id             TEXT NOT NULL REFERENCES remembries(id) ON DELETE CASCADE,
    relation          TEXT NOT NULL,
    weight            REAL NOT NULL DEFAULT 1.0,
    evidence          TEXT,
    created_at_ms     INTEGER NOT NULL,
    PRIMARY KEY(from_id, to_id, relation),
    CHECK(from_id <> to_id)
);

CREATE TABLE IF NOT EXISTS processing_jobs (
    id                TEXT PRIMARY KEY NOT NULL,
    remembrie_id      TEXT REFERENCES remembries(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'pending'
                      CHECK(state IN ('pending', 'running', 'complete', 'failed')),
    attempts          INTEGER NOT NULL DEFAULT 0,
    last_error        TEXT,
    available_at_ms   INTEGER NOT NULL,
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_jobs_ready
    ON processing_jobs(state, available_at_ms);

CREATE TABLE IF NOT EXISTS capture_rules (
    id                TEXT PRIMARY KEY NOT NULL,
    rule_type         TEXT NOT NULL,
    pattern           TEXT NOT NULL,
    enabled           INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    created_at_ms     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS capture_state (
    singleton         INTEGER PRIMARY KEY CHECK(singleton = 1),
    paused            INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0, 1)),
    updated_at_ms     INTEGER NOT NULL
);

INSERT OR IGNORE INTO capture_state(singleton, paused, updated_at_ms)
VALUES(1, 0, 0);
"#;

const MIGRATION_2: &str = r#"
ALTER TABLE capture_state ADD COLUMN paused_until_ms INTEGER;
ALTER TABLE capture_state ADD COLUMN clipboard_enabled INTEGER NOT NULL DEFAULT 0
    CHECK(clipboard_enabled IN (0, 1));
ALTER TABLE capture_rules ADD COLUMN label TEXT;
ALTER TABLE capture_rules ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0
    CHECK(is_default IN (0, 1));

CREATE UNIQUE INDEX IF NOT EXISTS idx_capture_rules_unique
    ON capture_rules(rule_type, pattern);

CREATE TABLE IF NOT EXISTS capture_events (
    id                TEXT PRIMARY KEY NOT NULL,
    occurred_at_ms    INTEGER NOT NULL,
    source_kind       TEXT NOT NULL,
    outcome           TEXT NOT NULL,
    reason            TEXT,
    remembrie_id      TEXT REFERENCES remembries(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_capture_events_time
    ON capture_events(occurred_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_capture_events_outcome
    ON capture_events(outcome, occurred_at_ms DESC);

INSERT OR IGNORE INTO capture_rules(
    id, rule_type, pattern, enabled, created_at_ms, label, is_default
) VALUES
    ('default-1password', 'app', '*1password*', 1, 0, '1Password', 1),
    ('default-bitwarden', 'app', '*bitwarden*', 1, 0, 'Bitwarden', 1),
    ('default-keepass', 'app', '*keepass*', 1, 0, 'KeePass', 1),
    ('default-seahorse', 'app', '*seahorse*', 1, 0, 'Passwords and Keys', 1),
    ('default-gnome-password-safe', 'app', '*passwordsafe*', 1, 0, 'Password Safe', 1),
    ('default-private-window', 'window_title', '*private browsing*', 1, 0, 'Private browsing windows', 1),
    ('default-incognito-window', 'window_title', '*incognito*', 1, 0, 'Incognito windows', 1);
"#;

const MIGRATION_3: &str = r#"
CREATE TABLE IF NOT EXISTS intelligence_settings (
    singleton         INTEGER PRIMARY KEY CHECK(singleton = 1),
    chat_model        TEXT NOT NULL,
    embedding_model   TEXT NOT NULL,
    context_tokens    INTEGER NOT NULL CHECK(context_tokens BETWEEN 2048 AND 32768),
    updated_at_ms     INTEGER NOT NULL
);

INSERT OR IGNORE INTO intelligence_settings(
    singleton, chat_model, embedding_model, context_tokens, updated_at_ms
) VALUES(1, 'gemma4:12b', 'embeddinggemma:latest', 8192, 0);

CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_remembrie_kind
    ON processing_jobs(remembrie_id, kind)
    WHERE remembrie_id IS NOT NULL;

INSERT OR IGNORE INTO processing_jobs(
    id, remembrie_id, kind, state, attempts, available_at_ms, created_at_ms, updated_at_ms
)
SELECT 'enrich:' || id, id, 'enrich', 'pending', 0, 0, created_at_ms, created_at_ms
FROM remembries
WHERE deleted_at_ms IS NULL;

UPDATE processing_jobs
SET state = 'pending', available_at_ms = 0, updated_at_ms = 0
WHERE state = 'running';
"#;

const MIGRATION_4: &str = r#"
ALTER TABLE capture_state ADD COLUMN activity_enabled INTEGER NOT NULL DEFAULT 0
    CHECK(activity_enabled IN (0, 1));
ALTER TABLE capture_state ADD COLUMN activity_idle_threshold_ms INTEGER NOT NULL DEFAULT 900000
    CHECK(activity_idle_threshold_ms BETWEEN 300000 AND 14400000);

CREATE TABLE IF NOT EXISTS activity_sessions (
    id                  TEXT PRIMARY KEY NOT NULL,
    started_at_ms       INTEGER NOT NULL,
    ended_at_ms         INTEGER,
    last_active_at_ms   INTEGER NOT NULL,
    end_reason          TEXT,
    remembrie_id        TEXT UNIQUE REFERENCES remembries(id) ON DELETE CASCADE,
    created_at_ms       INTEGER NOT NULL,
    updated_at_ms       INTEGER NOT NULL,
    CHECK(ended_at_ms IS NULL OR ended_at_ms >= started_at_ms)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_activity_one_open_session
    ON activity_sessions((1)) WHERE ended_at_ms IS NULL;
CREATE INDEX IF NOT EXISTS idx_activity_sessions_time
    ON activity_sessions(started_at_ms DESC);

CREATE TABLE IF NOT EXISTS activity_observations (
    id                  TEXT PRIMARY KEY NOT NULL,
    session_id          TEXT NOT NULL REFERENCES activity_sessions(id) ON DELETE CASCADE,
    observed_at_ms      INTEGER NOT NULL,
    app_id              TEXT NOT NULL,
    app_name            TEXT NOT NULL,
    window_title        TEXT NOT NULL,
    created_at_ms       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_activity_observations_session
    ON activity_observations(session_id, observed_at_ms);

INSERT OR IGNORE INTO capture_rules(
    id, rule_type, pattern, enabled, created_at_ms, label, is_default
) VALUES(
    'default-membrie', 'app', '*membrie*', 1, 0, 'Membrie itself', 1
);
"#;

const MIGRATION_5: &str = r#"
ALTER TABLE capture_state ADD COLUMN screen_enabled INTEGER NOT NULL DEFAULT 0
    CHECK(screen_enabled IN (0, 1));
ALTER TABLE capture_state ADD COLUMN screen_sample_interval_ms INTEGER NOT NULL DEFAULT 120000
    CHECK(screen_sample_interval_ms BETWEEN 30000 AND 300000);
ALTER TABLE capture_state ADD COLUMN screen_model TEXT NOT NULL DEFAULT 'gemma4:e2b';

CREATE TABLE IF NOT EXISTS screen_observations (
    id                  TEXT PRIMARY KEY NOT NULL,
    session_id          TEXT NOT NULL REFERENCES activity_sessions(id) ON DELETE CASCADE,
    observed_at_ms      INTEGER NOT NULL,
    app_id              TEXT NOT NULL,
    app_name            TEXT NOT NULL,
    window_title        TEXT NOT NULL,
    width               INTEGER NOT NULL CHECK(width BETWEEN 1 AND 16384),
    height              INTEGER NOT NULL CHECK(height BETWEEN 1 AND 16384),
    state               TEXT NOT NULL DEFAULT 'processing'
                        CHECK(state IN ('processing', 'complete', 'failed')),
    description         TEXT,
    visible_text        TEXT,
    confidence          TEXT CHECK(confidence IS NULL OR confidence IN ('low', 'medium', 'high')),
    model               TEXT NOT NULL,
    last_error          TEXT,
    image_retained      INTEGER NOT NULL DEFAULT 0 CHECK(image_retained = 0),
    created_at_ms       INTEGER NOT NULL,
    updated_at_ms       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_screen_observations_session
    ON screen_observations(session_id, observed_at_ms);
CREATE INDEX IF NOT EXISTS idx_screen_observations_state
    ON screen_observations(state, observed_at_ms DESC);
"#;

const MIGRATION_6: &str = r#"
ALTER TABLE capture_state ADD COLUMN semantic_enabled INTEGER NOT NULL DEFAULT 0
    CHECK(semantic_enabled IN (0, 1));
ALTER TABLE capture_state ADD COLUMN semantic_sample_interval_ms INTEGER NOT NULL DEFAULT 60000
    CHECK(semantic_sample_interval_ms BETWEEN 30000 AND 300000);

CREATE TABLE IF NOT EXISTS semantic_observations (
    id                  TEXT PRIMARY KEY NOT NULL,
    session_id          TEXT NOT NULL REFERENCES activity_sessions(id) ON DELETE CASCADE,
    observed_at_ms      INTEGER NOT NULL,
    app_id              TEXT NOT NULL,
    app_name            TEXT NOT NULL,
    window_title        TEXT NOT NULL,
    quality             TEXT NOT NULL CHECK(quality IN ('partial', 'rich')),
    visible_nodes       INTEGER NOT NULL CHECK(visible_nodes BETWEEN 0 AND 10000),
    text_nodes          INTEGER NOT NULL CHECK(text_nodes BETWEEN 0 AND 10000),
    document_nodes      INTEGER NOT NULL CHECK(document_nodes BETWEEN 0 AND 10000),
    text_content        TEXT NOT NULL,
    created_at_ms       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_semantic_observations_session
    ON semantic_observations(session_id, observed_at_ms);
CREATE INDEX IF NOT EXISTS idx_semantic_observations_app
    ON semantic_observations(app_id, observed_at_ms DESC);
"#;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("could not prepare data directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Remembrie: {0}")]
    Validation(String),
    #[error("system clock is before the Unix epoch")]
    Clock,
}

pub struct Repository {
    connection: Connection,
    path: PathBuf,
}

#[derive(Debug)]
struct ActivityObservationRecord {
    observed_at_ms: i64,
    app_id: String,
    app_name: String,
    window_title: String,
}

impl Repository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RepositoryError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "secure_delete", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        let mut version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < 1 {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_1)?;
            transaction.pragma_update(None, "user_version", 1)?;
            transaction.commit()?;
            version = 1;
        }
        if version < 2 {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_2)?;
            transaction.pragma_update(None, "user_version", 2)?;
            transaction.commit()?;
            version = 2;
        }
        if version < 3 {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_3)?;
            transaction.pragma_update(None, "user_version", 3)?;
            transaction.commit()?;
            version = 3;
        }
        if version < 4 {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_4)?;
            transaction.pragma_update(None, "user_version", 4)?;
            transaction.commit()?;
            version = 4;
        }
        if version < 5 {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_5)?;
            transaction.pragma_update(None, "user_version", 5)?;
            transaction.commit()?;
            version = 5;
        }
        if version < 6 {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(MIGRATION_6)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
        }

        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), RepositoryError> {
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        self.connection
            .backup(DatabaseName::Main, destination, None)?;

        let backup = Connection::open(destination)?;
        let integrity: String =
            backup.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
        if integrity != "ok" {
            return Err(RepositoryError::Validation(format!(
                "backup integrity check failed: {integrity}"
            )));
        }
        drop(backup);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(destination, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn create(&mut self, input: NewRemembrie) -> Result<Remembrie, RepositoryError> {
        let title = input.title.trim();
        let body = input.body.trim();
        if title.is_empty() && body.is_empty() {
            return Err(RepositoryError::Validation(
                "a Remembrie needs a title or body".to_owned(),
            ));
        }
        if input.kind.trim().is_empty() {
            return Err(RepositoryError::Validation(
                "a Remembrie needs a kind".to_owned(),
            ));
        }

        let now = now_ms()?;
        let occurred_at_ms = input.occurred_at_ms.unwrap_or(now);
        let id = Uuid::now_v7().to_string();
        let content_id = Uuid::now_v7().to_string();
        let display_title = if title.is_empty() {
            body.lines()
                .next()
                .unwrap_or("Untitled Remembrie")
                .chars()
                .take(80)
                .collect()
        } else {
            title.to_owned()
        };

        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO remembries(
                id, kind, occurred_at_ms, source_app, window_title, title,
                sensitivity, importance, pinned, created_at_ms, updated_at_ms
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'normal', 0.5, 0, ?7, ?7)",
            params![
                id,
                input.kind.trim(),
                occurred_at_ms,
                input.source_app,
                input.window_title,
                display_title,
                now,
            ],
        )?;
        transaction.execute(
            "INSERT INTO remembrie_contents(
                id, remembrie_id, role, mime_type, text_content, created_at_ms
             ) VALUES(?1, ?2, 'captured', 'text/plain', ?3, ?4)",
            params![content_id, id, body, now],
        )?;
        transaction.execute(
            "INSERT INTO remembrie_fts(remembrie_id, title, body, summary)
             VALUES(?1, ?2, ?3, '')",
            params![id, display_title, body],
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO processing_jobs(
                id, remembrie_id, kind, state, attempts,
                available_at_ms, created_at_ms, updated_at_ms
             ) VALUES(?1, ?2, 'enrich', 'pending', 0, 0, ?3, ?3)",
            params![format!("enrich:{id}"), id, now],
        )?;
        transaction.commit()?;

        self.get(&id)?.ok_or_else(|| {
            RepositoryError::Validation("created Remembrie could not be loaded".to_owned())
        })
    }

    pub fn capture(
        &mut self,
        candidate: CaptureCandidate,
    ) -> Result<CaptureDecision, RepositoryError> {
        let now = now_ms()?;
        if self.effective_pause(now)?.0 {
            return self.skip_capture(
                &candidate.kind,
                "paused",
                "Capture is currently paused",
                now,
            );
        }

        if candidate.kind == "clipboard" && !self.clipboard_enabled()? {
            return self.skip_capture(
                &candidate.kind,
                "source_disabled",
                "Automatic clipboard capture has not been enabled",
                now,
            );
        }

        if candidate.body.trim().is_empty() {
            return self.skip_capture(
                &candidate.kind,
                "empty",
                "There was no text to remember",
                now,
            );
        }
        if candidate.body.len() > MAX_AUTOMATIC_CONTENT_BYTES {
            return self.skip_capture(
                &candidate.kind,
                "too_large",
                "Automatic content exceeded the 256 KiB safety limit",
                now,
            );
        }

        let rules = self.list_capture_rules()?;
        if let Some(rule) = exclusion_reason(&candidate, &rules) {
            let label = rule.label.as_deref().unwrap_or(&rule.pattern);
            return self.skip_capture(
                &candidate.kind,
                "excluded",
                &format!("Excluded by rule: {label}"),
                now,
            );
        }

        if let Some(reason) = sensitive_reason(&candidate.body) {
            return self.skip_capture(
                &candidate.kind,
                "sensitive",
                &format!("Skipped probable {reason}"),
                now,
            );
        }

        if self.is_recent_duplicate(&candidate, now)? {
            return self.skip_capture(
                &candidate.kind,
                "duplicate",
                "This content was already remembered recently",
                now,
            );
        }

        let remembrie = self.create(NewRemembrie {
            kind: candidate.kind.clone(),
            title: candidate.title,
            body: candidate.body,
            source_app: candidate.source_app,
            window_title: candidate.window_title,
            occurred_at_ms: candidate.occurred_at_ms,
        })?;
        self.record_capture_event(&candidate.kind, "stored", None, Some(&remembrie.id), now)?;
        Ok(CaptureDecision::Stored { remembrie })
    }

    pub fn get(&self, id: &str) -> Result<Option<Remembrie>, RepositoryError> {
        self.connection
            .query_row(
                &format!(
                    "{} WHERE r.id = ?1 AND r.deleted_at_ms IS NULL GROUP BY r.id",
                    remembrie_select()
                ),
                [id],
                map_remembrie,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<Remembrie>, RepositoryError> {
        let limit = limit.clamp(1, 500);
        let sql = format!(
            "{} WHERE r.deleted_at_ms IS NULL
             GROUP BY r.id ORDER BY r.occurred_at_ms DESC LIMIT ?1",
            remembrie_select()
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map([limit], map_remembrie)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn timeline_history(
        &self,
        since_ms: i64,
        until_ms: i64,
    ) -> Result<Vec<TimelineHistorySpan>, RepositoryError> {
        validate_timeline_range(since_ms, until_ms, 400 * 24 * 60 * 60 * 1000)?;
        let mut statement = self.connection.prepare(
            "SELECT kind, occurred_at_ms,
                    MAX(occurred_at_ms, COALESCE(ended_at_ms, occurred_at_ms))
             FROM remembries
             WHERE deleted_at_ms IS NULL
               AND occurred_at_ms < ?2
               AND MAX(occurred_at_ms, COALESCE(ended_at_ms, occurred_at_ms)) >= ?1
             ORDER BY occurred_at_ms
             LIMIT 10000",
        )?;
        let rows = statement.query_map(params![since_ms, until_ms], |row| {
            Ok(TimelineHistorySpan {
                kind: row.get(0)?,
                started_at_ms: row.get(1)?,
                ended_at_ms: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn timeline_day(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<TimelineEntry>, RepositoryError> {
        validate_timeline_range(start_ms, end_ms, 48 * 60 * 60 * 1000)?;
        let records = {
            let mut statement = self.connection.prepare(
                "SELECT id, kind, occurred_at_ms,
                        MAX(occurred_at_ms, COALESCE(ended_at_ms, occurred_at_ms)),
                        source_app, window_title, title, summary
                 FROM remembries
                 WHERE deleted_at_ms IS NULL
                   AND occurred_at_ms < ?2
                   AND MAX(occurred_at_ms, COALESCE(ended_at_ms, occurred_at_ms)) >= ?1
                 ORDER BY occurred_at_ms
                 LIMIT 500",
            )?;
            let rows = statement.query_map(params![start_ms, end_ms], |row| {
                Ok(TimelineEntry {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    started_at_ms: row.get(2)?,
                    ended_at_ms: row.get(3)?,
                    source_app: row.get(4)?,
                    window_title: row.get(5)?,
                    title: row.get(6)?,
                    summary: row.get(7)?,
                    activity: None,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let mut entries = Vec::with_capacity(records.len());
        for mut entry in records {
            if entry.kind == "activity" {
                entry.activity = self.timeline_activity_summary(&entry.id)?;
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    fn timeline_activity_summary(
        &self,
        remembrie_id: &str,
    ) -> Result<Option<TimelineActivitySummary>, RepositoryError> {
        let session = self
            .connection
            .query_row(
                "SELECT id, COALESCE(end_reason, 'unknown')
                 FROM activity_sessions WHERE remembrie_id = ?1 LIMIT 1",
                [remembrie_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((session_id, end_reason)) = session else {
            return Ok(None);
        };
        let observation_count = self.connection.query_row(
            "SELECT COUNT(*) FROM activity_observations WHERE session_id = ?1",
            [&session_id],
            |row| row.get::<_, u64>(0),
        )?;
        let observations = {
            let mut statement = self.connection.prepare(
                "SELECT observed_at_ms, app_id, app_name, window_title
                 FROM activity_observations
                 WHERE session_id = ?1
                 ORDER BY observed_at_ms, created_at_ms
                 LIMIT 1000",
            )?;
            let rows = statement.query_map([&session_id], |row| {
                Ok(TimelineActivityObservation {
                    observed_at_ms: row.get(0)?,
                    app_id: row.get(1)?,
                    app_name: row.get(2)?,
                    window_title: row.get(3)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let semantic_observation_count = self.connection.query_row(
            "SELECT COUNT(*) FROM semantic_observations WHERE session_id = ?1",
            [&session_id],
            |row| row.get::<_, u64>(0),
        )?;
        let screen_observation_count = self.connection.query_row(
            "SELECT COUNT(*) FROM screen_observations
             WHERE session_id = ?1 AND state = 'complete'",
            [&session_id],
            |row| row.get::<_, u64>(0),
        )?;
        Ok(Some(TimelineActivitySummary {
            end_reason,
            observation_count,
            observations,
            semantic_observation_count,
            screen_observation_count,
        }))
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, RepositoryError> {
        let Some(fts_query) = plain_fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.clamp(1, 200);
        let mut statement = self.connection.prepare(
            "SELECT
                r.id, r.kind, r.occurred_at_ms, r.ended_at_ms, r.source_app,
                r.window_title, r.title, COALESCE(c.text_content, ''), r.summary,
                r.sensitivity, r.importance, r.pinned, r.created_at_ms,
                snippet(remembrie_fts, 2, '', '', ' … ', 18),
                remembrie_fts.rank
             FROM remembrie_fts
             JOIN remembries r ON r.id = remembrie_fts.remembrie_id
             LEFT JOIN remembrie_contents c
                ON c.remembrie_id = r.id AND c.role = 'captured'
             WHERE remembrie_fts MATCH ?1 AND r.deleted_at_ms IS NULL
             ORDER BY remembrie_fts.rank
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![fts_query, limit], |row| {
            Ok(SearchHit {
                remembrie: map_remembrie(row)?,
                snippet: row.get(13)?,
                lexical_score: row.get(14)?,
                semantic_score: None,
                combined_score: 0.0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn hybrid_search(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        embedding_model: &str,
        limit: u32,
    ) -> Result<Vec<SearchHit>, RepositoryError> {
        let limit = limit.clamp(1, 100) as usize;
        let lexical_hits = self.search(query, 100)?;
        let Some(query_embedding) = query_embedding.filter(|vector| !vector.is_empty()) else {
            return Ok(lexical_hits
                .into_iter()
                .enumerate()
                .take(limit)
                .map(|(rank, mut hit)| {
                    hit.combined_score = reciprocal_rank(rank);
                    hit
                })
                .collect());
        };

        let mut semantic_by_remembrie: HashMap<String, (f64, String)> = HashMap::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT c.remembrie_id, c.text_content, e.dimensions, e.vector
                 FROM embeddings e
                 JOIN remembrie_chunks c ON c.id = e.chunk_id
                 JOIN remembries r ON r.id = c.remembrie_id
                 WHERE e.model = ?1 AND r.deleted_at_ms IS NULL",
            )?;
            let rows = statement.query_map([embedding_model], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, usize>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?;
            for row in rows {
                let (remembrie_id, text, dimensions, bytes) = row?;
                if dimensions != query_embedding.len() {
                    continue;
                }
                let Some(vector) = decode_embedding(&bytes, dimensions) else {
                    continue;
                };
                let score = cosine_similarity(query_embedding, &vector);
                let entry = semantic_by_remembrie
                    .entry(remembrie_id)
                    .or_insert_with(|| (score, text.clone()));
                if score > entry.0 {
                    *entry = (score, text);
                }
            }
        }

        let mut semantic_hits: Vec<(String, f64, String)> = semantic_by_remembrie
            .into_iter()
            .map(|(id, (score, text))| (id, score, text))
            .collect();
        semantic_hits.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(best_score) = semantic_hits.first().map(|hit| hit.1) {
            semantic_hits.retain(|hit| {
                hit.1 >= MIN_SEMANTIC_SIMILARITY && hit.1 >= best_score - SEMANTIC_SCORE_WINDOW
            });
        }

        let mut fused: HashMap<String, SearchHit> = HashMap::new();
        for (rank, mut hit) in lexical_hits.into_iter().enumerate() {
            hit.combined_score = reciprocal_rank(rank);
            fused.insert(hit.remembrie.id.clone(), hit);
        }
        for (rank, (id, semantic_score, snippet)) in semantic_hits.into_iter().enumerate() {
            let semantic_rank_score = reciprocal_rank(rank);
            if let Some(hit) = fused.get_mut(&id) {
                hit.semantic_score = Some(semantic_score);
                hit.combined_score += semantic_rank_score;
                if hit.snippet.trim().is_empty() {
                    hit.snippet = snippet;
                }
            } else if let Some(remembrie) = self.get(&id)? {
                fused.insert(
                    id,
                    SearchHit {
                        remembrie,
                        snippet,
                        lexical_score: 0.0,
                        semantic_score: Some(semantic_score),
                        combined_score: semantic_rank_score,
                    },
                );
            }
        }

        let mut hits: Vec<SearchHit> = fused.into_values().collect();
        hits.sort_by(|left, right| {
            right
                .combined_score
                .partial_cmp(&left.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn intelligence_settings(&self) -> Result<IntelligenceSettings, RepositoryError> {
        self.connection
            .query_row(
                "SELECT chat_model, embedding_model, context_tokens
                 FROM intelligence_settings WHERE singleton = 1",
                [],
                |row| {
                    Ok(IntelligenceSettings {
                        chat_model: row.get(0)?,
                        embedding_model: row.get(1)?,
                        context_tokens: row.get(2)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn set_intelligence_settings(
        &mut self,
        settings: &IntelligenceSettings,
    ) -> Result<(), RepositoryError> {
        validate_model_name(&settings.chat_model)?;
        validate_model_name(&settings.embedding_model)?;
        if !(2048..=32768).contains(&settings.context_tokens) {
            return Err(RepositoryError::Validation(
                "context size must be between 2048 and 32768 tokens".to_owned(),
            ));
        }

        if self.intelligence_settings()? == *settings {
            return Ok(());
        }

        let now = now_ms()?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "UPDATE intelligence_settings
             SET chat_model = ?1, embedding_model = ?2,
                 context_tokens = ?3, updated_at_ms = ?4
             WHERE singleton = 1",
            params![
                settings.chat_model.trim(),
                settings.embedding_model.trim(),
                settings.context_tokens,
                now
            ],
        )?;
        transaction.execute(
            "UPDATE processing_jobs
             SET state = 'pending', attempts = 0, last_error = NULL,
                 available_at_ms = 0, updated_at_ms = ?1
             WHERE kind = 'enrich'",
            [now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn reset_interrupted_processing(&self) -> Result<(), RepositoryError> {
        let now = now_ms()?;
        self.connection.execute(
            "INSERT OR IGNORE INTO processing_jobs(
                id, remembrie_id, kind, state, attempts,
                available_at_ms, created_at_ms, updated_at_ms
             )
             SELECT 'enrich:' || id, id, 'enrich', 'pending', 0, 0,
                    created_at_ms, created_at_ms
             FROM remembries WHERE deleted_at_ms IS NULL",
            [],
        )?;
        self.connection.execute(
            "UPDATE processing_jobs
             SET state = 'pending', available_at_ms = 0, updated_at_ms = ?1
             WHERE state = 'running'",
            [now],
        )?;
        self.connection.execute(
            "UPDATE screen_observations
             SET state = 'failed', last_error = 'local analysis was interrupted',
                 updated_at_ms = ?1
             WHERE state = 'processing'",
            [now],
        )?;
        self.connection.execute(
            "UPDATE processing_jobs
             SET state = 'pending', attempts = 0, last_error = NULL,
                 available_at_ms = 0, updated_at_ms = ?1
             WHERE state = 'pending' AND available_at_ms > ?1
               AND remembrie_id IN (
                 SELECT remembrie_id FROM activity_sessions
                 WHERE remembrie_id IS NOT NULL
             )",
            [now],
        )?;
        Ok(())
    }

    pub fn claim_processing_job(&mut self) -> Result<Option<ProcessingJob>, RepositoryError> {
        let now = now_ms()?;
        let transaction = self.connection.transaction()?;
        let candidate = transaction
            .query_row(
                "SELECT id, remembrie_id, attempts
                 FROM processing_jobs
                 WHERE kind = 'enrich' AND state = 'pending' AND available_at_ms <= ?1
                 ORDER BY available_at_ms, created_at_ms
                 LIMIT 1",
                [now],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((job_id, remembrie_id, attempts)) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };
        transaction.execute(
            "UPDATE processing_jobs
             SET state = 'running', attempts = attempts + 1,
                 last_error = NULL, updated_at_ms = ?2
             WHERE id = ?1",
            params![job_id, now],
        )?;
        transaction.commit()?;

        let Some(remembrie) = self.get(&remembrie_id)? else {
            self.connection
                .execute("DELETE FROM processing_jobs WHERE id = ?1", [&job_id])?;
            return Ok(None);
        };
        Ok(Some(ProcessingJob {
            id: job_id,
            remembrie,
            attempts: attempts + 1,
        }))
    }

    pub fn complete_enrichment(
        &mut self,
        job_id: &str,
        summary: &str,
        chat_model: &str,
        embedding_model: &str,
        chunks: &[EmbeddedChunk],
    ) -> Result<bool, RepositoryError> {
        if chunks.is_empty() || chunks.iter().any(|chunk| chunk.embedding.is_empty()) {
            return Err(RepositoryError::Validation(
                "enrichment requires at least one embedded chunk".to_owned(),
            ));
        }
        let dimensions = chunks[0].embedding.len();
        if chunks
            .iter()
            .any(|chunk| chunk.embedding.len() != dimensions)
        {
            return Err(RepositoryError::Validation(
                "all chunk embeddings must have the same dimensions".to_owned(),
            ));
        }

        let current = self.intelligence_settings()?;
        if current.chat_model != chat_model || current.embedding_model != embedding_model {
            self.connection.execute(
                "UPDATE processing_jobs
                 SET state = 'pending', attempts = 0, available_at_ms = 0,
                     last_error = NULL, updated_at_ms = ?2
                 WHERE id = ?1",
                params![job_id, now_ms()?],
            )?;
            return Ok(false);
        }

        let remembrie_id: String = self.connection.query_row(
            "SELECT remembrie_id FROM processing_jobs WHERE id = ?1",
            [job_id],
            |row| row.get(0),
        )?;
        let now = now_ms()?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM remembrie_chunks WHERE remembrie_id = ?1",
            [&remembrie_id],
        )?;
        for chunk in chunks {
            let chunk_id = Uuid::now_v7().to_string();
            transaction.execute(
                "INSERT INTO remembrie_chunks(
                    id, remembrie_id, content_id, ordinal, start_offset,
                    end_offset, text_content, token_count
                 ) VALUES(?1, ?2, NULL, ?3, ?4, ?5, ?6, NULL)",
                params![
                    chunk_id,
                    remembrie_id,
                    chunk.ordinal,
                    chunk.start_offset,
                    chunk.end_offset,
                    chunk.text
                ],
            )?;
            transaction.execute(
                "INSERT INTO embeddings(
                    chunk_id, model, dimensions, vector, created_at_ms
                 ) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    chunk_id,
                    embedding_model,
                    dimensions,
                    encode_embedding(&chunk.embedding),
                    now
                ],
            )?;
        }
        transaction.execute(
            "DELETE FROM derived_artifacts
             WHERE remembrie_id = ?1 AND kind = 'summary'",
            [&remembrie_id],
        )?;
        transaction.execute(
            "INSERT INTO derived_artifacts(
                id, remembrie_id, kind, text_content, model, prompt_version, created_at_ms
             ) VALUES(?1, ?2, 'summary', ?3, ?4, 'summary-v1', ?5)",
            params![
                Uuid::now_v7().to_string(),
                remembrie_id,
                summary.trim(),
                chat_model,
                now
            ],
        )?;
        transaction.execute(
            "UPDATE remembries SET summary = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![summary.trim(), now, remembrie_id],
        )?;
        transaction.execute(
            "UPDATE remembrie_fts SET summary = ?1 WHERE remembrie_id = ?2",
            params![summary.trim(), remembrie_id],
        )?;
        transaction.execute(
            "UPDATE processing_jobs
             SET state = 'complete', last_error = NULL, updated_at_ms = ?2
             WHERE id = ?1",
            params![job_id, now],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn fail_processing_job(&self, job_id: &str, error: &str) -> Result<(), RepositoryError> {
        let now = now_ms()?;
        let safe_error: String = error.chars().take(1000).collect();
        self.connection.execute(
            "UPDATE processing_jobs
             SET state = CASE WHEN attempts >= 5 THEN 'failed' ELSE 'pending' END,
                 last_error = ?2,
                 available_at_ms = ?3 + MIN(attempts * 60000, 600000),
                 updated_at_ms = ?3
             WHERE id = ?1",
            params![job_id, safe_error, now],
        )?;
        Ok(())
    }

    pub fn retry_failed_processing(&self) -> Result<u64, RepositoryError> {
        Ok(self.connection.execute(
            "UPDATE processing_jobs
             SET state = 'pending', attempts = 0, last_error = NULL,
                 available_at_ms = 0, updated_at_ms = ?1
             WHERE kind = 'enrich' AND state = 'failed'",
            [now_ms()?],
        )? as u64)
    }

    pub fn intelligence_status(
        &self,
        ollama_available: bool,
        available_models: Vec<LocalModel>,
    ) -> Result<IntelligenceStatus, RepositoryError> {
        let settings = self.intelligence_settings()?;
        let total_remembries = self.connection.query_row(
            "SELECT COUNT(*) FROM remembries WHERE deleted_at_ms IS NULL",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        let indexed_remembries = self.connection.query_row(
            "SELECT COUNT(DISTINCT c.remembrie_id)
             FROM embeddings e
             JOIN remembrie_chunks c ON c.id = e.chunk_id
             JOIN remembries r ON r.id = c.remembrie_id
             WHERE e.model = ?1 AND r.deleted_at_ms IS NULL",
            [&settings.embedding_model],
            |row| row.get::<_, u64>(0),
        )?;
        let (pending_jobs, running_jobs, failed_jobs) = self.connection.query_row(
            "SELECT
                COALESCE(SUM(state = 'pending'), 0),
                COALESCE(SUM(state = 'running'), 0),
                COALESCE(SUM(state = 'failed'), 0)
             FROM processing_jobs WHERE kind = 'enrich'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let last_error = self
            .connection
            .query_row(
                "SELECT last_error FROM processing_jobs
                 WHERE last_error IS NOT NULL
                 ORDER BY updated_at_ms DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(IntelligenceStatus {
            ollama_available,
            settings,
            available_models,
            total_remembries,
            indexed_remembries,
            pending_jobs,
            running_jobs,
            failed_jobs,
            last_error,
        })
    }

    pub fn status(&self) -> Result<CaptureStatus, RepositoryError> {
        let (paused, paused_until_ms) = self.effective_pause(now_ms()?)?;
        let (
            clipboard_enabled,
            activity_enabled,
            activity_idle_threshold_ms,
            semantic_enabled,
            semantic_sample_interval_ms,
            screen_enabled,
            screen_sample_interval_ms,
            screen_model,
        ) = self.connection.query_row(
            "SELECT clipboard_enabled, activity_enabled, activity_idle_threshold_ms,
                        semantic_enabled, semantic_sample_interval_ms,
                        screen_enabled, screen_sample_interval_ms, screen_model
                 FROM capture_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )?;
        let count = self.connection.query_row(
            "SELECT COUNT(*) FROM remembries WHERE deleted_at_ms IS NULL",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        let (skipped_total, skipped_sensitive, skipped_duplicate) = self.connection.query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(outcome = 'sensitive'), 0),
                COALESCE(SUM(outcome = 'duplicate'), 0)
             FROM capture_events
             WHERE outcome <> 'stored'",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )?;
        let activity_session_count = self.connection.query_row(
            "SELECT COUNT(*) FROM activity_sessions WHERE ended_at_ms IS NOT NULL",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        let current_activity = self
            .connection
            .query_row(
                "SELECT s.started_at_ms, o.app_name, o.window_title
                 FROM activity_sessions s
                 LEFT JOIN activity_observations o ON o.id = (
                    SELECT id FROM activity_observations
                    WHERE session_id = s.id
                    ORDER BY observed_at_ms DESC, created_at_ms DESC LIMIT 1
                 )
                 WHERE s.ended_at_ms IS NULL",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        let (screen_observation_count, screen_failed_count, screen_processing_count) =
            self.connection.query_row(
                "SELECT
                COALESCE(SUM(state = 'complete'), 0),
                COALESCE(SUM(state = 'failed'), 0),
                COALESCE(SUM(state = 'processing'), 0)
             FROM screen_observations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let last_screen_observation = self
            .connection
            .query_row(
                "SELECT observed_at_ms, app_name
                 FROM screen_observations
                 WHERE state = 'complete'
                 ORDER BY observed_at_ms DESC, created_at_ms DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let semantic_observation_count =
            self.connection
                .query_row("SELECT COUNT(*) FROM semantic_observations", [], |row| {
                    row.get::<_, u64>(0)
                })?;
        let last_semantic_observation = self
            .connection
            .query_row(
                "SELECT observed_at_ms, app_name
                 FROM semantic_observations
                 ORDER BY observed_at_ms DESC, created_at_ms DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(CaptureStatus {
            paused,
            paused_until_ms,
            clipboard_enabled,
            remembrie_count: count,
            skipped_total,
            skipped_sensitive,
            skipped_duplicate,
            clipboard_agent_last_seen_ms: None,
            activity_enabled,
            activity_idle_threshold_ms,
            activity_session_count,
            activity_active_since_ms: current_activity.as_ref().map(|activity| activity.0),
            activity_current_app: current_activity
                .as_ref()
                .and_then(|activity| activity.1.clone()),
            activity_current_window: current_activity.and_then(|activity| activity.2),
            semantic_enabled,
            semantic_sample_interval_ms,
            semantic_observation_count,
            semantic_last_observed_at_ms: last_semantic_observation
                .as_ref()
                .map(|observation| observation.0),
            semantic_last_app: last_semantic_observation.map(|observation| observation.1),
            screen_enabled,
            screen_sample_interval_ms,
            screen_model,
            screen_observation_count,
            screen_failed_count,
            screen_processing_count,
            screen_last_observed_at_ms: last_screen_observation
                .as_ref()
                .map(|observation| observation.0),
            screen_last_app: last_screen_observation.map(|observation| observation.1),
            database_path: self.path.display().to_string(),
        })
    }

    pub fn set_pause(&mut self, mode: PauseMode) -> Result<CaptureStatus, RepositoryError> {
        let now = now_ms()?;
        let (paused, paused_until_ms) = match mode {
            PauseMode::Resume => (false, None),
            PauseMode::Until { timestamp_ms } => {
                if timestamp_ms <= now {
                    return Err(RepositoryError::Validation(
                        "pause time must be in the future".to_owned(),
                    ));
                }
                (true, Some(timestamp_ms))
            }
            PauseMode::Indefinite => (true, None),
        };
        self.connection.execute(
            "UPDATE capture_state
             SET paused = ?1, paused_until_ms = ?2, updated_at_ms = ?3
             WHERE singleton = 1",
            params![paused, paused_until_ms, now],
        )?;
        if paused {
            self.finish_active_activity_session(now, "capture_paused")?;
        }
        self.status()
    }

    pub fn set_clipboard_enabled(
        &self,
        clipboard_enabled: bool,
    ) -> Result<CaptureStatus, RepositoryError> {
        self.connection.execute(
            "UPDATE capture_state
             SET clipboard_enabled = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![clipboard_enabled, now_ms()?],
        )?;
        self.status()
    }

    pub fn set_activity_enabled(
        &mut self,
        activity_enabled: bool,
    ) -> Result<CaptureStatus, RepositoryError> {
        let now = now_ms()?;
        self.connection.execute(
            "UPDATE capture_state
             SET activity_enabled = ?1,
                 semantic_enabled = CASE WHEN ?1 THEN semantic_enabled ELSE 0 END,
                 screen_enabled = CASE WHEN ?1 THEN screen_enabled ELSE 0 END,
                 updated_at_ms = ?2
             WHERE singleton = 1",
            params![activity_enabled, now],
        )?;
        if !activity_enabled {
            self.finish_active_activity_session(now, "activity_disabled")?;
        }
        self.status()
    }

    pub fn set_semantic_enabled(
        &self,
        semantic_enabled: bool,
    ) -> Result<CaptureStatus, RepositoryError> {
        let activity_enabled = self.connection.query_row(
            "SELECT activity_enabled FROM capture_state WHERE singleton = 1",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if semantic_enabled && !activity_enabled {
            return Err(RepositoryError::Validation(
                "enable activity context before enabling semantic context".to_owned(),
            ));
        }
        self.connection.execute(
            "UPDATE capture_state
             SET semantic_enabled = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![semantic_enabled, now_ms()?],
        )?;
        self.status()
    }

    pub fn set_semantic_sample_interval(
        &self,
        sample_interval_ms: u64,
    ) -> Result<CaptureStatus, RepositoryError> {
        if !(30_000..=300_000).contains(&sample_interval_ms) {
            return Err(RepositoryError::Validation(
                "the semantic-context interval must be between 30 seconds and 5 minutes".to_owned(),
            ));
        }
        self.connection.execute(
            "UPDATE capture_state
             SET semantic_sample_interval_ms = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![sample_interval_ms, now_ms()?],
        )?;
        self.status()
    }

    pub fn set_screen_enabled(
        &self,
        screen_enabled: bool,
    ) -> Result<CaptureStatus, RepositoryError> {
        let activity_enabled = self.connection.query_row(
            "SELECT activity_enabled FROM capture_state WHERE singleton = 1",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if screen_enabled && !activity_enabled {
            return Err(RepositoryError::Validation(
                "enable activity context before enabling screen memory".to_owned(),
            ));
        }
        self.connection.execute(
            "UPDATE capture_state
             SET screen_enabled = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![screen_enabled, now_ms()?],
        )?;
        self.status()
    }

    pub fn set_screen_sample_interval(
        &self,
        sample_interval_ms: u64,
    ) -> Result<CaptureStatus, RepositoryError> {
        if !(30_000..=300_000).contains(&sample_interval_ms) {
            return Err(RepositoryError::Validation(
                "the screen-memory interval must be between 30 seconds and 5 minutes".to_owned(),
            ));
        }
        self.connection.execute(
            "UPDATE capture_state
             SET screen_sample_interval_ms = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![sample_interval_ms, now_ms()?],
        )?;
        self.status()
    }

    pub fn set_screen_model(&self, model: &str) -> Result<CaptureStatus, RepositoryError> {
        validate_model_name(model)?;
        self.connection.execute(
            "UPDATE capture_state
             SET screen_model = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![model.trim(), now_ms()?],
        )?;
        self.status()
    }

    pub fn set_activity_idle_threshold(
        &self,
        idle_threshold_ms: u64,
    ) -> Result<CaptureStatus, RepositoryError> {
        if !(300_000..=14_400_000).contains(&idle_threshold_ms) {
            return Err(RepositoryError::Validation(
                "the activity idle threshold must be between 5 minutes and 4 hours".to_owned(),
            ));
        }
        self.connection.execute(
            "UPDATE capture_state
             SET activity_idle_threshold_ms = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![idle_threshold_ms, now_ms()?],
        )?;
        self.status()
    }

    pub fn record_activity_snapshot(
        &mut self,
        snapshot: ActivitySnapshot,
    ) -> Result<ActivityRecordResult, RepositoryError> {
        validate_activity_text("application identifier", &snapshot.app_id, 512)?;
        validate_activity_text("application name", &snapshot.app_name, 512)?;
        validate_activity_text("window title", &snapshot.window_title, 4096)?;
        let now = snapshot.occurred_at_ms.unwrap_or(now_ms()?);
        if now < 0 {
            return Err(RepositoryError::Validation(
                "activity timestamps cannot be negative".to_owned(),
            ));
        }

        let (paused, _) = self.effective_pause(now)?;
        let (activity_enabled, idle_threshold_ms, semantic_enabled, screen_enabled) =
            self.connection.query_row(
                "SELECT activity_enabled, activity_idle_threshold_ms,
                    semantic_enabled, screen_enabled
             FROM capture_state WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, bool>(3)?,
                    ))
                },
            )?;
        if !activity_enabled {
            return Ok(activity_result(
                "ignored",
                Some("activity capture is disabled"),
                None,
            ));
        }

        let bounded_idle = snapshot.idle_ms.min(now as u64);
        let last_active_at_ms = now.saturating_sub(bounded_idle as i64);
        if paused {
            self.finish_active_activity_session(last_active_at_ms, "capture_paused")?;
            return Ok(activity_result("ignored", Some("capture is paused"), None));
        }
        if snapshot.locked {
            self.finish_active_activity_session(last_active_at_ms, "screen_locked")?;
            return Ok(activity_result(
                "boundary",
                Some("the screen is locked"),
                None,
            ));
        }
        if snapshot.idle_ms >= idle_threshold_ms {
            self.finish_active_activity_session(last_active_at_ms, "idle")?;
            return Ok(activity_result(
                "boundary",
                Some("the computer is idle"),
                None,
            ));
        }

        let app_id = snapshot.app_id.trim();
        let app_name = snapshot.app_name.trim();
        let window_title = snapshot.window_title.trim();
        let source_app = match (app_name.is_empty(), app_id.is_empty()) {
            (false, false) if !app_name.eq_ignore_ascii_case(app_id) => {
                format!("{app_name} ({app_id})")
            }
            (false, _) => app_name.to_owned(),
            (_, false) => app_id.to_owned(),
            _ => "Desktop".to_owned(),
        };
        let candidate = CaptureCandidate {
            kind: "activity".to_owned(),
            title: "Desktop activity".to_owned(),
            body: window_title.to_owned(),
            source_app: Some(source_app),
            window_title: (!window_title.is_empty()).then(|| window_title.to_owned()),
            source_uri: None,
            occurred_at_ms: Some(now),
        };
        let rules = self.list_capture_rules()?;
        if let Some(rule) = exclusion_reason(&candidate, &rules) {
            self.finish_active_activity_session(last_active_at_ms, "excluded_context")?;
            let label = rule.label.as_deref().unwrap_or("a privacy rule");
            return Ok(activity_result(
                "skipped",
                Some(&format!("excluded by {label}")),
                None,
            ));
        }
        if sensitive_reason(window_title).is_some() {
            self.finish_active_activity_session(last_active_at_ms, "sensitive_context")?;
            return Ok(activity_result(
                "skipped",
                Some("the window title looked sensitive"),
                None,
            ));
        }

        let active_session = self
            .connection
            .query_row(
                "SELECT id FROM activity_sessions WHERE ended_at_ms IS NULL",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if active_session.is_none() && snapshot.idle_ms > 60_000 {
            return Ok(activity_result(
                "ignored",
                Some("waiting for new user activity"),
                None,
            ));
        }

        let session_id = if let Some(id) = active_session {
            id
        } else {
            let id = Uuid::now_v7().to_string();
            self.connection.execute(
                "INSERT INTO activity_sessions(
                    id, started_at_ms, ended_at_ms, last_active_at_ms,
                    end_reason, remembrie_id, created_at_ms, updated_at_ms
                 ) VALUES(?1, ?2, NULL, ?2, NULL, NULL, ?2, ?2)",
                params![id, now],
            )?;
            id
        };
        self.connection.execute(
            "UPDATE activity_sessions
             SET last_active_at_ms = MAX(last_active_at_ms, ?2), updated_at_ms = ?3
             WHERE id = ?1",
            params![session_id, last_active_at_ms, now],
        )?;

        let previous = self
            .connection
            .query_row(
                "SELECT app_id, app_name, window_title
                 FROM activity_observations WHERE session_id = ?1
                 ORDER BY observed_at_ms DESC, created_at_ms DESC LIMIT 1",
                [&session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let current = (
            app_id.to_owned(),
            app_name.to_owned(),
            window_title.to_owned(),
        );
        if previous.as_ref() != Some(&current) {
            let observation_count = self.connection.query_row(
                "SELECT COUNT(*) FROM activity_observations WHERE session_id = ?1",
                [&session_id],
                |row| row.get::<_, u64>(0),
            )?;
            if observation_count < 1000 {
                self.connection.execute(
                    "INSERT INTO activity_observations(
                        id, session_id, observed_at_ms, app_id, app_name,
                        window_title, created_at_ms
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?3)",
                    params![
                        Uuid::now_v7().to_string(),
                        session_id,
                        now,
                        app_id,
                        app_name,
                        window_title
                    ],
                )?;
            }
        }

        let mut result = activity_result("recorded", None, Some(&session_id));
        result.semantic_capture_allowed = semantic_enabled;
        result.screen_capture_allowed = screen_enabled;
        Ok(result)
    }

    pub fn end_activity_session(&mut self, reason: &str) -> Result<(), RepositoryError> {
        if !matches!(
            reason,
            "desktop_bridge_unavailable" | "service_stopped" | "daemon_restart" | "manual"
        ) {
            return Err(RepositoryError::Validation(
                "unsupported activity-session boundary".to_owned(),
            ));
        }
        self.finish_active_activity_session(now_ms()?, reason)?;
        Ok(())
    }

    pub fn reset_interrupted_activity(&mut self) -> Result<(), RepositoryError> {
        self.finish_active_activity_session(now_ms()?, "daemon_restart")?;
        Ok(())
    }

    pub fn record_semantic_observation(
        &mut self,
        candidate: &SemanticCaptureCandidate,
    ) -> Result<SemanticCaptureResult, RepositoryError> {
        validate_semantic_candidate(candidate)?;
        let wall_now = now_ms()?;
        let observed_at_ms = candidate.observed_at_ms.unwrap_or(wall_now);
        let (paused, _) = self.effective_pause(observed_at_ms)?;
        let (activity_enabled, semantic_enabled) = self.connection.query_row(
            "SELECT activity_enabled, semantic_enabled
             FROM capture_state WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
        )?;
        if paused || !activity_enabled || !semantic_enabled {
            return self.skip_semantic_observation(
                "disabled",
                "Semantic Context is paused or disabled",
                observed_at_ms,
            );
        }

        let session_started_at_ms = self
            .connection
            .query_row(
                "SELECT started_at_ms FROM activity_sessions
                 WHERE id = ?1 AND ended_at_ms IS NULL",
                [&candidate.session_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(session_started_at_ms) = session_started_at_ms else {
            return self.skip_semantic_observation(
                "session_ended",
                "the activity session ended before semantic context arrived",
                observed_at_ms,
            );
        };

        let snapshot = CaptureCandidate {
            kind: "semantic".to_owned(),
            title: "Semantic context".to_owned(),
            body: candidate.text_content.clone(),
            source_app: Some(activity_source_app(&candidate.app_id, &candidate.app_name)),
            window_title: (!candidate.window_title.trim().is_empty())
                .then(|| candidate.window_title.clone()),
            source_uri: None,
            occurred_at_ms: candidate.observed_at_ms,
        };
        let rules = self.list_capture_rules()?;
        if let Some(rule) = exclusion_reason(&snapshot, &rules) {
            let label = rule.label.as_deref().unwrap_or("a privacy rule");
            return self.skip_semantic_observation(
                "excluded",
                &format!("semantic context is excluded by {label}"),
                observed_at_ms,
            );
        }
        if sensitive_reason(&candidate.window_title).is_some()
            || sensitive_reason(&candidate.text_content).is_some()
        {
            return self.skip_semantic_observation(
                "sensitive",
                "semantic context looked sensitive",
                observed_at_ms,
            );
        }
        if observed_at_ms < session_started_at_ms || observed_at_ms > wall_now + 60_000 {
            return Err(RepositoryError::Validation(
                "semantic timestamp is outside the active session".to_owned(),
            ));
        }

        let text_content = normalize_semantic_text(&candidate.text_content);
        let duplicate = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM semantic_observations
                WHERE session_id = ?1 AND app_id = ?2 AND app_name = ?3
                  AND window_title = ?4 AND text_content = ?5
             )",
            params![
                candidate.session_id,
                candidate.app_id.trim(),
                candidate.app_name.trim(),
                candidate.window_title.trim(),
                text_content,
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if duplicate {
            return self.skip_semantic_observation(
                "duplicate",
                "semantic context has not materially changed",
                observed_at_ms,
            );
        }
        let observation_count = self.connection.query_row(
            "SELECT COUNT(*) FROM semantic_observations WHERE session_id = ?1",
            [&candidate.session_id],
            |row| row.get::<_, u64>(0),
        )?;
        if observation_count >= 1000 {
            return self.skip_semantic_observation(
                "limit",
                "the activity session reached its semantic observation limit",
                observed_at_ms,
            );
        }

        let observation_id = Uuid::now_v7().to_string();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO semantic_observations(
                id, session_id, observed_at_ms, app_id, app_name, window_title,
                quality, visible_nodes, text_nodes, document_nodes,
                text_content, created_at_ms
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                observation_id,
                candidate.session_id,
                observed_at_ms,
                candidate.app_id.trim(),
                candidate.app_name.trim(),
                candidate.window_title.trim(),
                candidate.quality,
                candidate.visible_nodes,
                candidate.text_nodes,
                candidate.document_nodes,
                text_content,
                wall_now,
            ],
        )?;
        transaction.execute(
            "INSERT INTO capture_events(
                id, occurred_at_ms, source_kind, outcome, reason, remembrie_id
             ) VALUES(?1, ?2, 'semantic', 'stored', NULL, NULL)",
            params![Uuid::now_v7().to_string(), observed_at_ms],
        )?;
        transaction.commit()?;
        Ok(semantic_capture_result(
            "stored",
            None,
            Some(&observation_id),
        ))
    }

    fn skip_semantic_observation(
        &self,
        outcome: &str,
        reason: &str,
        observed_at_ms: i64,
    ) -> Result<SemanticCaptureResult, RepositoryError> {
        self.record_capture_event("semantic", outcome, Some(reason), None, observed_at_ms)?;
        Ok(semantic_capture_result("skipped", Some(reason), None))
    }

    fn authorize_screen_candidate(
        &self,
        candidate: &ScreenCaptureCandidate,
    ) -> Result<String, RepositoryError> {
        validate_screen_candidate(candidate)?;
        let wall_now = now_ms()?;
        let observed_at_ms = candidate.observed_at_ms.unwrap_or(wall_now);
        let (paused, _) = self.effective_pause(observed_at_ms)?;
        let (activity_enabled, screen_enabled, screen_model) = self.connection.query_row(
            "SELECT activity_enabled, screen_enabled, screen_model
                 FROM capture_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        if paused || !activity_enabled || !screen_enabled {
            return Err(RepositoryError::Validation(
                "screen memory is paused or disabled".to_owned(),
            ));
        }
        let session_exists = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM activity_sessions
                WHERE id = ?1 AND ended_at_ms IS NULL
             )",
            [&candidate.session_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !session_exists {
            return Err(RepositoryError::Validation(
                "the activity session ended before screen analysis began".to_owned(),
            ));
        }

        let snapshot = CaptureCandidate {
            kind: "screen".to_owned(),
            title: "Screen memory".to_owned(),
            body: candidate.window_title.clone(),
            source_app: Some(activity_source_app(&candidate.app_id, &candidate.app_name)),
            window_title: (!candidate.window_title.trim().is_empty())
                .then(|| candidate.window_title.clone()),
            source_uri: None,
            occurred_at_ms: candidate.observed_at_ms,
        };
        let rules = self.list_capture_rules()?;
        if let Some(rule) = exclusion_reason(&snapshot, &rules) {
            let label = rule.label.as_deref().unwrap_or("a privacy rule");
            return Err(RepositoryError::Validation(format!(
                "screen context is excluded by {label}"
            )));
        }
        if sensitive_reason(&candidate.window_title).is_some() {
            return Err(RepositoryError::Validation(
                "the active window title looked sensitive".to_owned(),
            ));
        }
        let session_started_at_ms = self.connection.query_row(
            "SELECT started_at_ms FROM activity_sessions WHERE id = ?1",
            [&candidate.session_id],
            |row| row.get::<_, i64>(0),
        )?;
        if observed_at_ms < session_started_at_ms || observed_at_ms > wall_now + 60_000 {
            return Err(RepositoryError::Validation(
                "screen timestamp is outside the active session".to_owned(),
            ));
        }
        validate_model_name(&screen_model)?;
        Ok(screen_model)
    }

    pub fn begin_screen_analysis(
        &self,
        candidate: &ScreenCaptureCandidate,
    ) -> Result<(String, String), RepositoryError> {
        let model = self.authorize_screen_candidate(candidate)?;
        let observation_id = Uuid::now_v7().to_string();
        let now = now_ms()?;
        let observed_at_ms = candidate.observed_at_ms.unwrap_or(now);
        self.connection.execute(
            "INSERT INTO screen_observations(
                id, session_id, observed_at_ms, app_id, app_name, window_title,
                width, height, state, description, visible_text, confidence,
                model, last_error, image_retained, created_at_ms, updated_at_ms
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'processing', NULL, NULL,
                      NULL, ?9, NULL, 0, ?10, ?10)",
            params![
                observation_id,
                candidate.session_id,
                observed_at_ms,
                candidate.app_id.trim(),
                candidate.app_name.trim(),
                candidate.window_title.trim(),
                candidate.width,
                candidate.height,
                model,
                now,
            ],
        )?;
        Ok((observation_id, model))
    }

    pub fn complete_screen_analysis(
        &mut self,
        observation_id: &str,
        analysis: &ScreenAnalysis,
    ) -> Result<ScreenCaptureResult, RepositoryError> {
        validate_activity_text("screen observation identifier", observation_id, 128)?;
        if observation_id.trim().is_empty() {
            return Err(RepositoryError::Validation(
                "screen analysis needs an observation identifier".to_owned(),
            ));
        }
        validate_screen_analysis(analysis)?;
        let pending = self
            .connection
            .query_row(
                "SELECT session_id, observed_at_ms, model
                 FROM screen_observations
                 WHERE id = ?1 AND state = 'processing'",
                [observation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((session_id, observed_at_ms, model)) = pending else {
            return Err(RepositoryError::Validation(
                "screen observation is no longer awaiting analysis".to_owned(),
            ));
        };
        let now = now_ms()?;
        let (paused, _) = self.effective_pause(now)?;
        let (activity_enabled, screen_enabled, current_model) = self.connection.query_row(
            "SELECT activity_enabled, screen_enabled, screen_model
             FROM capture_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        if paused || !activity_enabled || !screen_enabled {
            self.discard_screen_analysis(
                observation_id,
                &session_id,
                "Screen Memory was paused or disabled while analysis was running",
                "disabled",
                observed_at_ms,
            )?;
            return Ok(screen_capture_result(
                "skipped",
                Some("Screen Memory was paused or disabled while analysis was running"),
                None,
            ));
        }
        if analysis.model != model || current_model != model {
            self.discard_screen_analysis(
                observation_id,
                &session_id,
                "the screen model changed while analysis was running",
                "model_changed",
                observed_at_ms,
            )?;
            return Ok(screen_capture_result(
                "skipped",
                Some("the screen model changed while analysis was running"),
                None,
            ));
        }
        let combined = format!("{}\n{}", analysis.description, analysis.visible_text);
        if sensitive_reason(&combined).is_some() {
            self.discard_screen_analysis(
                observation_id,
                &session_id,
                "local analysis found content that looked sensitive",
                "sensitive",
                observed_at_ms,
            )?;
            return Ok(screen_capture_result(
                "skipped",
                Some("local analysis found content that looked sensitive"),
                None,
            ));
        }
        if analysis.description.trim().is_empty() && analysis.visible_text.trim().is_empty() {
            self.discard_screen_analysis(
                observation_id,
                &session_id,
                "the local screen model found no useful visible context",
                "empty",
                observed_at_ms,
            )?;
            return Ok(screen_capture_result(
                "skipped",
                Some("the local screen model found no useful visible context"),
                None,
            ));
        }

        let updated = self.connection.execute(
            "UPDATE screen_observations
             SET state = 'complete', description = ?2, visible_text = ?3,
                 confidence = ?4, last_error = NULL, updated_at_ms = ?5
             WHERE id = ?1 AND state = 'processing'",
            params![
                observation_id,
                analysis.description.trim(),
                analysis.visible_text.trim(),
                analysis.confidence,
                now,
            ],
        )?;
        if updated == 0 {
            return Err(RepositoryError::Validation(
                "screen observation was resolved by another worker".to_owned(),
            ));
        }
        self.materialize_late_screen_observation(&session_id, observation_id)?;
        self.record_capture_event("screen", "stored", None, None, observed_at_ms)?;
        Ok(screen_capture_result("stored", None, Some(observation_id)))
    }

    pub fn fail_screen_analysis(
        &mut self,
        observation_id: &str,
        error: &str,
    ) -> Result<(), RepositoryError> {
        validate_activity_text("screen observation identifier", observation_id, 128)?;
        let session_id = self
            .connection
            .query_row(
                "SELECT session_id FROM screen_observations
                 WHERE id = ?1 AND state = 'processing'",
                [observation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(session_id) = session_id else {
            return Ok(());
        };
        let now = now_ms()?;
        let safe_error: String = error.chars().take(500).collect();
        self.connection.execute(
            "UPDATE screen_observations
             SET state = 'failed', last_error = ?2, updated_at_ms = ?3
             WHERE id = ?1 AND state = 'processing'",
            params![observation_id, safe_error, now],
        )?;
        self.release_enrichment_after_screen_work(&session_id)?;
        Ok(())
    }

    fn discard_screen_analysis(
        &mut self,
        observation_id: &str,
        session_id: &str,
        reason: &str,
        outcome: &str,
        observed_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.connection.execute(
            "DELETE FROM screen_observations
             WHERE id = ?1 AND state = 'processing'",
            [observation_id],
        )?;
        self.record_capture_event("screen", outcome, Some(reason), None, observed_at_ms)?;
        self.release_enrichment_after_screen_work(session_id)
    }

    fn materialize_late_screen_observation(
        &mut self,
        session_id: &str,
        observation_id: &str,
    ) -> Result<(), RepositoryError> {
        let materialized = self
            .connection
            .query_row(
                "SELECT s.started_at_ms, s.remembrie_id, c.id, c.text_content
                 FROM activity_sessions s
                 JOIN remembries r ON r.id = s.remembrie_id
                 JOIN remembrie_contents c
                   ON c.remembrie_id = r.id AND c.role = 'captured'
                 WHERE s.id = ?1 AND s.ended_at_ms IS NOT NULL
                 LIMIT 1",
                [session_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((started_at_ms, remembrie_id, content_id, mut body)) = materialized else {
            return Ok(());
        };
        let observation = self.connection.query_row(
            "SELECT observed_at_ms, app_name, window_title, description,
                    visible_text, confidence, model
             FROM screen_observations
             WHERE id = ?1 AND state = 'complete'",
            [observation_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )?;
        let intro = screen_context_intro();
        let line = screen_context_line(
            started_at_ms,
            observation.0,
            &observation.1,
            &observation.2,
            &observation.3,
            &observation.4,
            &observation.5,
            &observation.6,
        );
        let mut changed = false;
        if !body.contains("Machine-described screen context:")
            && body.chars().count() + intro.chars().count() <= 20_000
        {
            body.push_str(intro);
            changed = true;
        }
        if body.chars().count() + line.chars().count() <= 20_000 {
            body.push_str(&line);
            changed = true;
        }
        if changed {
            let now = now_ms()?;
            let transaction = self.connection.transaction()?;
            transaction.execute(
                "UPDATE remembrie_contents SET text_content = ?1 WHERE id = ?2",
                params![body, content_id],
            )?;
            transaction.execute(
                "UPDATE remembries SET summary = NULL, updated_at_ms = ?1 WHERE id = ?2",
                params![now, remembrie_id],
            )?;
            transaction.execute(
                "UPDATE remembrie_fts SET body = ?1, summary = '' WHERE remembrie_id = ?2",
                params![body, remembrie_id],
            )?;
            transaction.execute(
                "DELETE FROM remembrie_chunks WHERE remembrie_id = ?1",
                [&remembrie_id],
            )?;
            transaction.execute(
                "DELETE FROM derived_artifacts
                 WHERE remembrie_id = ?1 AND kind = 'summary'",
                [&remembrie_id],
            )?;
            transaction.commit()?;
        }
        self.release_enrichment_after_screen_work(session_id)
    }

    fn release_enrichment_after_screen_work(
        &self,
        session_id: &str,
    ) -> Result<(), RepositoryError> {
        let pending = self.connection.query_row(
            "SELECT COUNT(*) FROM screen_observations
             WHERE session_id = ?1 AND state = 'processing'",
            [session_id],
            |row| row.get::<_, u64>(0),
        )?;
        if pending > 0 {
            return Ok(());
        }
        let remembrie_id = self
            .connection
            .query_row(
                "SELECT remembrie_id FROM activity_sessions WHERE id = ?1",
                [session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        if let Some(remembrie_id) = remembrie_id {
            self.connection.execute(
                "UPDATE processing_jobs
                 SET state = 'pending', attempts = 0, last_error = NULL,
                     available_at_ms = 0, updated_at_ms = ?1
                 WHERE id = ?2",
                params![now_ms()?, format!("enrich:{remembrie_id}")],
            )?;
        }
        Ok(())
    }

    pub fn list_capture_rules(&self) -> Result<Vec<CaptureRule>, RepositoryError> {
        let mut statement = self.connection.prepare(
            "SELECT id, rule_type, pattern, label, enabled, is_default, created_at_ms
             FROM capture_rules
             ORDER BY is_default DESC, COALESCE(label, pattern) COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(CaptureRule {
                id: row.get(0)?,
                rule_type: row.get(1)?,
                pattern: row.get(2)?,
                label: row.get(3)?,
                enabled: row.get(4)?,
                is_default: row.get(5)?,
                created_at_ms: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn add_capture_rule(
        &self,
        rule_type: &str,
        pattern: &str,
        label: Option<&str>,
    ) -> Result<CaptureRule, RepositoryError> {
        if !matches!(rule_type, "app" | "window_title" | "domain" | "content") {
            return Err(RepositoryError::Validation(
                "rule type must be app, window_title, domain, or content".to_owned(),
            ));
        }
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Err(RepositoryError::Validation(
                "an exclusion pattern cannot be empty".to_owned(),
            ));
        }
        if pattern.len() > 512 {
            return Err(RepositoryError::Validation(
                "an exclusion pattern cannot exceed 512 characters".to_owned(),
            ));
        }

        let id = Uuid::now_v7().to_string();
        let now = now_ms()?;
        self.connection.execute(
            "INSERT INTO capture_rules(
                id, rule_type, pattern, enabled, created_at_ms, label, is_default
             ) VALUES(?1, ?2, ?3, 1, ?4, ?5, 0)",
            params![id, rule_type, pattern, now, label],
        )?;
        self.capture_rule(&id)?.ok_or_else(|| {
            RepositoryError::Validation("created rule could not be loaded".to_owned())
        })
    }

    pub fn delete_capture_rule(&self, id: &str) -> Result<bool, RepositoryError> {
        let is_default = self
            .connection
            .query_row(
                "SELECT is_default FROM capture_rules WHERE id = ?1",
                [id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        match is_default {
            None => Ok(false),
            Some(true) => Err(RepositoryError::Validation(
                "built-in safety exclusions cannot be removed".to_owned(),
            )),
            Some(false) => Ok(self
                .connection
                .execute("DELETE FROM capture_rules WHERE id = ?1", [id])?
                > 0),
        }
    }

    pub fn delete_since(&mut self, timestamp_ms: i64) -> Result<u64, RepositoryError> {
        if timestamp_ms < 0 || timestamp_ms > now_ms()? {
            return Err(RepositoryError::Validation(
                "deletion timestamp is outside the valid range".to_owned(),
            ));
        }
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM remembrie_fts
             WHERE remembrie_id IN (
                SELECT id FROM remembries
                WHERE occurred_at_ms >= ?1
                   OR id IN (
                       SELECT remembrie_id FROM activity_sessions
                       WHERE last_active_at_ms >= ?1 AND remembrie_id IS NOT NULL
                   )
             )",
            [timestamp_ms],
        )?;
        let count = transaction.execute(
            "DELETE FROM remembries
             WHERE occurred_at_ms >= ?1
                OR id IN (
                    SELECT remembrie_id FROM activity_sessions
                    WHERE last_active_at_ms >= ?1 AND remembrie_id IS NOT NULL
                )",
            [timestamp_ms],
        )? as u64;
        transaction.execute(
            "DELETE FROM activity_sessions WHERE last_active_at_ms >= ?1",
            [timestamp_ms],
        )?;
        transaction.commit()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(count)
    }

    fn finish_active_activity_session(
        &mut self,
        requested_end_ms: i64,
        reason: &str,
    ) -> Result<Option<Remembrie>, RepositoryError> {
        let session = self
            .connection
            .query_row(
                "SELECT id, started_at_ms, last_active_at_ms FROM activity_sessions
                 WHERE ended_at_ms IS NULL",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((session_id, started_at_ms, last_active_at_ms)) = session else {
            return Ok(None);
        };

        let observations = {
            let mut statement = self.connection.prepare(
                "SELECT observed_at_ms, app_id, app_name, window_title
                 FROM activity_observations WHERE session_id = ?1
                 ORDER BY observed_at_ms, created_at_ms",
            )?;
            let rows = statement.query_map([&session_id], |row| {
                Ok(ActivityObservationRecord {
                    observed_at_ms: row.get(0)?,
                    app_id: row.get(1)?,
                    app_name: row.get(2)?,
                    window_title: row.get(3)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if observations.is_empty() {
            self.connection
                .execute("DELETE FROM activity_sessions WHERE id = ?1", [&session_id])?;
            return Ok(None);
        }

        let ended_at_ms = if matches!(
            reason,
            "desktop_bridge_unavailable" | "service_stopped" | "daemon_restart"
        ) {
            last_active_at_ms
        } else {
            requested_end_ms
        }
        .max(started_at_ms);
        let first_app = activity_app_label(&observations[0]);
        let last = observations
            .last()
            .expect("activity observations are not empty");
        let last_app = activity_app_label(last);
        let title = if first_app == last_app {
            format!("Activity session · {first_app}")
        } else {
            format!("Activity session · {first_app} → {last_app}")
        };
        let duration_ms = (ended_at_ms - started_at_ms).max(0);
        let duration_minutes = (duration_ms + 59_999) / 60_000;
        let duration = match (duration_ms, duration_minutes) {
            (0..60_000, _) => "less than 1 minute".to_owned(),
            (_, 1) => "1 minute".to_owned(),
            _ => format!("{duration_minutes} minutes"),
        };
        let mut body = format!(
            "Observed desktop activity over approximately {duration}.\n\
             Session ended: {}.\n\nApplications and windows:\n",
            activity_end_reason(reason)
        );
        let mut included = 0_usize;
        for observation in &observations {
            let offset_minutes = (observation.observed_at_ms - started_at_ms).max(0) / 60_000;
            let app = activity_app_label(observation);
            let identity = if observation.app_id.trim().is_empty()
                || observation.app_id.eq_ignore_ascii_case(&app)
            {
                String::new()
            } else {
                format!(" ({})", observation.app_id)
            };
            let window = if observation.window_title.trim().is_empty() {
                String::new()
            } else {
                format!(" — {}", observation.window_title)
            };
            let line = format!("- +{offset_minutes}m: {app}{identity}{window}\n");
            if body.chars().count() + line.chars().count() > 20_000 {
                break;
            }
            body.push_str(&line);
            included += 1;
        }
        if included < observations.len() {
            body.push_str(&format!(
                "- {} additional window changes remain in the local activity ledger.\n",
                observations.len() - included
            ));
        }

        let semantic_observations = {
            let mut statement = self.connection.prepare(
                "SELECT observed_at_ms, app_name, window_title, quality,
                        visible_nodes, text_nodes, document_nodes, text_content
                 FROM semantic_observations
                 WHERE session_id = ?1
                 ORDER BY observed_at_ms, created_at_ms",
            )?;
            let rows = statement.query_map([&session_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if !semantic_observations.is_empty() {
            body.push_str(semantic_context_intro());
            let mut included_semantic = 0_usize;
            for (
                observed_at_ms,
                app_name,
                window_title,
                quality,
                visible_nodes,
                text_nodes,
                document_nodes,
                text_content,
            ) in &semantic_observations
            {
                let line = semantic_context_line(
                    started_at_ms,
                    *observed_at_ms,
                    app_name,
                    window_title,
                    quality,
                    *visible_nodes,
                    *text_nodes,
                    *document_nodes,
                    text_content,
                );
                if body.chars().count() + line.chars().count() > 20_000 {
                    break;
                }
                body.push_str(&line);
                included_semantic += 1;
            }
            if included_semantic < semantic_observations.len() {
                body.push_str(&format!(
                    "- {} additional application-provided semantic observations remain in the local ledger.\n",
                    semantic_observations.len() - included_semantic
                ));
            }
        }

        let screen_observations = {
            let mut statement = self.connection.prepare(
                "SELECT observed_at_ms, app_name, window_title, description,
                        visible_text, confidence, model
                 FROM screen_observations
                 WHERE session_id = ?1 AND state = 'complete'
                 ORDER BY observed_at_ms, created_at_ms",
            )?;
            let rows = statement.query_map([&session_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if !screen_observations.is_empty() {
            body.push_str(screen_context_intro());
            let mut included_screen = 0_usize;
            for (
                observed_at_ms,
                app_name,
                window_title,
                description,
                visible_text,
                confidence,
                model,
            ) in &screen_observations
            {
                let line = screen_context_line(
                    started_at_ms,
                    *observed_at_ms,
                    app_name,
                    window_title,
                    description,
                    visible_text,
                    confidence,
                    model,
                );
                if body.chars().count() + line.chars().count() > 20_000 {
                    break;
                }
                body.push_str(&line);
                included_screen += 1;
            }
            if included_screen < screen_observations.len() {
                body.push_str(&format!(
                    "- {} additional machine-described screen observations remain in the local ledger.\n",
                    screen_observations.len() - included_screen
                ));
            }
        }

        let now = now_ms()?;
        let processing_screen_count = self.connection.query_row(
            "SELECT COUNT(*) FROM screen_observations
             WHERE session_id = ?1 AND state = 'processing'",
            [&session_id],
            |row| row.get::<_, u64>(0),
        )?;
        let enrichment_available_at_ms = if processing_screen_count > 0 {
            now + 16 * 60 * 1000
        } else {
            0
        };
        let remembrie_id = Uuid::now_v7().to_string();
        let content_id = Uuid::now_v7().to_string();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO remembries(
                id, kind, occurred_at_ms, ended_at_ms, source_app, window_title,
                title, sensitivity, importance, pinned, created_at_ms, updated_at_ms
             ) VALUES(?1, 'activity', ?2, ?3, 'Membrie Activity', ?4, ?5,
                      'normal', 0.4, 0, ?6, ?6)",
            params![
                remembrie_id,
                started_at_ms,
                ended_at_ms,
                last.window_title,
                title,
                now
            ],
        )?;
        transaction.execute(
            "INSERT INTO remembrie_contents(
                id, remembrie_id, role, mime_type, text_content, created_at_ms
             ) VALUES(?1, ?2, 'captured', 'text/plain', ?3, ?4)",
            params![content_id, remembrie_id, body, now],
        )?;
        transaction.execute(
            "INSERT INTO remembrie_fts(remembrie_id, title, body, summary)
             VALUES(?1, ?2, ?3, '')",
            params![remembrie_id, title, body],
        )?;
        transaction.execute(
            "INSERT INTO processing_jobs(
                id, remembrie_id, kind, state, attempts,
                available_at_ms, created_at_ms, updated_at_ms
             ) VALUES(?1, ?2, 'enrich', 'pending', 0, ?4, ?3, ?3)",
            params![
                format!("enrich:{remembrie_id}"),
                remembrie_id,
                now,
                enrichment_available_at_ms
            ],
        )?;
        transaction.execute(
            "UPDATE activity_sessions
             SET ended_at_ms = ?2, last_active_at_ms = MIN(last_active_at_ms, ?2),
                 end_reason = ?3, remembrie_id = ?4, updated_at_ms = ?5
             WHERE id = ?1",
            params![session_id, ended_at_ms, reason, remembrie_id, now],
        )?;
        transaction.execute(
            "INSERT INTO capture_events(
                id, occurred_at_ms, source_kind, outcome, reason, remembrie_id
             ) VALUES(?1, ?2, 'activity', 'stored', NULL, ?3)",
            params![Uuid::now_v7().to_string(), ended_at_ms, remembrie_id],
        )?;
        transaction.commit()?;
        self.get(&remembrie_id)
    }

    fn capture_rule(&self, id: &str) -> Result<Option<CaptureRule>, RepositoryError> {
        self.connection
            .query_row(
                "SELECT id, rule_type, pattern, label, enabled, is_default, created_at_ms
                 FROM capture_rules WHERE id = ?1",
                [id],
                |row| {
                    Ok(CaptureRule {
                        id: row.get(0)?,
                        rule_type: row.get(1)?,
                        pattern: row.get(2)?,
                        label: row.get(3)?,
                        enabled: row.get(4)?,
                        is_default: row.get(5)?,
                        created_at_ms: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn effective_pause(&self, now: i64) -> Result<(bool, Option<i64>), RepositoryError> {
        let (paused, paused_until_ms) = self.connection.query_row(
            "SELECT paused, paused_until_ms FROM capture_state WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, Option<i64>>(1)?)),
        )?;
        if paused && paused_until_ms.is_some_and(|until| until <= now) {
            self.connection.execute(
                "UPDATE capture_state
                 SET paused = 0, paused_until_ms = NULL, updated_at_ms = ?1
                 WHERE singleton = 1",
                [now],
            )?;
            return Ok((false, None));
        }
        Ok((paused, paused_until_ms))
    }

    fn clipboard_enabled(&self) -> Result<bool, RepositoryError> {
        self.connection
            .query_row(
                "SELECT clipboard_enabled FROM capture_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn is_recent_duplicate(
        &self,
        candidate: &CaptureCandidate,
        now: i64,
    ) -> Result<bool, RepositoryError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM remembries r
                    JOIN remembrie_contents c ON c.remembrie_id = r.id
                    WHERE r.kind = ?1
                      AND c.role = 'captured'
                      AND c.text_content = ?2
                      AND r.occurred_at_ms >= ?3
                      AND r.deleted_at_ms IS NULL
                )",
                params![candidate.kind, candidate.body, now - DUPLICATE_WINDOW_MS],
                |row| row.get::<_, bool>(0),
            )
            .map_err(Into::into)
    }

    fn skip_capture(
        &self,
        source_kind: &str,
        category: &str,
        reason: &str,
        now: i64,
    ) -> Result<CaptureDecision, RepositoryError> {
        self.record_capture_event(source_kind, category, Some(reason), None, now)?;
        Ok(CaptureDecision::Skipped {
            category: category.to_owned(),
            reason: reason.to_owned(),
        })
    }

    fn record_capture_event(
        &self,
        source_kind: &str,
        outcome: &str,
        reason: Option<&str>,
        remembrie_id: Option<&str>,
        occurred_at_ms: i64,
    ) -> Result<(), RepositoryError> {
        self.connection.execute(
            "INSERT INTO capture_events(
                id, occurred_at_ms, source_kind, outcome, reason, remembrie_id
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::now_v7().to_string(),
                occurred_at_ms,
                source_kind,
                outcome,
                reason,
                remembrie_id,
            ],
        )?;
        Ok(())
    }
}

fn validate_activity_text(
    label: &str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), RepositoryError> {
    if value.len() > maximum_bytes {
        return Err(RepositoryError::Validation(format!(
            "{label} cannot exceed {maximum_bytes} bytes"
        )));
    }
    Ok(())
}

fn validate_screen_candidate(candidate: &ScreenCaptureCandidate) -> Result<(), RepositoryError> {
    validate_activity_text("activity session identifier", &candidate.session_id, 128)?;
    validate_activity_text("screenshot path", &candidate.screenshot_path, 4096)?;
    validate_activity_text("application identifier", &candidate.app_id, 512)?;
    validate_activity_text("application name", &candidate.app_name, 512)?;
    validate_activity_text("window title", &candidate.window_title, 4096)?;
    if candidate.session_id.trim().is_empty() || candidate.screenshot_path.trim().is_empty() {
        return Err(RepositoryError::Validation(
            "screen capture needs a session and temporary image".to_owned(),
        ));
    }
    if !(1..=16_384).contains(&candidate.width) || !(1..=16_384).contains(&candidate.height) {
        return Err(RepositoryError::Validation(
            "screen dimensions are outside the supported range".to_owned(),
        ));
    }
    if candidate
        .observed_at_ms
        .is_some_and(|timestamp| timestamp < 0)
    {
        return Err(RepositoryError::Validation(
            "screen timestamps cannot be negative".to_owned(),
        ));
    }
    Ok(())
}

fn validate_semantic_candidate(
    candidate: &SemanticCaptureCandidate,
) -> Result<(), RepositoryError> {
    validate_activity_text("activity session identifier", &candidate.session_id, 128)?;
    validate_activity_text("application identifier", &candidate.app_id, 512)?;
    validate_activity_text("application name", &candidate.app_name, 512)?;
    validate_activity_text("window title", &candidate.window_title, 4096)?;
    validate_activity_text("semantic context", &candidate.text_content, 8_000)?;
    if candidate.session_id.trim().is_empty() {
        return Err(RepositoryError::Validation(
            "semantic context needs an activity session".to_owned(),
        ));
    }
    if !matches!(candidate.quality.as_str(), "partial" | "rich") {
        return Err(RepositoryError::Validation(
            "semantic quality must be partial or rich".to_owned(),
        ));
    }
    if candidate.text_content.trim().is_empty() {
        return Err(RepositoryError::Validation(
            "semantic context needs visible application-provided text".to_owned(),
        ));
    }
    if candidate.visible_nodes > 10_000
        || candidate.text_nodes > 10_000
        || candidate.document_nodes > 10_000
    {
        return Err(RepositoryError::Validation(
            "semantic node counts are outside the supported range".to_owned(),
        ));
    }
    if candidate
        .observed_at_ms
        .is_some_and(|timestamp| timestamp < 0)
    {
        return Err(RepositoryError::Validation(
            "semantic timestamps cannot be negative".to_owned(),
        ));
    }
    Ok(())
}

fn validate_screen_analysis(analysis: &ScreenAnalysis) -> Result<(), RepositoryError> {
    validate_activity_text("screen description", &analysis.description, 2_000)?;
    validate_activity_text("visible screen text", &analysis.visible_text, 8_000)?;
    validate_model_name(&analysis.model)?;
    if !matches!(analysis.confidence.as_str(), "low" | "medium" | "high") {
        return Err(RepositoryError::Validation(
            "screen-analysis confidence must be low, medium, or high".to_owned(),
        ));
    }
    Ok(())
}

fn validate_timeline_range(
    start_ms: i64,
    end_ms: i64,
    maximum_duration_ms: i64,
) -> Result<(), RepositoryError> {
    if start_ms < 0 || end_ms <= start_ms || end_ms - start_ms > maximum_duration_ms {
        return Err(RepositoryError::Validation(
            "timeline range is outside the supported bounds".to_owned(),
        ));
    }
    Ok(())
}

fn activity_source_app(app_id: &str, app_name: &str) -> String {
    let app_id = app_id.trim();
    let app_name = app_name.trim();
    match (app_name.is_empty(), app_id.is_empty()) {
        (false, false) if !app_name.eq_ignore_ascii_case(app_id) => {
            format!("{app_name} ({app_id})")
        }
        (false, _) => app_name.to_owned(),
        (_, false) => app_id.to_owned(),
        _ => "Desktop".to_owned(),
    }
}

fn normalize_screen_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_semantic_text(text: &str) -> String {
    text.lines()
        .map(normalize_screen_text)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn semantic_context_intro() -> &'static str {
    "\nApplication-provided semantic context:\n\
     These labels and visible text were supplied directly by application accessibility \
     interfaces. They are strong evidence of what was visible—not confirmation that an \
     action was completed.\n"
}

#[allow(clippy::too_many_arguments)]
fn semantic_context_line(
    started_at_ms: i64,
    observed_at_ms: i64,
    app_name: &str,
    window_title: &str,
    quality: &str,
    visible_nodes: u32,
    text_nodes: u32,
    document_nodes: u32,
    text_content: &str,
) -> String {
    let offset_minutes = (observed_at_ms - started_at_ms).max(0) / 60_000;
    let window = if window_title.trim().is_empty() {
        String::new()
    } else {
        format!(" — {}", window_title.trim())
    };
    format!(
        "- +{offset_minutes}m: {}{window} [{quality}; {visible_nodes} visible, {text_nodes} text, {document_nodes} document] {}\n",
        app_name.trim(),
        normalize_semantic_text(text_content),
    )
}

fn screen_context_intro() -> &'static str {
    "\nMachine-described screen context:\n\
     These notes were generated locally from temporary active-window screenshots. \
     The screenshots were immediately deleted. Descriptions can be incomplete or wrong, \
     so they are supporting context—not confirmation that an action occurred.\n"
}

#[allow(clippy::too_many_arguments)]
fn screen_context_line(
    started_at_ms: i64,
    observed_at_ms: i64,
    app_name: &str,
    window_title: &str,
    description: &str,
    visible_text: &str,
    confidence: &str,
    model: &str,
) -> String {
    let offset_minutes = (observed_at_ms - started_at_ms).max(0) / 60_000;
    let window = if window_title.trim().is_empty() {
        String::new()
    } else {
        format!(" — {}", window_title.trim())
    };
    let visible = if visible_text.trim().is_empty() {
        String::new()
    } else {
        format!(" Visible text: {}", normalize_screen_text(visible_text))
    };
    format!(
        "- +{offset_minutes}m: {}{window} [{confidence} confidence; model {model}] {}{visible}\n",
        app_name.trim(),
        normalize_screen_text(description),
    )
}

fn activity_result(
    outcome: &str,
    reason: Option<&str>,
    session_id: Option<&str>,
) -> ActivityRecordResult {
    ActivityRecordResult {
        outcome: outcome.to_owned(),
        reason: reason.map(str::to_owned),
        session_id: session_id.map(str::to_owned),
        screen_capture_allowed: false,
        semantic_capture_allowed: false,
    }
}

fn semantic_capture_result(
    outcome: &str,
    reason: Option<&str>,
    observation_id: Option<&str>,
) -> SemanticCaptureResult {
    SemanticCaptureResult {
        outcome: outcome.to_owned(),
        reason: reason.map(str::to_owned),
        observation_id: observation_id.map(str::to_owned),
    }
}

fn screen_capture_result(
    outcome: &str,
    reason: Option<&str>,
    observation_id: Option<&str>,
) -> ScreenCaptureResult {
    ScreenCaptureResult {
        outcome: outcome.to_owned(),
        reason: reason.map(str::to_owned),
        observation_id: observation_id.map(str::to_owned),
    }
}

fn activity_app_label(observation: &ActivityObservationRecord) -> String {
    if !observation.app_name.trim().is_empty() {
        observation.app_name.trim().to_owned()
    } else if !observation.app_id.trim().is_empty() {
        observation.app_id.trim().to_owned()
    } else {
        "Desktop".to_owned()
    }
}

fn activity_end_reason(reason: &str) -> &'static str {
    match reason {
        "idle" => "the computer became idle",
        "screen_locked" => "the screen was locked",
        "capture_paused" => "automatic capture was paused",
        "activity_disabled" => "activity capture was disabled",
        "excluded_context" => "an excluded application or window became active",
        "sensitive_context" => "a sensitive window context became active",
        "desktop_bridge_unavailable" => "the GNOME desktop bridge became unavailable",
        "service_stopped" => "the desktop capture service stopped",
        "daemon_restart" => "the Membrie daemon restarted",
        "manual" => "the session was finished manually",
        _ => "the activity boundary was reached",
    }
}

fn remembrie_select() -> &'static str {
    "SELECT
        r.id, r.kind, r.occurred_at_ms, r.ended_at_ms, r.source_app,
        r.window_title, r.title, COALESCE(c.text_content, ''), r.summary,
        r.sensitivity, r.importance, r.pinned, r.created_at_ms
     FROM remembries r
     LEFT JOIN remembrie_contents c
        ON c.remembrie_id = r.id AND c.role = 'captured'"
}

fn map_remembrie(row: &Row<'_>) -> rusqlite::Result<Remembrie> {
    Ok(Remembrie {
        id: row.get(0)?,
        kind: row.get(1)?,
        occurred_at_ms: row.get(2)?,
        ended_at_ms: row.get(3)?,
        source_app: row.get(4)?,
        window_title: row.get(5)?,
        title: row.get(6)?,
        body: row.get(7)?,
        summary: row.get(8)?,
        sensitivity: row.get(9)?,
        importance: row.get(10)?,
        pinned: row.get(11)?,
        created_at_ms: row.get(12)?,
    })
}

fn plain_fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter_map(|term| {
            let cleaned = term.trim_matches(|character: char| {
                !character.is_alphanumeric() && character != '_' && character != '-'
            });
            if cleaned.is_empty() {
                None
            } else {
                Some(format!("\"{}\"", cleaned.replace('"', "\"\"")))
            }
        })
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn validate_model_name(name: &str) -> Result<(), RepositoryError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(RepositoryError::Validation(
            "model names must contain between 1 and 128 characters".to_owned(),
        ));
    }
    if !name.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':' | '/')
    }) {
        return Err(RepositoryError::Validation(
            "model names contain an unsupported character".to_owned(),
        ));
    }
    if name.to_ascii_lowercase().contains("cloud") {
        return Err(RepositoryError::Validation(
            "cloud-backed model names are not allowed in Membrie".to_owned(),
        ));
    }
    Ok(())
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (60.0 + rank as f64 + 1.0)
}

fn encode_embedding(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_embedding(bytes: &[u8], dimensions: usize) -> Option<Vec<f32>> {
    if bytes.len() != dimensions.checked_mul(std::mem::size_of::<f32>())? {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (left, right) in left.iter().zip(right) {
        let left = f64::from(*left);
        let right = f64::from(*right);
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

fn now_ms() -> Result<i64, RepositoryError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RepositoryError::Clock)?
        .as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_database(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("membrie-test-{name}-{}.db", Uuid::now_v7()))
    }

    fn activity_snapshot(
        occurred_at_ms: i64,
        app_id: &str,
        app_name: &str,
        window_title: &str,
        idle_ms: u64,
    ) -> ActivitySnapshot {
        ActivitySnapshot {
            app_id: app_id.to_owned(),
            app_name: app_name.to_owned(),
            window_title: window_title.to_owned(),
            idle_ms,
            locked: false,
            occurred_at_ms: Some(occurred_at_ms),
        }
    }

    #[test]
    fn creates_and_lists_a_remembrie() {
        let path = temporary_database("create");
        let mut repository = Repository::open(&path).unwrap();
        let created = repository
            .create(NewRemembrie::manual(
                "A first Remembrie",
                "A private memory stored locally.",
            ))
            .unwrap();

        assert_eq!(created.kind, "note");
        assert_eq!(repository.list_recent(10).unwrap(), vec![created]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn full_text_search_finds_exact_language() {
        let path = temporary_database("search");
        let mut repository = Repository::open(&path).unwrap();
        repository
            .create(NewRemembrie::manual(
                "GTK debugging",
                "Found a SIGSEGV while measuring a widget.",
            ))
            .unwrap();
        repository
            .create(NewRemembrie::manual(
                "Garden",
                "Planted basil near the kitchen window.",
            ))
            .unwrap();

        let hits = repository.search("SIGSEGV widget", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].remembrie.title, "GTK debugging");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pause_state_is_persistent() {
        let path = temporary_database("pause");
        let mut repository = Repository::open(&path).unwrap();
        assert!(!repository.status().unwrap().paused);
        repository.set_pause(PauseMode::Indefinite).unwrap();
        assert!(repository.status().unwrap().paused);
        repository.set_pause(PauseMode::Resume).unwrap();
        assert!(!repository.status().unwrap().paused);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn upgrades_a_version_three_database_with_activity_off() {
        let path = temporary_database("migration-v3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection.execute_batch(MIGRATION_3).unwrap();
        connection.pragma_update(None, "user_version", 3).unwrap();
        drop(connection);

        let repository = Repository::open(&path).unwrap();
        let status = repository.status().unwrap();
        assert!(!status.activity_enabled);
        assert_eq!(status.activity_idle_threshold_ms, 15 * 60 * 1000);
        assert!(!status.semantic_enabled);
        assert_eq!(status.semantic_sample_interval_ms, 60 * 1000);
        assert!(!status.screen_enabled);
        assert_eq!(status.screen_sample_interval_ms, 2 * 60 * 1000);
        assert_eq!(status.screen_model, "gemma4:e2b");
        assert!(
            repository
                .list_capture_rules()
                .unwrap()
                .iter()
                .any(|rule| rule.label.as_deref() == Some("Membrie itself"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn upgrades_a_version_five_database_with_semantic_context_off() {
        let path = temporary_database("migration-v5");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection.execute_batch(MIGRATION_3).unwrap();
        connection.execute_batch(MIGRATION_4).unwrap();
        connection.execute_batch(MIGRATION_5).unwrap();
        connection.pragma_update(None, "user_version", 5).unwrap();
        drop(connection);

        let repository = Repository::open(&path).unwrap();
        let status = repository.status().unwrap();
        assert!(!status.semantic_enabled);
        assert_eq!(status.semantic_sample_interval_ms, 60_000);
        assert_eq!(status.semantic_observation_count, 0);
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn semantic_context_is_opt_in_deduplicated_and_materialized() {
        let path = temporary_database("semantic-context");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        repository.set_semantic_enabled(true).unwrap();
        let start = 1_700_000_000_000_i64;
        let activity = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "thunderbird_thunderbird.desktop",
                "Thunderbird",
                "Inbox",
                0,
            ))
            .unwrap();
        assert!(activity.semantic_capture_allowed);
        let session_id = activity.session_id.unwrap();
        let candidate = SemanticCaptureCandidate {
            session_id,
            app_id: "thunderbird_thunderbird.desktop".to_owned(),
            app_name: "Thunderbird".to_owned(),
            window_title: "Inbox".to_owned(),
            observed_at_ms: Some(start),
            quality: "rich".to_owned(),
            visible_nodes: 42,
            text_nodes: 12,
            document_nodes: 1,
            text_content: "heading: Inbox\nlist item: Message from Duke Jones".to_owned(),
        };
        let stored = repository.record_semantic_observation(&candidate).unwrap();
        assert_eq!(stored.outcome, "stored");
        let duplicate = repository.record_semantic_observation(&candidate).unwrap();
        assert_eq!(duplicate.outcome, "skipped");
        repository.end_activity_session("manual").unwrap();

        let remembries = repository.list_recent(10).unwrap();
        assert_eq!(remembries.len(), 1);
        assert!(
            remembries[0]
                .body
                .contains("Application-provided semantic context")
        );
        assert!(remembries[0].body.contains("Message from Duke Jones"));
        assert!(remembries[0].body.contains("not confirmation"));
        assert_eq!(repository.search("Duke Jones", 10).unwrap().len(), 1);
        assert_eq!(repository.status().unwrap().semantic_observation_count, 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn semantic_context_rejects_sensitive_text_without_storing_it() {
        let path = temporary_database("semantic-sensitive");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        repository.set_semantic_enabled(true).unwrap();
        let start = 1_700_000_000_000_i64;
        let activity = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "org.gnome.TextEditor",
                "Text Editor",
                "Private notes",
                0,
            ))
            .unwrap();
        let candidate = SemanticCaptureCandidate {
            session_id: activity.session_id.unwrap(),
            app_id: "org.gnome.TextEditor".to_owned(),
            app_name: "Text Editor".to_owned(),
            window_title: "Private notes".to_owned(),
            observed_at_ms: Some(start),
            quality: "rich".to_owned(),
            visible_nodes: 12,
            text_nodes: 4,
            document_nodes: 1,
            text_content: "text: password=definitely-secret".to_owned(),
        };

        let result = repository.record_semantic_observation(&candidate).unwrap();
        assert_eq!(result.outcome, "skipped");
        assert!(result.reason.unwrap().contains("sensitive"));
        let stored: u64 = repository
            .connection
            .query_row("SELECT COUNT(*) FROM semantic_observations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, 0);
        assert_eq!(repository.status().unwrap().skipped_sensitive, 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn semantic_context_honors_content_exclusions() {
        let path = temporary_database("semantic-content-exclusion");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        repository.set_semantic_enabled(true).unwrap();
        repository
            .add_capture_rule("content", "*Duke Jones*", Some("Duke research"))
            .unwrap();
        let start = 1_700_000_000_000_i64;
        let activity = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "thunderbird_thunderbird.desktop",
                "Thunderbird",
                "Inbox",
                0,
            ))
            .unwrap();
        let candidate = SemanticCaptureCandidate {
            session_id: activity.session_id.unwrap(),
            app_id: "thunderbird_thunderbird.desktop".to_owned(),
            app_name: "Thunderbird".to_owned(),
            window_title: "Inbox".to_owned(),
            observed_at_ms: Some(start),
            quality: "rich".to_owned(),
            visible_nodes: 30,
            text_nodes: 10,
            document_nodes: 1,
            text_content: "list item: Message from Duke Jones".to_owned(),
        };

        let result = repository.record_semantic_observation(&candidate).unwrap();
        assert_eq!(result.outcome, "skipped");
        assert!(result.reason.unwrap().contains("Duke research"));
        assert_eq!(repository.status().unwrap().semantic_observation_count, 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn disabling_activity_also_disables_semantic_context() {
        let path = temporary_database("semantic-activity-dependency");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        assert!(
            repository
                .set_semantic_enabled(true)
                .unwrap()
                .semantic_enabled
        );

        let status = repository.set_activity_enabled(false).unwrap();
        assert!(!status.activity_enabled);
        assert!(!status.semantic_enabled);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn activity_context_closes_into_a_searchable_remembrie() {
        let path = temporary_database("activity-session");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        let start = 1_700_000_000_000_i64;

        let first = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "org.gnome.TextEditor",
                "Text Editor",
                "Membrie architecture notes",
                0,
            ))
            .unwrap();
        assert_eq!(first.outcome, "recorded");
        repository
            .record_activity_snapshot(activity_snapshot(
                start + 60_000,
                "google-chrome.desktop",
                "Google Chrome",
                "Compose email to Duke Jones",
                0,
            ))
            .unwrap();
        let boundary = repository
            .record_activity_snapshot(activity_snapshot(
                start + 16 * 60_000,
                "google-chrome.desktop",
                "Google Chrome",
                "Compose email to Duke Jones",
                15 * 60_000,
            ))
            .unwrap();
        assert_eq!(boundary.outcome, "boundary");

        let remembries = repository.list_recent(10).unwrap();
        assert_eq!(remembries.len(), 1);
        assert_eq!(remembries[0].kind, "activity");
        assert!(remembries[0].title.contains("Text Editor"));
        assert!(remembries[0].title.contains("Google Chrome"));
        assert!(remembries[0].body.contains("Duke Jones"));
        assert_eq!(remembries[0].ended_at_ms, Some(start + 60_000));
        assert_eq!(repository.search("Duke Jones", 10).unwrap().len(), 1);
        let history = repository
            .timeline_history(start - 1, start + 60 * 60_000)
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].kind, "activity");
        assert_eq!(history[0].started_at_ms, start);
        assert_eq!(history[0].ended_at_ms, start + 60_000);
        let timeline = repository
            .timeline_day(start - 1, start + 60 * 60_000)
            .unwrap();
        assert_eq!(timeline.len(), 1);
        let activity = timeline[0].activity.as_ref().unwrap();
        assert_eq!(activity.end_reason, "idle");
        assert_eq!(activity.observation_count, 2);
        assert_eq!(activity.observations[0].app_name, "Text Editor");
        assert_eq!(activity.observations[1].app_name, "Google Chrome");
        let status = repository.status().unwrap();
        assert_eq!(status.activity_session_count, 1);
        assert!(status.activity_active_since_ms.is_none());

        assert_eq!(repository.delete_since(start + 30_000).unwrap(), 1);
        assert!(repository.list_recent(10).unwrap().is_empty());
        assert_eq!(repository.status().unwrap().activity_session_count, 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn activity_context_never_stores_excluded_windows() {
        let path = temporary_database("activity-exclusion");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();

        let result = repository
            .record_activity_snapshot(activity_snapshot(
                1_700_000_000_000,
                "com.chuk.Membrie",
                "Membrie",
                "Private local memories",
                0,
            ))
            .unwrap();
        assert_eq!(result.outcome, "skipped");
        let observations = repository
            .connection
            .query_row("SELECT COUNT(*) FROM activity_observations", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        assert_eq!(observations, 0);
        assert!(repository.list_recent(10).unwrap().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn interrupted_activity_ends_at_last_observed_input() {
        let path = temporary_database("activity-restart");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        let start = 1_700_000_000_000_i64;
        repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "org.gnome.Terminal",
                "Terminal",
                "Membrie build",
                0,
            ))
            .unwrap();
        repository
            .record_activity_snapshot(activity_snapshot(
                start + 60_000,
                "org.gnome.Terminal",
                "Terminal",
                "Membrie tests",
                0,
            ))
            .unwrap();

        repository.reset_interrupted_activity().unwrap();
        let remembries = repository.list_recent(10).unwrap();
        assert_eq!(remembries.len(), 1);
        assert_eq!(remembries[0].ended_at_ms, Some(start + 60_000));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn screen_memory_is_opt_in_and_materializes_labeled_context() {
        let path = temporary_database("screen-memory");
        let mut repository = Repository::open(&path).unwrap();
        assert!(repository.set_screen_enabled(true).is_err());
        repository.set_activity_enabled(true).unwrap();
        repository.set_screen_enabled(true).unwrap();
        let start = now_ms().unwrap() - 60_000;
        let activity = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "google-chrome.desktop",
                "Google Chrome",
                "Compose email to Duke Jones",
                0,
            ))
            .unwrap();
        assert!(activity.screen_capture_allowed);
        let candidate = ScreenCaptureCandidate {
            session_id: activity.session_id.unwrap(),
            screenshot_path: "/run/user/1000/membrie-screen/example.png".to_owned(),
            app_id: "google-chrome.desktop".to_owned(),
            app_name: "Google Chrome".to_owned(),
            window_title: "Compose email to Duke Jones".to_owned(),
            observed_at_ms: Some(start + 1_000),
            width: 1200,
            height: 800,
        };
        let (observation_id, model) = repository.begin_screen_analysis(&candidate).unwrap();
        assert_eq!(model, "gemma4:e2b");
        assert_eq!(repository.status().unwrap().screen_processing_count, 1);
        let stored = repository
            .complete_screen_analysis(
                &observation_id,
                &ScreenAnalysis {
                    description: "An email compose window is open.".to_owned(),
                    visible_text: "To: Duke Jones; Subject: Meeting notes".to_owned(),
                    confidence: "high".to_owned(),
                    model: "gemma4:e2b".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(stored.outcome, "stored");
        repository.end_activity_session("manual").unwrap();

        let remembries = repository.list_recent(10).unwrap();
        assert_eq!(remembries.len(), 1);
        assert!(
            remembries[0]
                .body
                .contains("Machine-described screen context")
        );
        assert!(
            remembries[0]
                .body
                .contains("supporting context—not confirmation")
        );
        assert!(remembries[0].body.contains("Duke Jones"));
        let status = repository.status().unwrap();
        assert_eq!(status.screen_observation_count, 1);
        assert_eq!(status.screen_processing_count, 0);
        assert_eq!(status.screen_last_app.as_deref(), Some("Google Chrome"));
        assert_eq!(status.screen_last_observed_at_ms, Some(start + 1_000));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn screen_analysis_finishing_after_session_close_is_added_before_enrichment() {
        let path = temporary_database("screen-late-finish");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        repository.set_screen_enabled(true).unwrap();
        let start = now_ms().unwrap() - 60_000;
        let activity = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "google-chrome.desktop",
                "Google Chrome",
                "Research notes",
                0,
            ))
            .unwrap();
        let candidate = ScreenCaptureCandidate {
            session_id: activity.session_id.unwrap(),
            screenshot_path: "/run/user/1000/membrie-screen/example.png".to_owned(),
            app_id: "google-chrome.desktop".to_owned(),
            app_name: "Google Chrome".to_owned(),
            window_title: "Research notes".to_owned(),
            observed_at_ms: Some(start + 1_000),
            width: 1200,
            height: 800,
        };
        let (observation_id, _) = repository.begin_screen_analysis(&candidate).unwrap();
        repository.end_activity_session("manual").unwrap();
        assert!(repository.claim_processing_job().unwrap().is_none());

        let result = repository
            .complete_screen_analysis(
                &observation_id,
                &ScreenAnalysis {
                    description: "A browser shows advocacy research notes.".to_owned(),
                    visible_text: "Duke Jones follow-up details".to_owned(),
                    confidence: "high".to_owned(),
                    model: "gemma4:e2b".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(result.outcome, "stored");

        let remembries = repository.list_recent(10).unwrap();
        assert_eq!(remembries.len(), 1);
        assert!(remembries[0].body.contains("Duke Jones follow-up details"));
        let job = repository.claim_processing_job().unwrap().unwrap();
        assert_eq!(job.remembrie.id, remembries[0].id);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn sensitive_screen_analysis_is_not_stored() {
        let path = temporary_database("screen-sensitive");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_activity_enabled(true).unwrap();
        repository.set_screen_enabled(true).unwrap();
        let start = now_ms().unwrap() - 1_000;
        let activity = repository
            .record_activity_snapshot(activity_snapshot(
                start,
                "org.gnome.TextEditor",
                "Text Editor",
                "Ordinary notes",
                0,
            ))
            .unwrap();
        let candidate = ScreenCaptureCandidate {
            session_id: activity.session_id.unwrap(),
            screenshot_path: "/run/user/1000/membrie-screen/example.png".to_owned(),
            app_id: "org.gnome.TextEditor".to_owned(),
            app_name: "Text Editor".to_owned(),
            window_title: "Ordinary notes".to_owned(),
            observed_at_ms: Some(start),
            width: 800,
            height: 600,
        };
        let (observation_id, _) = repository.begin_screen_analysis(&candidate).unwrap();
        let result = repository
            .complete_screen_analysis(
                &observation_id,
                &ScreenAnalysis {
                    description: "A note contains an AWS access key.".to_owned(),
                    visible_text: "AKIAIOSFODNN7EXAMPLE".to_owned(),
                    confidence: "high".to_owned(),
                    model: "gemma4:e2b".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(result.outcome, "skipped");
        let status = repository.status().unwrap();
        assert_eq!(status.screen_observation_count, 0);
        assert_eq!(status.screen_processing_count, 0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn upgrades_a_version_one_database_without_losing_state() {
        let path = temporary_database("migration-v1");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);

        let repository = Repository::open(&path).unwrap();
        let status = repository.status().unwrap();
        assert!(!status.clipboard_enabled);
        assert!(status.clipboard_agent_last_seen_ms.is_none());
        assert!(
            repository
                .list_capture_rules()
                .unwrap()
                .iter()
                .any(|rule| rule.label.as_deref() == Some("Bitwarden"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn upgrades_a_version_two_database_and_backfills_processing() {
        let path = temporary_database("migration-v2");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection.execute_batch(MIGRATION_2).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        let now = now_ms().unwrap();
        connection
            .execute(
                "INSERT INTO remembries(
                    id, kind, occurred_at_ms, title, sensitivity, importance,
                    pinned, created_at_ms, updated_at_ms
                 ) VALUES('existing', 'note', ?1, 'Existing', 'normal', 0.5, 0, ?1, ?1)",
                [now],
            )
            .unwrap();
        drop(connection);

        let repository = Repository::open(&path).unwrap();
        let settings = repository.intelligence_settings().unwrap();
        assert_eq!(settings.chat_model, "gemma4:12b");
        assert_eq!(settings.embedding_model, "embeddinggemma:latest");
        let status = repository.intelligence_status(false, Vec::new()).unwrap();
        assert_eq!(status.total_remembries, 1);
        assert_eq!(status.pending_jobs, 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn creates_an_integrity_checked_backup() {
        let path = temporary_database("backup-source");
        let backup_path = temporary_database("backup-destination");
        let mut repository = Repository::open(&path).unwrap();
        repository
            .create(NewRemembrie::manual(
                "Worth keeping",
                "A verified database snapshot should contain this.",
            ))
            .unwrap();

        repository.backup_to(&backup_path).unwrap();
        let backup = Repository::open(&backup_path).unwrap();
        let remembries = backup.list_recent(10).unwrap();
        assert_eq!(remembries.len(), 1);
        assert_eq!(remembries[0].title, "Worth keeping");

        let _ = fs::remove_file(path);
        let _ = fs::remove_file(backup_path);
    }

    #[test]
    fn enrichment_is_restart_safe_and_supports_hybrid_search() {
        let path = temporary_database("enrichment");
        let mut repository = Repository::open(&path).unwrap();
        repository
            .create(NewRemembrie::manual(
                "Blue notebook",
                "The research notes about lighthouse optics are in the blue notebook.",
            ))
            .unwrap();
        let job = repository.claim_processing_job().unwrap().unwrap();
        assert_eq!(job.attempts, 1);
        let chunks = vec![EmbeddedChunk {
            ordinal: 0,
            start_offset: 0,
            end_offset: job.remembrie.body.len(),
            text: job.remembrie.body.clone(),
            embedding: vec![1.0, 0.0, 0.0],
        }];
        assert!(
            repository
                .complete_enrichment(
                    &job.id,
                    "Research notes about lighthouse optics are in the blue notebook.",
                    "gemma4:12b",
                    "embeddinggemma:latest",
                    &chunks,
                )
                .unwrap()
        );

        let status = repository.intelligence_status(true, Vec::new()).unwrap();
        assert_eq!(status.indexed_remembries, 1);
        assert_eq!(status.pending_jobs, 0);
        let hits = repository
            .hybrid_search(
                "where are the optics notes",
                Some(&[1.0, 0.0, 0.0]),
                "embeddinggemma:latest",
                10,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].remembrie.title, "Blue notebook");
        assert!(hits[0].semantic_score.is_some());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn automatic_capture_skips_secrets_and_duplicates() {
        let path = temporary_database("capture-policy");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_clipboard_enabled(true).unwrap();

        let sensitive = repository
            .capture(CaptureCandidate::clipboard(
                "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret",
            ))
            .unwrap();
        assert!(matches!(
            sensitive,
            CaptureDecision::Skipped { category, .. } if category == "sensitive"
        ));
        assert!(repository.list_recent(10).unwrap().is_empty());

        let first = repository
            .capture(CaptureCandidate::clipboard("A useful clipboard value"))
            .unwrap();
        assert!(matches!(first, CaptureDecision::Stored { .. }));
        let duplicate = repository
            .capture(CaptureCandidate::clipboard("A useful clipboard value"))
            .unwrap();
        assert!(matches!(
            duplicate,
            CaptureDecision::Skipped { category, .. } if category == "duplicate"
        ));

        let status = repository.status().unwrap();
        assert_eq!(status.remembrie_count, 1);
        assert_eq!(status.skipped_sensitive, 1);
        assert_eq!(status.skipped_duplicate, 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn custom_exclusions_and_time_deletion_work() {
        let path = temporary_database("rules-delete");
        let mut repository = Repository::open(&path).unwrap();
        repository.set_clipboard_enabled(true).unwrap();
        let rule = repository
            .add_capture_rule("app", "*secret-app*", Some("Secret app"))
            .unwrap();

        let mut candidate = CaptureCandidate::clipboard("ordinary text");
        candidate.source_app = Some("my-secret-app".to_owned());
        let decision = repository.capture(candidate).unwrap();
        assert!(matches!(
            decision,
            CaptureDecision::Skipped { category, .. } if category == "excluded"
        ));
        assert!(repository.delete_capture_rule(&rule.id).unwrap());

        repository
            .create(NewRemembrie::manual("Temporary", "Delete me"))
            .unwrap();
        assert_eq!(repository.delete_since(0).unwrap(), 1);
        assert!(repository.list_recent(10).unwrap().is_empty());
        let _ = fs::remove_file(path);
    }
}
