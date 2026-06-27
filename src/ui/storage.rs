// ui::storage — SQLite persistence for ui_message / ui_block / ui_attachment_ref.
// Reuses the same database file as core::SessionStorage; tables are namespaced with `ui_`.

use std::path::{Path, PathBuf};

use crate::ui::model::*;

pub struct MessageStore {
    conn: rusqlite::Connection,
}

impl MessageStore {
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ui_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                ordinal INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_ui_message_session
                ON ui_message(session_id, ordinal);

            CREATE TABLE IF NOT EXISTS ui_block (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_ui_block_message
                ON ui_block(message_id, ordinal);

            CREATE TABLE IF NOT EXISTS ui_attachment_ref (
                attachment_id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                local_path TEXT NOT NULL
            );
            ",
        )?;
        Ok(Self { conn })
    }

    pub fn insert_message(&self, m: &UiMessage) -> anyhow::Result<()> {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::ToolResult => "tool_result",
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO ui_message (id, session_id, role, created_at, ordinal)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![m.id, m.session_id, role, m.created_at, m.ordinal],
        )?;
        Ok(())
    }

    pub fn list_messages(&self, session_id: &str) -> anyhow::Result<Vec<UiMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, created_at, ordinal FROM ui_message
             WHERE session_id = ?1 ORDER BY ordinal ASC",
        )?;
        let rows = stmt.query_map([session_id], |r| {
            let role: String = r.get(2)?;
            Ok(UiMessage {
                id: r.get(0)?,
                session_id: r.get(1)?,
                role: match role.as_str() {
                    "assistant" => Role::Assistant,
                    "tool_result" => Role::ToolResult,
                    _ => Role::User,
                },
                created_at: r.get(3)?,
                ordinal: r.get(4)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_block(&self, b: &UiBlock) -> anyhow::Result<()> {
        let (kind, payload) = encode_block(&b.kind)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO ui_block (id, message_id, ordinal, kind, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![b.id, b.message_id, b.ordinal, kind, payload],
        )?;
        Ok(())
    }

    pub fn list_blocks(&self, message_id: &str) -> anyhow::Result<Vec<UiBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, message_id, ordinal, kind, payload FROM ui_block
             WHERE message_id = ?1 ORDER BY ordinal ASC",
        )?;
        let rows = stmt.query_map([message_id], |r| {
            let id: String = r.get(0)?;
            let message_id: String = r.get(1)?;
            let ordinal: i32 = r.get(2)?;
            let kind: String = r.get(3)?;
            let payload: Vec<u8> = r.get(4)?;
            Ok((id, message_id, ordinal, kind, payload))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, message_id, ordinal, kind, payload) = row?;
            let kind = decode_block(&kind, &payload)?;
            out.push(UiBlock { id, message_id, ordinal, kind });
        }
        Ok(out)
    }

    /// Delete blocks/messages for a session. Used by "Clear session" action.
    pub fn clear_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM ui_block WHERE message_id IN
            (SELECT id FROM ui_message WHERE session_id = ?1)", [session_id])?;
        self.conn.execute("DELETE FROM ui_message WHERE session_id = ?1", [session_id])?;
        Ok(())
    }
}

fn encode_block(k: &BlockKind) -> anyhow::Result<(&'static str, Vec<u8>)> {
    let (tag, json) = match k {
        BlockKind::Text { text } => ("text", serde_json::json!({ "text": text })),
        BlockKind::Thinking { thinking, signature } => (
            "thinking",
            serde_json::json!({ "thinking": thinking, "signature": signature }),
        ),
        BlockKind::ToolUse { id, name, input } => (
            "tool_use",
            serde_json::json!({ "id": id, "name": name, "input": input }),
        ),
        BlockKind::ToolResult { tool_use_id, content, is_error } => (
            "tool_result",
            serde_json::json!({ "tool_use_id": tool_use_id, "content": content, "is_error": is_error }),
        ),
        BlockKind::Attachments { items } => {
            let items_json: Vec<serde_json::Value> = items.iter().map(attachment_to_json).collect();
            ("attachments", serde_json::json!({ "items": items_json }))
        }
    };
    let bytes = serde_json::to_vec(&json)?;
    Ok((tag, bytes))
}

fn decode_block(tag: &str, payload: &[u8]) -> anyhow::Result<BlockKind> {
    let json: serde_json::Value = serde_json::from_slice(payload)?;
    let k = match tag {
        "text" => BlockKind::Text {
            text: json["text"].as_str().unwrap_or("").to_string(),
        },
        "thinking" => BlockKind::Thinking {
            thinking: json["thinking"].as_str().unwrap_or("").to_string(),
            signature: json["signature"].as_str().map(|s| s.to_string()),
        },
        "tool_use" => BlockKind::ToolUse {
            id: json["id"].as_str().unwrap_or("").to_string(),
            name: json["name"].as_str().unwrap_or("").to_string(),
            input: json["input"].clone(),
        },
        "tool_result" => BlockKind::ToolResult {
            tool_use_id: json["tool_use_id"].as_str().unwrap_or("").to_string(),
            content: json["content"].as_str().unwrap_or("").to_string(),
            is_error: json["is_error"].as_bool().unwrap_or(false),
        },
        "attachments" => {
            let items_array = json["items"].as_array().cloned().unwrap_or_default();
            let items: Vec<Attachment> = items_array.iter().filter_map(json_to_attachment).collect();
            BlockKind::Attachments { items }
        }
        other => anyhow::bail!("unknown block kind: {other}"),
    };
    Ok(k)
}

fn attachment_to_json(a: &Attachment) -> serde_json::Value {
    let kind_str = match a.kind {
        AttachmentKind::Image => "image",
        AttachmentKind::Text => "text",
        AttachmentKind::Pdf => "pdf",
    };
    serde_json::json!({
        "id": a.id,
        "kind": kind_str,
        "display_name": a.display_name,
        "mime": a.mime,
        "local_path": a.local_path.to_string_lossy(),
        "size_bytes": a.size_bytes,
    })
}

fn json_to_attachment(v: &serde_json::Value) -> Option<Attachment> {
    let kind = match v.get("kind").and_then(|k| k.as_str()).unwrap_or("text") {
        "image" => AttachmentKind::Image,
        "text" => AttachmentKind::Text,
        "pdf" => AttachmentKind::Pdf,
        _ => return None,
    };
    Some(Attachment {
        id: v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        kind,
        display_name: v.get("display_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        mime: v.get("mime").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        local_path: PathBuf::from(v.get("local_path").and_then(|x| x.as_str()).unwrap_or("")),
        size_bytes: v.get("size_bytes").and_then(|x| x.as_u64()).unwrap_or(0),
    })
}
