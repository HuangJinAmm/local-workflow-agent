# UI Agent 模块去重 — 改用 lib 已有实现

> **状态**：已批准，待转 writing-plans
> **日期**：2026-07-01
> **范围**：B 集群（Provider/Model 注册表 + 附件上传下沉）
> **目标**：把 `src/ui/services/agent/` 下与 lib 重复的逻辑全部替换为 lib 已有实现，UI 仅保留 GUI 特有层

---

## 1. 背景与动机

`src/ui/services/agent/` 目录是 chat-ai 时代的产物，许多逻辑与项目 lib（`src/api/`、`src/core/`、`src/query/`）重复实现，且 UI 路径漏掉了 lib 的关键能力：

| 问题 | 后果 |
|---|---|
| `AgentBuilder::build` 只读 `ANTHROPIC_API_KEY` env 变量 | 切到 DeepSeek/Qwen 时 key 完全读不到 |
| `build_provider_request` 不查 `capabilities()` | 切到不支持 tool/image 的模型时直接报错 |
| `rebuild_provider` 手写 3 路 match | 不跟随 lib 新增 provider，已有 38 个 compat provider 都没接上 |
| `PROVIDER_PRESETS` 硬编码 13 项 | lib 有 50+ provider，UI 列表过期 |
| `ToolDefinition`/`ContentBlock` 重定义 | 翻译函数 `message_to_lib`/`content_block_to_lib` 纯属冗余 |
| `default_system_prompt` 硬编码 3 行字符串 | 不启用 lib 的 prompt caching 分段 |
| `upload_file` 在 UI 层直连 Anthropic HTTP | lib 无 Files API 封装，UI 持有 HTTP 客户端属于层级倒置 |

本设计消除以上重复，并把附件上传下沉到 lib。

---

## 2. 架构总览

### 2.1 改造矩阵

| 层 | 当前状态 | 改造后 |
|---|---|---|
| Provider 构造 | `client.rs::rebuild_provider` 手写 3 路 match | 调 `api::registry::provider_from_key(id, key)` + `with_base_url` |
| Key/base_url 解析 | 仅读 `ANTHROPIC_API_KEY` + `ANTHROPIC_API_BASE` | 调 `Config::resolve_provider_api_key/base(provider_id)` |
| Capabilities | 完全不查 | 在 `build_provider_request` 复用 `query/mod.rs:1043-1081` 范式过滤 tools/Image/Document |
| Provider 列表 | `PROVIDER_PRESETS` 硬编码 13 项 + 中文 label | `ModelRegistry::list_providers()` 返回英文名（接受英文化） |
| ToolDefinition | `types.rs` 重定义 | `pub use core::types::ToolDefinition` |
| ContentBlock | `types.rs` 重定义 4 变体 | `pub use core::types::ContentBlock`（含 13+ 变体） |
| Message | `types.rs` enum 形态 | **保留**（与 lib struct 形状不同，序列化路径不同） |
| System prompt | 3 行硬编码字符串 | `core::system_prompt::build_system_prompt(opts)` |
| Model 默认值 | `claude-haiku-4-5-20251001` 魔法字符串 | `effective_model_for_config(config, registry)` |
| File upload | `files.rs` 直接调 Anthropic HTTP | 下沉到 `api::uploads` 新模块，UI 仅调用 |
| `FileSource::File { file_id }` | UI 独有，lib 无对应 | 在 `core::types::DocumentSource` 和 `ImageSource` 新增 `file_id: Option<String>` 字段 |

### 2.2 保留不动

- `messages.rs`（UI 通道类型，几乎全部 UI 独有）
- `permission_handler.rs`（lib 第 5 个 PermissionHandler 实现，不重复）
- `types.rs` 的 `Message` / `Tool`（形状/用途与 lib 不同）
- `core::attachments`（语义不同，是上下文注入，不与 `api::uploads` 合并）

---

## 3. 新 lib 模块：`api::uploads`

**文件**：`src/api/uploads.rs`
**职责**：把本地文件上传到 provider 并返回可嵌入对话的 `ContentBlock`。

### 3.1 公共 API

```rust
pub struct UploadedFile {
    pub file_id: String,
    pub filename: String,
    pub bytes: u64,
    pub mime: String,
}

pub enum FileKind { Image, Document, Other }

pub enum UploadError {
    Io(std::io::Error),
    Http(reqwest::Error),
    Api(String),           // provider 返回的错误 JSON
    TooLarge { bytes: u64, limit: u64, kind: FileKind },
}

/// 上传文件到 Anthropic Files API。
/// 内置大小校验：Image ≤5MB，Document/Text ≤50MB（与项目 memory 约束一致）。
pub async fn upload_anthropic(api_key: &str, path: &Path) -> Result<UploadedFile, UploadError>;

/// 按 provider 能力决定如何把 UploadedFile 嵌入对话。
/// - Anthropic + caps.pdf_input=true 且 mime 是 application/pdf
///   → ContentBlock::Document { source: DocumentSource { source_type: "file", file_id: Some(...), .. } }
/// - caps.image_input=true 且 mime 是 image/*
///   → ContentBlock::Image { source: ImageSource { source_type: "file", file_id: Some(...), .. } }
/// - 不支持 → ContentBlock::Text { text: "[附件: <filename>]" }
pub fn to_content_block(up: &UploadedFile, caps: &ProviderCapabilities) -> ContentBlock;

/// 扩展名 → MIME（从 ui/services/agent/files.rs:17-30 迁入并扩展为 pub）
pub fn get_mime_type(path: &Path) -> &'static str;
```

### 3.2 支持的文件类型

| 扩展名 | MIME | 类别 |
|---|---|---|
| `.pdf` | `application/pdf` | Document |
| `.txt` `.md` | `text/plain` | Text |
| `.json` | `application/json` | Text |
| `.csv` | `text/csv` | Text |
| `.jpg` `.jpeg` | `image/jpeg` | Image |
| `.png` | `image/png` | Image |
| `.gif` | `image/gif` | Image |
| `.webp` | `image/webp` | Image |
| 其他 | `application/octet-stream` | 兜底（Anthropic 通常会拒收） |

### 3.3 大小限制（与项目 memory 一致）

- Image：≤ 5 MB
- Text/PDF：≤ 50 MB
- 超出返回 `UploadError::TooLarge`

### 3.4 实现要点

- 复用 `core::constants::{ANTHROPIC_API_BASE, ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER}` 而非硬编码 URL/header
- multipart body 构造逻辑从 `ui/services/agent/files.rs:46-60` 迁入
- `ANTHROPIC_BETA_HEADER` 已包含 `files-api-2025-04-14`，直接复用

### 3.5 UI 侧改动

- `ui/services/agent/files.rs` **删除**
- `ui/services/agent/mod.rs` 改为 `pub use crate::api::uploads::{UploadedFile, upload_anthropic, to_content_block, FileKind, UploadError, get_mime_type}`
- `handler.rs:122` 调用点：
  - 旧：`upload_file(&api_key, &path).await` → 返回 `String` (file_id)
  - 新：`uploads::upload_anthropic(&api_key, &path).await` → 返回 `UploadedFile`
  - 然后用 `uploads::to_content_block(&up, &provider.capabilities())` 生成 `ContentBlock`，append 到 `user_content`

---

## 4. lib 类型扩展：`core::types`

`ImageSource` 和 `DocumentSource` 当前是 flat struct（`source_type: String` + 可选 `media_type`/`data`/`url`），不是 enum。在 `src/core/mod.rs` 中给两者新增 `file_id` 字段：

```rust
// core/mod.rs:255-265  ImageSource
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
    pub file_id: Option<String>,  // ← 新增
}

// core/mod.rs:267-277  DocumentSource
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
    pub file_id: Option<String>,  // ← 新增
}
```

### 4.1 序列化验证

- 当 `source_type = "file"` 且 `file_id = Some("...")` 时，序列化为 `{"type":"file","file_id":"..."}`，与 Anthropic Files-API wire 格式一致
- 现有 `base64`/`url` 路径不受影响（`file_id` 为 `None` 时 `skip_serializing_if` 跳过）
- 需要新增序列化单元测试确认 wire 格式
- 需提供 `ImageSource::file(file_id)` / `DocumentSource::file(file_id)` 构造助手

---

## 5. `client.rs` 改造明细

### 5.1 删除项

| 行号 | 内容 | 替换为 |
|---|---|---|
| 35-49 | `PROVIDER_PRESETS` 常量 | `ModelRegistry::list_providers()` 调用 |
| 87-92 | `default_system_prompt` 函数 | `core::system_prompt::build_system_prompt(opts)` |
| 145-200 | `rebuild_provider` 方法 | `api::registry::provider_from_key(&id, &key)` + `with_base_url` |
| 307-332 | `message_to_lib` 翻译函数 | **简化**：`ContentBlock` 统一后，内部 `content_block_to_lib` 调用全部删除，仅保留 UI `Message` enum → `core::types::Message` struct 的外壳转换（因 Message 形态保留，见 §6/§11.1） |
| 334-363 | `content_block_to_lib` 翻译函数 | **删除**（`ContentBlock` 统一后无需翻译） |
| 385 | `model: "claude-haiku-4-5-20251001"` | `effective_model_for_config(&config, &registry)` |
| 425-434 | `AgentBuilder::build` 中 key/base 解析 | `Config::resolve_provider_api_key/base` |

### 5.2 新增项：capabilities 过滤

`build_provider_request` 中新增 capabilities 过滤（复用 `query/mod.rs:1043-1081` 范式）：

```rust
pub fn build_provider_request(&mut self, user_content: Vec<ContentBlock>) -> Result<ProviderRequest> {
    let caps = self.provider.capabilities();
    // 过滤 tools
    let tools = if caps.tool_calling {
        self.tools.iter().map(|t| t.to_definition()).collect()
    } else { vec![] };
    // 过滤 Image/Document 块
    let filtered = user_content.into_iter().map(|b| match &b {
        ContentBlock::Image { .. } if !caps.image_input =>
            ContentBlock::Text { text: "[Image not supported]".into() },
        ContentBlock::Document { .. } if !caps.pdf_input =>
            ContentBlock::Text { text: "[PDF not supported]".into() },
        _ => b,
    }).collect();
    // ... 组装 ProviderRequest
}
```

### 5.3 保留项

- `Agent` struct 本身、`AgentBuilder` builder 模式
- `set_provider`/`set_api_key`/`set_base_url`/`set_api_config`/`set_model`/`clear_conversation`/`add_user_message`/`provider_arc`/`api_key`/`base_url`/`model` getter
- `set_provider` 仍清空 conversation（保持当前行为）

---

## 6. `types.rs` 改造明细

| 类型 | 处理 |
|---|---|
| `ToolDefinition` (74-79) | **删除**，`pub use crate::core::types::ToolDefinition` |
| `ContentBlock` (43-63) | **删除**，`pub use crate::core::types::ContentBlock` |
| `FileSource` (66-71) | **删除**（已下沉到 `core::types::DocumentSource::File` 和 `ImageSource::File`） |
| `Message` (29-40) | **保留**（enum 形态，与 lib struct 不同，序列化路径不同） |
| `Tool` (21-26) | **保留**（UI 数据载体，lib 是 trait） |

---

## 7. `handler.rs` 与 `chat.rs` 改动

### 7.1 `handler.rs`

- `AgentRequest::Chat` 分支中 `upload_file(&api_key, &path)` 改为 `uploads::upload_anthropic(&api_key, &path).await`
- 上传成功后用 `uploads::to_content_block(&up, &provider.capabilities())` 生成 `ContentBlock`，append 到 `user_content`
- 失败处理保持 `AgentResponse::Error` 不变

### 7.2 `chat.rs`

- 无需改动（消费的 `ContentBlock` 通过 `pub use` 透明切换到 lib 版本，字段访问路径不变）

---

## 8. Provider 列表英文化

`PROVIDER_PRESETS` 删除后，`settings_panel.rs` 的 provider 下拉框改用 `ModelRegistry::list_providers()`：

- 返回 `Vec<ProviderEntry>`，每项含 `id` 和 `name`
- `id` 用于 `SetProvider` 请求
- `name`（英文，如 "Qwen"、"Ollama"）用于下拉框显示
- 不再维护中文 label 映射表（用户已批准英文化）

---

## 9. 测试策略

### 9.1 `api::uploads` 单元测试

- `get_mime_type` 全扩展名覆盖（含兜底 `octet-stream`）
- `to_content_block` 在不同 caps 下的分支：
  - image_input=true + image mime → `ContentBlock::Image { source: ImageSource { source_type: "file", file_id: Some(...), .. } }`
  - pdf_input=true + pdf mime → `ContentBlock::Document { source: DocumentSource { source_type: "file", file_id: Some(...), .. } }`
  - image_input=false → `ContentBlock::Text { "[Image not supported]" }`
  - pdf_input=false → `ContentBlock::Text { "[PDF not supported]" }`
  - 不支持的 mime → `ContentBlock::Text { "[附件: <filename>]" }`
- 大小校验：5MB/50MB 边界（用 `MockProvider` 或纯函数测试）

### 9.2 `core::types` 序列化测试

- `DocumentSource { source_type: "file", file_id: Some("..."), .. }` 序列化为 `{"type":"file","file_id":"..."}` 确认 wire 格式
- `ImageSource { source_type: "file", file_id: Some("..."), .. }` 同上
- 反序列化往返测试

### 9.3 `client.rs` 集成测试

- `build_provider_request` 在 `caps.tool_calling=false` 时不带 tools
- `build_provider_request` 在 `caps.image_input=false` 时把 Image 块替换为 Text
- `build_provider_request` 在 `caps.pdf_input=false` 时把 Document 块替换为 Text
- `AgentBuilder::build` 在 `DEEPSEEK_API_KEY` env 变量下能读到 key（验证 `resolve_provider_api_key` 接入）

### 9.4 回归测试

- 现有 376 个测试全部通过
- GUI 构建成功
- CLI 构建成功

---

## 10. 范围边界（不做）

- **不做** ModelRegistry 网络刷新（`refresh_from_models_dev`）—— 仍用 bundled snapshot
- **不做** `list_models()` 动态调用 —— provider 下拉用 `ModelRegistry::list_providers()`，model 仍由用户输入或 `effective_model_for_config` 自动选
- **不改** `messages.rs`（UI 通道类型保留）
- **不改** `permission_handler.rs`（GUI 特有桥接）
- **不改** `core::attachments`（语义不同，不合并）
- **不做** model 下拉候选列表（用户仍是文本输入，由 `effective_model_for_config` 提供默认值）

---

## 11. 关键风险点

1. **`types.rs::Message` 是 enum，`core::types::Message` 是 struct** —— `Message` 保留 UI 形态，但 `message_to_lib` 翻译函数会简化（`ContentBlock` 统一后不再需要 `content_block_to_lib`，但 `Message` 本身的 enum→struct 转换仍需保留）

2. **`rebuild_provider` 的 `"anthropic"` 分支用 `AnthropicClient::new(ClientConfig{...})?`**，而 `provider_from_key` 用 `AnthropicProvider::from_config(ClientConfig{...})`（`registry.rs:66-68`）—— 后者不开 `?`，失败行为不同。替换时注意错误传播路径。

3. **`PROVIDER_PRESETS` 删除后，`settings_panel.rs` 的下拉框初始化时序**：`ModelRegistry` 需要 hydrate bundled snapshot，必须在 GUI 启动早期完成。需确认 `ModelRegistry::default()` / `ModelRegistry::bundled()` 的初始化是同步的。

4. **`GuiPermissionHandler` 的 `block_in_place` 依赖多线程 runtime** —— 这是 UI 整体的前置假设，不受本次重构影响。

5. **`ANTHROPIC_BETA_HEADER` 常量是逗号分隔的长字符串** —— `uploads::upload_anthropic` 直接复用即可，但需确认 `files-api-2025-04-14` 子串确实在其中（memory 记录已包含）。
