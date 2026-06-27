// ui::model — pure data types, no GPUI imports.

use std::path::PathBuf;

pub type SessionId = String;
pub type MessageId = String;
pub type BlockId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    ToolResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Text,
    Pdf,
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub id: BlockId,
    pub kind: AttachmentKind,
    pub display_name: String,
    pub mime: String,
    pub local_path: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct UiSession {
    pub id: SessionId,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct UiMessage {
    pub id: MessageId,
    pub session_id: SessionId,
    pub role: Role,
    pub created_at: i64,
    pub ordinal: i32,
}

#[derive(Debug, Clone)]
pub struct UiBlock {
    pub id: BlockId,
    pub message_id: MessageId,
    pub ordinal: i32,
    pub kind: BlockKind,
}

#[derive(Debug, Clone)]
pub enum BlockKind {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Attachments { items: Vec<Attachment> },
}
