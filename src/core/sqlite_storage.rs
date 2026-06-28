// sqlite_storage.rs — Optional SQLite-backed session storage.
//
// Provides `SqliteSessionStore` as a faster, queryable alternative to
// the default JSONL storage while keeping the existing public type names.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use turso::{Builder, Database, Row, Value};

/// A persistent SQLite session + message store.
pub struct SqliteSessionStore {
    db: Database,
}

impl SqliteSessionStore {
    fn connect(&self) -> Result<turso::Connection> {
        self.db
            .connect()
            .map_err(|err| anyhow!("session DB connection error: {err}"))
    }

    fn conversion_error(field: &str, value: &Value) -> anyhow::Error {
        anyhow!("unexpected value type for {field}: {value:?}")
    }

    fn row_text(row: &Row, idx: usize, field: &str) -> Result<String> {
        match row.get_value(idx)? {
            Value::Text(value) => Ok(value),
            value => Err(Self::conversion_error(field, &value)),
        }
    }

    fn row_optional_text(row: &Row, idx: usize, field: &str) -> Result<Option<String>> {
        match row.get_value(idx)? {
            Value::Null => Ok(None),
            Value::Text(value) => Ok(Some(value)),
            value => Err(Self::conversion_error(field, &value)),
        }
    }

    fn row_u32(row: &Row, idx: usize, field: &str) -> Result<u32> {
        match row.get_value(idx)? {
            Value::Null => Ok(0),
            Value::Integer(value) => u32::try_from(value)
                .with_context(|| format!("{field} value {value} is outside the u32 range")),
            value => Err(Self::conversion_error(field, &value)),
        }
    }

    fn session_summary_from_row(row: &Row) -> Result<SessionSummary> {
        Ok(SessionSummary {
            id: Self::row_text(row, 0, "id")?,
            title: Self::row_optional_text(row, 1, "title")?,
            model: Self::row_optional_text(row, 2, "model")?.unwrap_or_default(),
            created_at: Self::row_text(row, 3, "created_at")?,
            updated_at: Self::row_text(row, 4, "updated_at")?,
            message_count: Self::row_u32(row, 5, "message_count")?,
        })
    }

    fn stored_message_from_row(row: &Row) -> Result<StoredMessage> {
        Ok(StoredMessage {
            id: Self::row_text(row, 0, "id")?,
            role: Self::row_text(row, 1, "role")?,
            content: Self::row_text(row, 2, "content")?,
            created_at: Self::row_text(row, 3, "created_at")?,
        })
    }

    /// Open (or create) the database at `db_path` and ensure the schema exists.
    pub async fn open(db_path: &Path) -> Result<Self> {
        let db_path = db_path.to_string_lossy().to_string();
        let db = Builder::new_local(&db_path).build().await?;
        let conn = db.connect()?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id            TEXT PRIMARY KEY,
                title         TEXT,
                model         TEXT NOT NULL DEFAULT '',
                created_at    TEXT NOT NULL,
                updated_at    TEXT NOT NULL,
                message_count INTEGER NOT NULL DEFAULT 0
            )",
            (),
        )
        .await?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id          TEXT PRIMARY KEY,
                session_id  TEXT NOT NULL REFERENCES sessions(id),
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                cost_usd    REAL
            )",
            (),
        )
        .await?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id)",
            (),
        )
        .await?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at)",
            (),
        )
        .await?;

        Ok(Self { db })
    }

    /// Insert or replace a session record. `created_at` is preserved on
    /// UPDATE so only `updated_at` changes.
    pub async fn save_session(
        &self,
        session_id: &str,
        title: Option<&str>,
        model: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.connect()?;

        conn.execute(
            "INSERT INTO sessions (id, title, model, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 title      = excluded.title,
                 model      = excluded.model,
                 updated_at = excluded.updated_at",
            turso::params![session_id, title, model, now.as_str()],
        )
        .await?;

        Ok(())
    }

    /// Append a message to the given session (idempotent on `msg_id`).
    /// Also bumps `sessions.message_count` and `sessions.updated_at`.
    pub async fn save_message(
        &self,
        session_id: &str,
        msg_id: &str,
        role: &str,
        content: &str,
        cost_usd: Option<f64>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.connect()?;
        let tx = conn.transaction().await?;

        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO messages
                 (id, session_id, role, content, created_at, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                turso::params![msg_id, session_id, role, content, now.as_str(), cost_usd],
            )
            .await?;

        if inserted > 0 {
            tx.execute(
                "UPDATE sessions
                 SET updated_at    = ?1,
                     message_count = message_count + 1
                 WHERE id = ?2",
                turso::params![now.as_str(), session_id],
            )
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Return the 100 most recently updated sessions.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT id, title, model, created_at, updated_at, message_count
                 FROM sessions
                 ORDER BY updated_at DESC
                 LIMIT 100",
                (),
            )
            .await?;

        let mut sessions = Vec::new();
        while let Some(row) = rows.next().await? {
            sessions.push(Self::session_summary_from_row(&row)?);
        }

        Ok(sessions)
    }

    /// Full-text search across session titles and message content.
    /// Returns up to 50 matching sessions ordered by recency.
    pub async fn search_sessions(&self, query: &str) -> Result<Vec<SessionSummary>> {
        let like = format!("%{query}%");
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT s.id, s.title, s.model,
                        s.created_at, s.updated_at, s.message_count
                 FROM sessions s
                 LEFT JOIN messages m ON m.session_id = s.id
                 WHERE s.title LIKE ?1
                    OR m.content LIKE ?1
                 ORDER BY s.updated_at DESC
                 LIMIT 50",
                turso::params![like.as_str()],
            )
            .await?;

        let mut sessions = Vec::new();
        while let Some(row) = rows.next().await? {
            sessions.push(Self::session_summary_from_row(&row)?);
        }

        Ok(sessions)
    }

    /// Delete a session and all of its messages.
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let conn = self.connect()?;

        conn.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            turso::params![session_id],
        )
        .await?;

        conn.execute("DELETE FROM sessions WHERE id = ?1", turso::params![session_id])
            .await?;

        Ok(())
    }

    /// Return every message for the given session, oldest first.
    /// `content` is the persisted body string (whatever the caller stored
    /// in `save_message`); the UI flattens it back into a single Text
    /// block for display.
    pub async fn list_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT id, role, content, created_at
                 FROM messages
                 WHERE session_id = ?1
                 ORDER BY created_at ASC, rowid ASC",
                turso::params![session_id],
            )
            .await?;

        let mut messages = Vec::new();
        while let Some(row) = rows.next().await? {
            messages.push(Self::stored_message_from_row(&row)?);
        }

        Ok(messages)
    }
}

/// A persisted message. `created_at` is the same RFC3339 string we
/// stored; the UI parses it back into a Unix-ms integer for `UiMessage`.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Summary row returned by `list_sessions` and `search_sessions`.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: u32,
}
