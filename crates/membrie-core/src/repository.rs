use crate::model::{
    CaptureCandidate, CaptureDecision, CaptureRule, CaptureStatus, EmbeddedChunk,
    IntelligenceSettings, IntelligenceStatus, LocalModel, NewRemembrie, PauseMode, ProcessingJob,
    Remembrie, SearchHit,
};
use crate::policy::{MAX_AUTOMATIC_CONTENT_BYTES, exclusion_reason, sensitive_reason};
use rusqlite::{Connection, DatabaseName, OptionalExtension, Row, params};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 3;
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
            [now_ms()?],
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
        let clipboard_enabled = self.clipboard_enabled()?;
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
        Ok(CaptureStatus {
            paused,
            paused_until_ms,
            clipboard_enabled,
            remembrie_count: count,
            skipped_total,
            skipped_sensitive,
            skipped_duplicate,
            clipboard_agent_last_seen_ms: None,
            database_path: self.path.display().to_string(),
        })
    }

    pub fn set_pause(&self, mode: PauseMode) -> Result<CaptureStatus, RepositoryError> {
        let (paused, paused_until_ms) = match mode {
            PauseMode::Resume => (false, None),
            PauseMode::Until { timestamp_ms } => {
                if timestamp_ms <= now_ms()? {
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
            params![paused, paused_until_ms, now_ms()?],
        )?;
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
                SELECT id FROM remembries WHERE occurred_at_ms >= ?1
             )",
            [timestamp_ms],
        )?;
        let count = transaction.execute(
            "DELETE FROM remembries WHERE occurred_at_ms >= ?1",
            [timestamp_ms],
        )? as u64;
        transaction.commit()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(count)
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
        let repository = Repository::open(&path).unwrap();
        assert!(!repository.status().unwrap().paused);
        repository.set_pause(PauseMode::Indefinite).unwrap();
        assert!(repository.status().unwrap().paused);
        repository.set_pause(PauseMode::Resume).unwrap();
        assert!(!repository.status().unwrap().paused);
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
