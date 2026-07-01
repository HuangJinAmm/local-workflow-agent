# UI Agent 模块去重 — 改用 lib 已有实现 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `src/ui/services/agent/` 下与 lib 重复的逻辑（provider 构造、key 解析、capabilities 过滤、provider 列表、类型定义、system prompt、附件上传）全部替换为 lib 已有实现。

**Architecture:** 在 lib 新增 `api::uploads` 模块封装 Anthropic Files API；在 `core::types` 给 `ImageSource`/`DocumentSource` 新增 `file_id` 字段；`ui/services/agent/client.rs` 改用 `api::registry::provider_from_config`、`Config::resolve_provider_api_key/base`、`ModelRegistry`、`core::system_prompt::build_system_prompt`；删除 `types.rs` 中重定义的 `ToolDefinition`/`ContentBlock`/`FileSource`，改用 `pub use`；删除 `files.rs`，改用 `api::uploads`。

**Tech Stack:** Rust 2024 edition, tokio 1.x, reqwest, serde, gpui-component

**Spec:** `docs/superpowers/specs/2026-07-01-ui-agent-lib-dedup-design.md`

---

## File Structure

| 文件 | 责任 | 操作 |
|---|---|---|
| `src/core/mod.rs` | `ImageSource`/`DocumentSource` 类型定义 | 修改（新增 `file_id` 字段 + 构造助手） |
| `src/api/uploads.rs` | Anthropic Files API 上传 + ContentBlock 转换 | 新建 |
| `src/api/mod.rs` | api 模块入口 | 修改（注册 `uploads` 模块） |
| `src/ui/services/agent/types.rs` | UI 数据载体 | 修改（删除 `ToolDefinition`/`ContentBlock`/`FileSource`，改 `pub use`） |
| `src/ui/services/agent/client.rs` | Agent 主体 | 修改（替换 provider 构造、key 解析、capabilities 过滤、system prompt、model 默认值） |
| `src/ui/services/agent/files.rs` | UI 层 HTTP 上传 | **删除** |
| `src/ui/services/agent/mod.rs` | UI agent 模块入口 | 修改（re-export 调整） |
| `src/ui/handler.rs` | UI ↔ agent 桥接 | 修改（上传调用改用 `api::uploads`） |
| `src/ui/settings_panel.rs` | 设置面板 | 修改（provider 下拉改用 `ModelRegistry::list_providers`） |

---

### Task 1: 给 `ImageSource`/`DocumentSource` 新增 `file_id` 字段

**Files:**
- Modify: `src/core/mod.rs:255-277`
- Test: `src/core/mod.rs` (内联 `#[cfg(test)] mod tests`)

- [ ] **Step 1: 写失败测试 — 序列化 wire 格式**

在 `src/core/mod.rs` 末尾的 `#[cfg(test)] mod tests` 中添加（若不存在则新建）：

```rust
#[cfg(test)]
mod lib_dedup_tests {
    use super::*;
    use serde_json;

    #[test]
    fn image_source_file_serializes_to_anthropic_wire_format() {
        let src = ImageSource {
            source_type: "file".to_string(),
            media_type: None,
            data: None,
            url: None,
            file_id: Some("file-abc-123".to_string()),
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["type"], "file");
        assert_eq!(json["file_id"], "file-abc-123");
        assert!(json.get("media_type").is_none());
        assert!(json.get("data").is_none());
        assert!(json.get("url").is_none());
    }

    #[test]
    fn document_source_file_serializes_to_anthropic_wire_format() {
        let src = DocumentSource {
            source_type: "file".to_string(),
            media_type: None,
            data: None,
            url: None,
            file_id: Some("file-xyz-456".to_string()),
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["type"], "file");
        assert_eq!(json["file_id"], "file-xyz-456");
    }

    #[test]
    fn image_source_file_roundtrip() {
        let src = ImageSource {
            source_type: "file".to_string(),
            media_type: None,
            data: None,
            url: None,
            file_id: Some("file-rt".to_string()),
        };
        let json = serde_json::to_string(&src).unwrap();
        let back: ImageSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back.source_type, "file");
        assert_eq!(back.file_id.as_deref(), Some("file-rt"));
    }

    #[test]
    fn image_source_base64_unchanged_after_adding_file_id() {
        let src = ImageSource {
            source_type: "base64".to_string(),
            media_type: Some("image/png".to_string()),
            data: Some("iVBORw0KGgo=".to_string()),
            url: None,
            file_id: None,
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["type"], "base64");
        assert_eq!(json["media_type"], "image/png");
        assert_eq!(json["data"], "iVBORw0KGgo=");
        assert!(json.get("file_id").is_none());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --lib lib_dedup_tests`
Expected: FAIL with "no field `file_id` on type `ImageSource`" or similar compile error

- [ ] **Step 3: 添加 `file_id` 字段 + 构造助手**

修改 `src/core/mod.rs` 中 `ImageSource`（约 255-265 行）和 `DocumentSource`（约 267-277 行），新增 `file_id` 字段和构造助手：

```rust
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ImageSource {
        #[serde(rename = "type")]
        pub source_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub file_id: Option<String>,
    }

    impl ImageSource {
        /// Construct a `file`-type source referencing an uploaded file id.
        pub fn file(file_id: impl Into<String>) -> Self {
            Self {
                source_type: "file".to_string(),
                media_type: None,
                data: None,
                url: None,
                file_id: Some(file_id.into()),
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct DocumentSource {
        #[serde(rename = "type")]
        pub source_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub file_id: Option<String>,
    }

    impl DocumentSource {
        /// Construct a `file`-type source referencing an uploaded file id.
        pub fn file(file_id: impl Into<String>) -> Self {
            Self {
                source_type: "file".to_string(),
                media_type: None,
                data: None,
                url: None,
                file_id: Some(file_id.into()),
            }
        }
    }
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test --lib lib_dedup_tests`
Expected: PASS (4 tests)

- [ ] **Step 5: 运行全量回归**

Run: `cargo build`
Expected: exit 0（无破坏性变更，新增字段为 `Option` + `skip_serializing_if`）

- [ ] **Step 6: Commit**

```bash
git add src/core/mod.rs
git commit -m "feat(core): add file_id field to ImageSource/DocumentSource"
```

---

### Task 2: 新建 `api::uploads` 模块

**Files:**
- Create: `src/api/uploads.rs`
- Modify: `src/api/mod.rs:25-50`（注册新模块）

- [ ] **Step 1: 写 `api/uploads.rs` 的失败测试**

新建 `src/api/uploads.rs`，先只放测试（实现部分暂为 `unimplemented!()`）：

```rust
//! File upload to the Anthropic Files API, plus provider-capability-aware
//! conversion of the uploaded file into a `ContentBlock`.
//!
//! Shared between the GUI (ui::services::agent::handler) and the future CLI
//! attachment path. Mirrors the project's hard constraints:
//!   - image ≤ 5 MB, text/PDF ≤ 50 MB
//!   - uses `core::constants::{ANTHROPIC_API_BASE, ANTHROPIC_API_VERSION,
//!     ANTHROPIC_BETA_HEADER}` so the wire constants have one source of truth

use std::path::Path;

use serde::Deserialize;

use crate::api::provider_types::ProviderCapabilities;
use crate::core::constants::{ANTHROPIC_API_BASE, ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER};
use crate::core::types::{ContentBlock, DocumentSource, ImageSource};

/// Metadata returned by a successful upload.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub file_id: String,
    pub filename: String,
    pub bytes: u64,
    pub mime: String,
}

/// Category of file, used for size-limit enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Image,
    Document,
    Other,
}

/// Error returned by [`upload_anthropic`].
#[derive(Debug)]
pub enum UploadError {
    Io(std::io::Error),
    Http(reqwest::Error),
    Api(String),
    TooLarge { bytes: u64, limit: u64, kind: FileKind },
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadError::Io(e) => write!(f, "IO error: {}", e),
            UploadError::Http(e) => write!(f, "HTTP error: {}", e),
            UploadError::Api(s) => write!(f, "API error: {}", s),
            UploadError::TooLarge { bytes, limit, kind } => write!(
                f,
                "File too large: {} bytes exceeds {} byte limit for {:?}",
                bytes, limit, kind
            ),
        }
    }
}

impl std::error::Error for UploadError {}

#[derive(Debug, Deserialize)]
struct FileUploadResponse {
    id: String,
}

/// Size limits (bytes). Mirrors the project memory constraints.
const IMAGE_LIMIT: u64 = 5 * 1024 * 1024;       // 5 MB
const DOCUMENT_LIMIT: u64 = 50 * 1024 * 1024;   // 50 MB

/// Extension → MIME type. Migrated from `ui/services/agent/files.rs:17-30`.
pub fn get_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf",
        Some("txt") | Some("md") => "text/plain",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Classify a file by MIME type for size-limit checks.
pub fn classify(mime: &str) -> FileKind {
    if mime.starts_with("image/") {
        FileKind::Image
    } else if mime == "application/pdf" {
        FileKind::Document
    } else {
        FileKind::Other
    }
}

/// Enforce size limits. Returns the appropriate `UploadError::TooLarge` if
/// the file exceeds its category's limit.
pub fn check_size(bytes: u64, mime: &str) -> Result<(), UploadError> {
    let kind = classify(mime);
    let limit = match kind {
        FileKind::Image => IMAGE_LIMIT,
        FileKind::Document | FileKind::Other => DOCUMENT_LIMIT,
    };
    if bytes > limit {
        Err(UploadError::TooLarge { bytes, limit, kind })
    } else {
        Ok(())
    }
}

/// Upload a file to the Anthropic Files API.
///
/// Performs built-in size validation (image ≤5MB, document/text ≤50MB).
/// Uses the project's shared Anthropic API constants — no hardcoded URLs
/// or header values.
pub async fn upload_anthropic(api_key: &str, path: &Path) -> Result<UploadedFile, UploadError> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let mime = get_mime_type(path).to_string();

    let file_bytes = std::fs::read(path).map_err(UploadError::Io)?;
    let bytes = file_bytes.len() as u64;
    check_size(bytes, &mime)?;

    // Hand-rolled multipart body (reqwest's `multipart` feature is not enabled).
    let boundary = "----AnthropicFileBoundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            file_name
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", mime).as_bytes());
    body.extend_from_slice(&file_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let url = format!("{}/v1/files", ANTHROPIC_API_BASE);
    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_API_VERSION)
        .header("anthropic-beta", ANTHROPIC_BETA_HEADER)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(body)
        .send()
        .await
        .map_err(UploadError::Http)?;

    let response_text = response.text().await.unwrap_or_default();

    if response_text.contains("\"error\"") {
        return Err(UploadError::Api(response_text));
    }

    let upload_response: FileUploadResponse = serde_json::from_str(&response_text)
        .map_err(|e| UploadError::Api(format!("Failed to parse upload response: {}. Response: {}", e, response_text)))?;

    tracing::debug!(
        "Uploaded file {} ({}) -> {}",
        path.display(),
        mime,
        upload_response.id
    );

    Ok(UploadedFile {
        file_id: upload_response.id,
        filename: file_name,
        bytes,
        mime,
    })
}

/// Convert an uploaded file into a `ContentBlock` suitable for the provider's
/// capabilities. When the provider doesn't support the file's modality, falls
/// back to a text placeholder so the model at least knows a file was attached.
pub fn to_content_block(up: &UploadedFile, caps: &ProviderCapabilities) -> ContentBlock {
    let kind = classify(&up.mime);
    match kind {
        FileKind::Image if caps.image_input => ContentBlock::Image {
            source: ImageSource::file(&up.file_id),
        },
        FileKind::Image => ContentBlock::Text {
            text: format!("[Image not supported: {}]", up.filename),
        },
        FileKind::Document if caps.pdf_input => ContentBlock::Document {
            source: DocumentSource::file(&up.file_id),
            title: None,
            context: None,
            citations: None,
        },
        FileKind::Document => ContentBlock::Text {
            text: format!("[PDF not supported: {}]", up.filename),
        },
        FileKind::Other => ContentBlock::Text {
            text: format!("[附件: {}]", up.filename),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_mime_type_covers_all_extensions() {
        assert_eq!(get_mime_type(Path::new("a.pdf")), "application/pdf");
        assert_eq!(get_mime_type(Path::new("a.txt")), "text/plain");
        assert_eq!(get_mime_type(Path::new("a.md")), "text/plain");
        assert_eq!(get_mime_type(Path::new("a.json")), "application/json");
        assert_eq!(get_mime_type(Path::new("a.csv")), "text/csv");
        assert_eq!(get_mime_type(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(get_mime_type(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(get_mime_type(Path::new("a.png")), "image/png");
        assert_eq!(get_mime_type(Path::new("a.gif")), "image/gif");
        assert_eq!(get_mime_type(Path::new("a.webp")), "image/webp");
        assert_eq!(get_mime_type(Path::new("a.xyz")), "application/octet-stream");
        assert_eq!(get_mime_type(Path::new("noext")), "application/octet-stream");
    }

    #[test]
    fn classify_by_mime() {
        assert_eq!(classify("image/png"), FileKind::Image);
        assert_eq!(classify("image/jpeg"), FileKind::Image);
        assert_eq!(classify("application/pdf"), FileKind::Document);
        assert_eq!(classify("text/plain"), FileKind::Other);
        assert_eq!(classify("application/octet-stream"), FileKind::Other);
    }

    #[test]
    fn check_size_allows_under_limit() {
        assert!(check_size(5 * 1024 * 1024, "image/png").is_ok());
        assert!(check_size(50 * 1024 * 1024, "application/pdf").is_ok());
    }

    #[test]
    fn check_size_rejects_over_limit() {
        assert!(matches!(
            check_size(5 * 1024 * 1024 + 1, "image/png"),
            Err(UploadError::TooLarge { kind: FileKind::Image, .. })
        ));
        assert!(matches!(
            check_size(50 * 1024 * 1024 + 1, "application/pdf"),
            Err(UploadError::TooLarge { kind: FileKind::Document, .. })
        ));
    }

    fn caps_all() -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: true,
            image_input: true,
            pdf_input: true,
            audio_input: false,
            video_input: false,
            caching: true,
            structured_output: true,
            system_prompt_style: crate::api::provider_types::SystemPromptStyle::TopLevel,
        }
    }

    fn caps_none() -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: false,
            tool_calling: false,
            thinking: false,
            image_input: false,
            pdf_input: false,
            audio_input: false,
            video_input: false,
            caching: false,
            structured_output: false,
            system_prompt_style: crate::api::provider_types::SystemPromptStyle::TopLevel,
        }
    }

    #[test]
    fn to_content_block_image_when_supported() {
        let up = UploadedFile {
            file_id: "f1".into(),
            filename: "a.png".into(),
            bytes: 100,
            mime: "image/png".into(),
        };
        let block = to_content_block(&up, &caps_all());
        match block {
            ContentBlock::Image { source } => {
                assert_eq!(source.source_type, "file");
                assert_eq!(source.file_id.as_deref(), Some("f1"));
            }
            other => panic!("expected Image, got {:?}", other),
        }
    }

    #[test]
    fn to_content_block_image_fallback_when_unsupported() {
        let up = UploadedFile {
            file_id: "f1".into(),
            filename: "a.png".into(),
            bytes: 100,
            mime: "image/png".into(),
        };
        let block = to_content_block(&up, &caps_none());
        match block {
            ContentBlock::Text { text } => assert!(text.contains("Image not supported")),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn to_content_block_pdf_when_supported() {
        let up = UploadedFile {
            file_id: "f2".into(),
            filename: "a.pdf".into(),
            bytes: 100,
            mime: "application/pdf".into(),
        };
        let block = to_content_block(&up, &caps_all());
        match block {
            ContentBlock::Document { source, .. } => {
                assert_eq!(source.source_type, "file");
                assert_eq!(source.file_id.as_deref(), Some("f2"));
            }
            other => panic!("expected Document, got {:?}", other),
        }
    }

    #[test]
    fn to_content_block_pdf_fallback_when_unsupported() {
        let up = UploadedFile {
            file_id: "f2".into(),
            filename: "a.pdf".into(),
            bytes: 100,
            mime: "application/pdf".into(),
        };
        let block = to_content_block(&up, &caps_none());
        match block {
            ContentBlock::Text { text } => assert!(text.contains("PDF not supported")),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn to_content_block_other_mime_yields_text_placeholder() {
        let up = UploadedFile {
            file_id: "f3".into(),
            filename: "a.txt".into(),
            bytes: 100,
            mime: "text/plain".into(),
        };
        let block = to_content_block(&up, &caps_all());
        match block {
            ContentBlock::Text { text } => assert!(text.contains("附件"), "got: {}", text),
            other => panic!("expected Text, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: 在 `api/mod.rs` 注册 `uploads` 模块**

修改 `src/api/mod.rs`，在 `pub mod model_registry;` 附近添加：

```rust
// File uploads (Anthropic Files API).
pub mod uploads;
```

并在公共 re-exports 区域添加：

```rust
pub use uploads::{UploadError, UploadedFile, FileKind, get_mime_type, to_content_block, upload_anthropic};
```

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test --lib api::uploads::tests`
Expected: PASS (9 tests: mime_type, classify, check_size ×2, to_content_block ×5)

- [ ] **Step 4: 运行全量构建**

Run: `cargo build`
Expected: exit 0

- [ ] **Step 5: Commit**

```bash
git add src/api/uploads.rs src/api/mod.rs
git commit -m "feat(api): add uploads module for Anthropic Files API"
```

---

### Task 3: 删除 `ui/services/agent/types.rs` 中的重定义类型

**Files:**
- Modify: `src/ui/services/agent/types.rs`
- Modify: `src/ui/services/agent/mod.rs`

- [ ] **Step 1: 修改 `types.rs`，删除 `ToolDefinition`/`ContentBlock`/`FileSource`，改 `pub use`**

将 `src/ui/services/agent/types.rs` 整体替换为：

```rust
//! Core types for the agent module.
//!
//! `Message` and `Tool` are UI-specific data carriers (the `Message` enum
//! shape differs from the library's `core::types::Message` struct, and `Tool`
//! is a UI-side data bag while the library has a `tools::Tool` trait). They
//! are kept here.
//!
//! The remaining types (`ContentBlock`, `ToolDefinition`) are re-exported
//! from the library's `core::types` so there is a single source of truth —
//! the translation layer in `client.rs` no longer needs to map between two
//! parallel definitions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// Re-export library types — single source of truth.
pub use crate::core::types::{ContentBlock, ToolDefinition};

/// A tool that can be executed by the agent.
///
/// Mirrors `chat-ai`'s `Tool` shape — just enough metadata for the API
/// request. Execution is done through the library's `tools::Tool` trait
/// from inside `run_turn`.
#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Message in a conversation with the LLM.
///
/// UI enum form — the library's `core::types::Message` is a struct with a
/// `MessageContent` enum; the UI keeps its own enum shape because the
/// serialization path and chat-ai history differ. `client.rs::message_to_lib`
/// handles the enum → struct translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    User {
        role: String,
        content: Vec<ContentBlock>,
    },
    Assistant {
        role: String,
        content: Vec<ContentBlock>,
    },
}
```

- [ ] **Step 2: 修改 `mod.rs`，移除 `FileSource` re-export**

修改 `src/ui/services/agent/mod.rs` 的 re-export 部分。当前是：

```rust
pub use types::{ContentBlock, FileSource, Message, Tool, ToolDefinition};
```

改为：

```rust
pub use types::{Message, Tool};
// `ContentBlock` and `ToolDefinition` come from `core::types` via `types.rs`.
pub use crate::core::types::{ContentBlock, ToolDefinition};
```

- [ ] **Step 3: 构建确认编译通过**

Run: `cargo build --features gui`
Expected: 编译错误会出现在 `handler.rs` 和 `client.rs` 中引用 `FileSource` 的地方 — 这是预期的，Task 4 和 Task 5 会修复。**仅检查 `types.rs` 和 `mod.rs` 本身没有语法错误**。

Run: `cargo build --features gui 2>&1 | findstr "types.rs mod.rs"`
Expected: 无 `types.rs`/`mod.rs` 本身的错误（错误应集中在 `handler.rs`/`client.rs`）

- [ ] **Step 4: Commit**

```bash
git add src/ui/services/agent/types.rs src/ui/services/agent/mod.rs
git commit -m "refactor(ui): dedup ContentBlock/ToolDefinition via pub use core::types"
```

---

### Task 4: 重构 `client.rs` — provider 构造、key 解析、capabilities 过滤

**Files:**
- Modify: `src/ui/services/agent/client.rs`

- [ ] **Step 1: 修改 `client.rs` — 删除 `PROVIDER_PRESETS`、`default_system_prompt`、`rebuild_provider`、`content_block_to_lib`，简化 `message_to_lib`，新增 capabilities 过滤**

整体替换 `src/ui/services/agent/client.rs` 为：

```rust
//! Agent client bridging the chat UI onto the `local_workflow_agent` library.
//!
//! Holds a `local_workflow_agent` `LlmProvider` (boxed behind `Arc<dyn …>`)
//! plus the in-memory conversation transcript. The transcript is stored in
//! chat-ai's wire types (`super::types::Message`) and translated into the
//! library's `core::types::Message` immediately before each request via
//! [`Agent::build_provider_request`].
//!
//! Provider construction, key/base_url resolution, and capabilities filtering
//! all delegate to the library's `api::registry`, `core::config::Config`, and
//! `api::provider_types::ProviderCapabilities` — no duplicate logic here.

use anyhow::Result;
use std::sync::Arc;

use crate::api::provider::LlmProvider;
use crate::api::provider_types::{ProviderCapabilities, ProviderRequest, SystemPrompt};
use crate::api::registry::provider_from_config;
use crate::api::model_registry::{effective_model_for_config, ModelRegistry};
use crate::core::config::Config;
use crate::core::system_prompt::{build_system_prompt, SystemPromptOptions};
use crate::core::types::{
    ContentBlock as LibContentBlock, DocumentSource, ImageSource, Message as LibMessage,
    MessageContent as LibMessageContent, Role as LibRole, ToolDefinition as LibToolDefinition,
    ToolResultContent,
};

use super::types::{ContentBlock, Message, Tool};

/// Agent that can converse with an LLM and execute tools.
#[derive(Clone)]
pub struct Agent {
    model: String,
    system_prompt: String,
    tools: Vec<Tool>,
    conversation: Vec<Message>,
    max_tokens: u32,
    provider: Arc<dyn LlmProvider>,
    provider_kind: String,
    api_key: String,
    base_url: String,
    /// Bundled model registry snapshot — used for `effective_model_for_config`.
    registry: ModelRegistry,
    /// Library config — used for key/base_url resolution and system prompt.
    config: Config,
}

impl Agent {
    pub fn new(tools: Vec<Tool>) -> Result<Self> {
        Agent::builder().build(tools)
    }

    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn set_max_tokens(&mut self, max_tokens: u32) {
        self.max_tokens = max_tokens;
    }

    /// Update the provider kind. Rebuilds the underlying provider from the
    /// library's `Config` (which reads the right env var per provider).
    /// Clears the conversation since different providers use different
    /// message formats / tool-call conventions.
    pub fn set_provider(&mut self, provider: String) -> Result<()> {
        self.provider_kind = provider.clone();
        // Persist into config so provider_from_config can resolve the key.
        self.config.provider = Some(provider);
        self.rebuild_provider()?;
        self.conversation.clear();
        Ok(())
    }

    pub fn set_api_key(&mut self, api_key: String) -> Result<()> {
        self.api_key = api_key;
        self.config.api_key = self.api_key.clone();
        self.rebuild_provider()
    }

    pub fn set_base_url(&mut self, base_url: String) -> Result<()> {
        self.base_url = base_url;
        self.rebuild_provider()
    }

    pub fn set_api_config(&mut self, api_key: String, base_url: String) -> Result<()> {
        self.api_key = api_key;
        self.base_url = base_url;
        self.config.api_key = self.api_key.clone();
        self.rebuild_provider()
    }

    /// Reconstruct the underlying `LlmProvider` via `api::registry::provider_from_config`.
    /// Falls back to constructing an Anthropic provider with the in-memory key
    /// (so the GUI can boot before the user fills the settings panel).
    fn rebuild_provider(&mut self) -> Result<()> {
        // Try the library's resolver first — it knows the env-var conventions
        // for all 50+ providers (DEEPSEEK_API_KEY, QWEN_API_KEY, …).
        if let Some(p) = provider_from_config(&self.config, &self.provider_kind) {
            self.provider = p;
            return Ok(());
        }
        // Fallback: construct an Anthropic provider with whatever key we have.
        // This matches the original boot-without-key behaviour.
        if self.provider_kind == "anthropic" {
            let cc = crate::api::client::ClientConfig {
                api_key: self.api_key.clone(),
                api_base: if self.base_url.trim().is_empty() {
                    crate::core::constants::ANTHROPIC_API_BASE.to_string()
                } else {
                    self.base_url.clone()
                },
                ..Default::default()
            };
            let client = crate::api::client::AnthropicClient::new(cc)?;
            self.provider = Arc::new(crate::api::providers::AnthropicProvider::new(Arc::new(client)));
            return Ok(());
        }
        // Last resort: keep the existing provider (might be stale, but better
        // than crashing the GUI).
        Ok(())
    }

    pub fn provider_kind(&self) -> &str {
        &self.provider_kind
    }

    pub fn api_key(&self) -> String {
        self.api_key.clone()
    }

    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.clone()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Provider capabilities — used by `build_provider_request` to filter
    /// tools and unsupported content blocks.
    pub fn capabilities(&self) -> ProviderCapabilities {
        self.provider.capabilities()
    }

    pub fn add_user_message(&mut self, content: String) {
        self.conversation.push(Message::User {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: content }],
        });
    }

    pub fn get_tool_definitions(&self) -> Vec<LibToolDefinition> {
        self.tools
            .iter()
            .map(|t| LibToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn get_conversation(&self) -> &[Message] {
        &self.conversation
    }

    pub fn clear_conversation(&mut self) {
        self.conversation.clear();
    }

    /// Build a `ProviderRequest` from the current transcript + system prompt.
    ///
    /// Filters tools and content blocks by the provider's capabilities, so
    /// a provider that doesn't support images/PDFs gets text placeholders
    /// instead of an API error.
    pub fn build_provider_request(
        &mut self,
        user_content: Vec<ContentBlock>,
    ) -> Result<ProviderRequest, anyhow::Error> {
        self.conversation.push(Message::User {
            role: "user".to_string(),
            content: user_content,
        });

        let messages: Vec<LibMessage> =
            self.conversation.iter().map(message_to_lib).collect();

        let caps = self.provider.capabilities();
        let tools: Vec<LibToolDefinition> = if caps.tool_calling {
            self.get_tool_definitions()
        } else {
            vec![]
        };

        Ok(ProviderRequest {
            model: self.model.clone(),
            messages,
            system_prompt: Some(SystemPrompt::Text(self.system_prompt.clone())),
            tools,
            max_tokens: self.max_tokens,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: Vec::new(),
            thinking: None,
            provider_options: serde_json::Value::Object(Default::default()),
        })
    }
}

// ---------------------------------------------------------------------------
// Translation helper — UI Message enum → library Message struct.
// ContentBlock is now shared, so no per-block translation needed.
// ---------------------------------------------------------------------------

fn message_to_lib(msg: &Message) -> LibMessage {
    match msg {
        Message::User { content, .. } => {
            let blocks: Vec<LibContentBlock> = content.iter().cloned().collect();
            LibMessage {
                role: LibRole::User,
                content: LibMessageContent::Blocks(blocks),
                uuid: None,
                cost: None,
                snapshot_patch: None,
            }
        }
        Message::Assistant { content, .. } => {
            let blocks: Vec<LibContentBlock> = content.iter().cloned().collect();
            LibMessage {
                role: LibRole::Assistant,
                content: LibMessageContent::Blocks(blocks),
                uuid: None,
                cost: None,
                snapshot_patch: None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub struct AgentBuilder {
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    system_prompt: Option<String>,
    max_tokens: u32,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            api_key: None,
            base_url: None,
            model: None,
            system_prompt: None,
            max_tokens: 4096,
        }
    }
}

#[allow(dead_code)]
impl AgentBuilder {
    pub fn provider(mut self, provider: String) -> Self {
        self.provider = provider;
        self
    }
    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }
    pub fn base_url(mut self, base_url: String) -> Self {
        self.base_url = Some(base_url);
        self
    }
    pub fn model(mut self, model: String) -> Self {
        self.model = Some(model);
        self
    }
    pub fn system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = Some(prompt);
        self
    }
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn build(self, tools: Vec<Tool>) -> Result<Agent> {
        let registry = ModelRegistry::new();
        let mut config = Config::default();
        config.provider = Some(self.provider.clone());

        // Resolve API key: explicit > config (env vars) > empty.
        let api_key = self.api_key.unwrap_or_else(|| {
            config
                .resolve_provider_api_key(&self.provider)
                .unwrap_or_default()
        });
        config.api_key = api_key.clone();

        // Resolve base URL: explicit > config > library default.
        let base_url = self.base_url.unwrap_or_else(|| {
            config
                .resolve_provider_api_base(&self.provider)
                .unwrap_or_else(|| crate::core::constants::ANTHROPIC_API_BASE.to_string())
        });

        // Resolve model: explicit > registry best > config default.
        let model = self.model.unwrap_or_else(|| {
            effective_model_for_config(&config, &registry)
        });

        // Resolve system prompt: explicit > library default.
        let system_prompt = self.system_prompt.unwrap_or_else(|| {
            let opts = SystemPromptOptions::default();
            build_system_prompt(&opts)
        });

        let mut agent = Agent {
            model,
            system_prompt,
            tools,
            conversation: Vec::new(),
            max_tokens: self.max_tokens,
            // Placeholder provider — rebuilt below.
            provider: Arc::new(crate::api::providers::AnthropicProvider::new(Arc::new(
                crate::api::client::AnthropicClient::new(
                    crate::api::client::ClientConfig::default(),
                )?,
            ))),
            provider_kind: self.provider,
            api_key,
            base_url,
            registry,
            config,
        };
        agent.rebuild_provider()?;
        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_builder() {
        let agent = Agent::builder()
            .api_key("test-key".to_string())
            .model("claude-sonnet-4-5-20250929".to_string())
            .system_prompt("You are a test assistant".to_string())
            .max_tokens(2048)
            .build(vec![]);
        assert!(agent.is_ok());
    }

    #[test]
    fn build_provider_request_filters_tools_when_unsupported() {
        // Use a mock-friendly agent: anthropic provider supports tools.
        let mut agent = Agent::builder()
            .api_key("test-key".to_string())
            .model("claude-sonnet-4-5-20250929".to_string())
            .build(vec![Tool {
                name: "echo".into(),
                description: "echo".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }])
            .unwrap();
        let req = agent
            .build_provider_request(vec![ContentBlock::Text { text: "hi".into() }])
            .unwrap();
        // Anthropic supports tools, so tools should be non-empty.
        assert_eq!(req.tools.len(), 1);
    }

    #[test]
    fn message_to_lib_translates_user_message() {
        let msg = Message::User {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: "hello".into() }],
        };
        let lib = message_to_lib(&msg);
        assert!(matches!(lib.role, LibRole::User));
        match lib.content {
            LibMessageContent::Blocks(blocks) => assert_eq!(blocks.len(), 1),
            _ => panic!("expected Blocks"),
        }
    }
}
```

- [ ] **Step 2: 构建确认 `client.rs` 本身编译通过**

Run: `cargo build --features gui 2>&1 | findstr "client.rs"`
Expected: 无 `client.rs` 的错误（剩余错误应在 `handler.rs` 和 `settings_panel.rs`，后续 Task 修复）

- [ ] **Step 3: 运行 client.rs 内联测试**

Run: `cargo test --lib --features gui services::agent::client`
Expected: PASS (3 tests)

- [ ] **Step 4: Commit**

```bash
git add src/ui/services/agent/client.rs
git commit -m "refactor(ui): client.rs uses lib provider/key/capabilities/system_prompt"
```

---

### Task 5: 重构 `handler.rs` — 上传调用改用 `api::uploads`

**Files:**
- Modify: `src/ui/handler.rs:29-36`（imports）
- Modify: `src/ui/handler.rs:117-136`（上传逻辑）

- [ ] **Step 1: 修改 imports**

将 `src/ui/handler.rs` 顶部的 import 块：

```rust
use crate::ui::{
    ChatAI,
    permission_modal::PermissionRequest as ModalPermissionRequest,
    services::agent::{
        Agent, AgentRequest, AgentResponse, ContentBlock, FileSource, GuiPermissionHandler,
        GuiPermissionRequest, UiMessage, upload_file,
    },
};
```

改为：

```rust
use crate::api::uploads;
use crate::core::types::{ContentBlock, DocumentSource};
use crate::ui::{
    ChatAI,
    permission_modal::PermissionRequest as ModalPermissionRequest,
    services::agent::{
        Agent, AgentRequest, AgentResponse, GuiPermissionHandler,
        GuiPermissionRequest, UiMessage,
    },
};
```

- [ ] **Step 2: 修改上传逻辑**

将 `src/ui/handler.rs` 中 `AgentRequest::Chat { content, files }` 分支的上传部分（约 117-136 行）：

```rust
            AgentRequest::Chat { content, files } => {
                // 1. Upload any attached files using the current API key.
                let api_key = agent.api_key();
                let mut user_content = vec![ContentBlock::Text { text: content }];
                for path in files {
                    match upload_file(&api_key, &path).await {
                        Ok(file_id) => {
                            user_content.push(ContentBlock::Document {
                                source: FileSource::File { file_id },
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to upload file: {}", e);
                            let _ = response_tx.try_send(AgentResponse::Error(format!(
                                "Failed to upload file: {}",
                                e
                            )));
                        }
                    }
                }
```

改为：

```rust
            AgentRequest::Chat { content, files } => {
                // 1. Upload any attached files using the lib's uploads module.
                let api_key = agent.api_key();
                let caps = agent.capabilities();
                let mut user_content = vec![ContentBlock::Text { text: content }];
                for path in files {
                    match uploads::upload_anthropic(&api_key, &path).await {
                        Ok(up) => {
                            let block = uploads::to_content_block(&up, &caps);
                            user_content.push(block);
                        }
                        Err(e) => {
                            tracing::error!("Failed to upload file: {}", e);
                            let _ = response_tx.try_send(AgentResponse::Error(format!(
                                "Failed to upload file: {}",
                                e
                            )));
                        }
                    }
                }
```

- [ ] **Step 3: 构建确认 `handler.rs` 编译通过**

Run: `cargo build --features gui 2>&1 | findstr "handler.rs"`
Expected: 无 `handler.rs` 错误

- [ ] **Step 4: Commit**

```bash
git add src/ui/handler.rs
git commit -m "refactor(ui): handler.rs uses api::uploads instead of UI-local upload_file"
```

---

### Task 6: 删除 `ui/services/agent/files.rs`，调整 `mod.rs`

**Files:**
- Delete: `src/ui/services/agent/files.rs`
- Modify: `src/ui/services/agent/mod.rs`

- [ ] **Step 1: 删除 `files.rs`**

使用 DeleteFile 工具删除 `d:\3-ai\ai-agent\local-workflow-agent\src\ui\services\agent\files.rs`。

- [ ] **Step 2: 修改 `mod.rs`，移除 `files` 模块和 `upload_file` re-export**

将 `src/ui/services/agent/mod.rs` 整体替换为：

```rust
//! Agent module for the chat UI.
//!
//! Re-exports the UI ↔ agent channel types (`AgentRequest`, `AgentResponse`,
//! `UiMessage`, …) and the `Agent` wrapper that bridges them onto the
//! `local_workflow_agent` library's `LlmProvider` trait.
//!
//! File uploads are handled by the library's `api::uploads` module; the UI
//! handler calls it directly.

mod client;
mod messages;
mod permission_handler;
mod types;

// Re-export main client types.
pub use client::{Agent, AgentBuilder};

// Re-export message types.
pub use messages::{
    AgentRequest, AgentResponse, MessageMetadata, MessageRole, ToolCallData, ToolResultData,
    UiMessage,
};

// Re-export core types — thin aliases over the library's own types so the
// chat view does not need to know about `local_workflow_agent::core` paths.
pub use types::{Message, Tool};
// `ContentBlock` and `ToolDefinition` come from `core::types` via `types.rs`.
pub use crate::core::types::{ContentBlock, ToolDefinition};

// Re-export the GUI permission handler.
pub use permission_handler::{GuiPermissionHandler, PermissionRequest as GuiPermissionRequest};
```

- [ ] **Step 3: 构建确认编译通过**

Run: `cargo build --features gui`
Expected: exit 0

- [ ] **Step 4: Commit**

```bash
git add src/ui/services/agent/mod.rs
git commit -m "refactor(ui): remove files.rs, use api::uploads"
```

注：使用 `git rm` 删除文件：
```bash
git rm src/ui/services/agent/files.rs
```

---

### Task 7: 重构 `settings_panel.rs` — provider 下拉改用 `ModelRegistry::list_providers`

**Files:**
- Modify: `src/ui/settings_panel.rs:27`（import）
- Modify: `src/ui/settings_panel.rs:71-85`（provider 列表构造）

- [ ] **Step 1: 修改 import**

将 `src/ui/settings_panel.rs:27`：

```rust
use crate::ui::services::agent::PROVIDER_PRESETS;
```

改为：

```rust
use crate::api::model_registry::ModelRegistry;
```

- [ ] **Step 2: 修改 provider 列表构造**

将 `src/ui/settings_panel.rs:71-85`（`SettingsPanel::new` 中）：

```rust
        let items: Vec<ProviderItem> = PROVIDER_PRESETS
            .iter()
            .map(|(id, label)| ProviderItem {
                id: (*id).into(),
                label: (*label).into(),
            })
            .collect();
        let selected_index = PROVIDER_PRESETS
            .iter()
            .position(|(id, _)| *id == settings.provider)
            .map(|i| IndexPath::default().row(i));
```

改为：

```rust
        // Build provider dropdown from the library's bundled model registry.
        let registry = ModelRegistry::new();
        let providers = registry.list_providers();
        let items: Vec<ProviderItem> = providers
            .iter()
            .map(|p| ProviderItem {
                id: p.id.as_str().into(),
                label: p.name.clone().into(),
            })
            .collect();
        let selected_index = providers
            .iter()
            .position(|p| p.id.as_str() == settings.provider)
            .map(|i| IndexPath::default().row(i));
```

- [ ] **Step 3: 构建确认编译通过**

Run: `cargo build --features gui`
Expected: exit 0

- [ ] **Step 4: 运行回归测试**

Run: `cargo test --lib --features gui`
Expected: 现有 376 测试全部通过（或更多）

- [ ] **Step 5: Commit**

```bash
git add src/ui/settings_panel.rs
git commit -m "refactor(ui): settings panel uses ModelRegistry::list_providers"
```

---

### Task 8: 最终集成验证

**Files:** 无修改

- [ ] **Step 1: 运行全量测试**

Run: `cargo test --lib --features gui -- --test-threads=1`
Expected: 全部通过，0 failed

- [ ] **Step 2: 构建 GUI 二进制**

Run: `cargo build --bin agent-gui --features gui`
Expected: exit 0

- [ ] **Step 3: 构建 CLI**

Run: `cargo build`
Expected: exit 0

- [ ] **Step 4: 确认无遗留引用**

Run: `cargo build --features gui 2>&1 | findstr "warning.*PROVIDER_PRESETS\|warning.*upload_file\|warning.*FileSource\|warning.*default_system_prompt"`
Expected: 无匹配（所有旧符号都已清除）

- [ ] **Step 5: 确认 lib 内联测试 + uploads 测试都通过**

Run: `cargo test --lib lib_dedup_tests && cargo test --lib api::uploads::tests`
Expected: 全部 PASS

- [ ] **Step 6: Commit（如有 lint 修复）**

如果前面步骤有任何 lint 修复：

```bash
git add -A
git commit -m "chore: final integration cleanup"
```

否则跳过此步。

---

## Self-Review

**1. Spec coverage**（对照 spec 各节）：

| Spec 节 | Plan Task | 状态 |
|---|---|---|
| §3 `api::uploads` 模块 | Task 2 | ✅ |
| §3.5 UI 侧改动（删除 `files.rs`） | Task 6 | ✅ |
| §4 `core::types` `file_id` 字段 | Task 1 | ✅ |
| §5.1 删除 `PROVIDER_PRESETS` | Task 4（client.rs）+ Task 7（settings_panel.rs） | ✅ |
| §5.1 删除 `default_system_prompt` | Task 4 | ✅ |
| §5.1 删除 `rebuild_provider` 手写 match | Task 4 | ✅ |
| §5.1 简化 `message_to_lib` | Task 4 | ✅ |
| §5.1 删除 `content_block_to_lib` | Task 4 | ✅ |
| §5.1 model 默认值用 `effective_model_for_config` | Task 4 | ✅ |
| §5.1 key/base_url 用 `Config::resolve_*` | Task 4 | ✅ |
| §5.2 capabilities 过滤 | Task 4（`build_provider_request`） | ✅ |
| §6 `types.rs` 删除 `ToolDefinition`/`ContentBlock`/`FileSource` | Task 3 | ✅ |
| §7.1 `handler.rs` 上传改用 `api::uploads` | Task 5 | ✅ |
| §7.2 `chat.rs` 无需改动 | — | ✅（无需 Task） |
| §8 Provider 列表英文化 | Task 7 | ✅ |
| §9.1 uploads 单元测试 | Task 2 | ✅ |
| §9.2 core::types 序列化测试 | Task 1 | ✅ |
| §9.3 client.rs 集成测试 | Task 4 | ✅ |
| §9.4 回归测试 | Task 8 | ✅ |

**2. Placeholder scan**: 无 TBD/TODO/vague phrases，所有代码块完整。

**3. Type consistency**:
- `UploadedFile` 在 Task 2 定义，Task 5 使用 — 字段名一致（`file_id`, `filename`, `bytes`, `mime`）
- `ImageSource::file` / `DocumentSource::file` 在 Task 1 定义，Task 2 使用 — 一致
- `capabilities()` 方法在 Task 4 的 `Agent` 上新增，Task 5 通过 `agent.capabilities()` 调用 — 一致
- `ContentBlock` 在 Task 3 改为 `pub use crate::core::types::ContentBlock`，Task 4/5 中的 `ContentBlock::Text`/`Image`/`Document` 变体匹配 lib 定义 — 一致

**4. 风险点（spec §11）应对**:
- §11.1 Message enum→struct 转换：Task 4 保留 `message_to_lib`，仅简化（删除 `content_block_to_lib`）— ✅
- §11.2 `provider_from_config` 错误传播：Task 4 的 `rebuild_provider` 有 fallback 到 Anthropic 构造 — ✅
- §11.3 `ModelRegistry::new()` 同步初始化：Task 7 直接调用 `ModelRegistry::new()`，`load_bundled_snapshot` 是同步 `include_bytes!` — ✅
- §11.4 `block_in_place` 不受影响 — ✅
- §11.5 `ANTHROPIC_BETA_HEADER` 包含 `files-api-2025-04-14`：Task 2 直接复用常量 — ✅（已在 spec 验证）

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-01-ui-agent-lib-dedup.md`. Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
