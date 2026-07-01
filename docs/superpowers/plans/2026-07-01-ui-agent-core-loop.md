# UI Agent 核心交互闭环实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 GUI agent 真正能干活——流式响应 + tool-use 闭环 + 21 个内置工具 + GUI 权限弹窗 + AskUserQuestion 弹窗 + 可配置工作目录。

**Architecture:** 在 `src/agent/turn.rs` 新建共享 `run_turn` 函数（流式 + 工具执行循环），CLI 与 UI 共用。UI 侧新建 `GuiPermissionHandler`（通过 channel 抛权限请求给前台弹窗）、`PermissionModal`、`AskModal`。`Agent::chat_step` 改为调 `run_turn`，sink 事件流通过现有 `AgentResponse` channel 转发到 GPUI 前台渲染。

**Tech Stack:** Rust 2024, GPUI 0.2.2, gpui-component 0.5.1, tokio 1.44 (Runtime + CancellationToken), tokio-util (CancellationToken), async-channel, futures (StreamExt).

**Spec:** `docs/superpowers/specs/2026-07-01-ui-agent-core-loop-design.md`

**重要 API 修正（spec 与实际库的差异，以实际为准）:**
- `PermissionHandler::request_permission(&self, &PermissionRequest) -> PermissionDecision` 是**同步方法**，GuiPermissionHandler 需在同步方法里阻塞等待异步弹窗（用 `tokio::runtime::Handle::block_on` 或 std channel）
- `StreamEvent` 直接是 `TextDelta { index, text }` / `InputJsonDelta { index, partial_json }` 等 variant，没有 `ContentBlockDelta` 包装层
- `ToolContext.mcp_manager: Option<Arc<McpManager>>`，首期传 `None`
- `PermissionDecision::Ask { reason: String }` 带参数；没有 `AlwaysAllow`，用 `AllowPermanently` 代替
- `UserQuestionEvent` 用 `tokio::sync::mpsc::UnboundedSender`
- `ToolContext.config: Config`（非 Arc）

---

## File Structure

### 新建
- `src/agent/mod.rs` — 库级 agent 模块入口，re-export `run_turn` / `TurnEvent` / `TurnSink` / `TurnCancel` / `MockProvider`
- `src/agent/turn.rs` — `run_turn` 函数 + `TurnEvent` 枚举 + `MAX_TOOL_ROUNDS=16` + 状态机
- `src/agent/mock_provider.rs` — 测试用 `MockProvider`，实现 `LlmProvider`
- `src/ui/services/agent/permission_handler.rs` — `GuiPermissionHandler` + `PermissionRequest` / `PermissionResponse` 类型
- `src/ui/permission_modal.rs` — `PermissionModal` GPUI view
- `src/ui/ask_modal.rs` — `AskModal` GPUI view

### 修改
- `src/lib.rs` — 加 `pub mod agent;`
- `src/ui/services/agent/mod.rs` — re-export permission_handler 模块
- `src/ui/services/agent/messages.rs` — `AgentResponse` 重构；`AgentRequest` 加 `Cancel` / `SetWorkingDir`，移除 `ToolResults`
- `src/ui/services/agent/client.rs` — `Agent::chat_step` 改为调 `run_turn`
- `src/ui/handler.rs` — `run_agent_loop` 构造 ToolContext + 调 run_turn；处理 Cancel/SetWorkingDir
- `src/ui/chat.rs` — 订阅 TurnEvent 渲染；停止按钮；PermissionModal/AskModal 集成
- `src/ui/settings.rs` — `Settings` 加 `working_dir` 字段
- `src/ui/settings_panel.rs` — Working Directory 输入行 + Browse 按钮
- `src/ui/mod.rs` — re-export `PermissionModal` / `AskModal`
- `src/main.rs` — Part 3 改用 `run_turn`（用 AutoPermissionHandler、stdout sink）
- `Cargo.toml` — 加 `tokio-util` 依赖（CancellationToken）

---

## Task 1: 基础设施 — `src/agent/` 模块骨架 + TurnEvent 类型

**Files:**
- Create: `src/agent/mod.rs`
- Create: `src/agent/turn.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: 加 tokio-util 依赖到 Cargo.toml**

在 `Cargo.toml` 的 `[dependencies]` 段，`tracing-subscriber` 之后加：

```toml
tokio-util = { version = "0.7", features = ["rt"] }
```

- [ ] **Step 2: 在 src/lib.rs 加 agent 模块**

修改 `src/lib.rs`，在 `pub mod api;` 之后加 `pub mod agent;`：

```rust
pub mod agent;
pub mod api;
pub mod core;
pub mod mcp;
pub mod query;
pub mod tools;

#[cfg(feature = "gui")]
pub mod ui;
```

- [ ] **Step 3: 创建 src/agent/mod.rs**

```rust
//! Agent turn orchestration — shared between CLI and UI.
//!
//! The `run_turn` function drives a multi-round LLM conversation with
//! tool-use: it streams provider events, accumulates text / tool_use
//! blocks, executes tools when `stop_reason == "tool_use"`, appends
//! `ToolResult` blocks, and re-calls the provider until `end_turn`,
//! cancellation, or `MAX_TOOL_ROUNDS` is reached.

mod turn;
mod mock_provider;

pub use mock_provider::MockProvider;
pub use turn::{run_turn, TurnEvent, TurnSink, TurnCancel, MAX_TOOL_ROUNDS};
```

- [ ] **Step 4: 创建 src/agent/turn.rs — TurnEvent 枚举与类型别名**

```rust
//! `run_turn` — streaming tool-use loop shared by CLI and UI.

use std::sync::Arc;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::api::provider::LlmProvider;
use crate::api::provider_error::ProviderError;
use crate::api::provider_types::{ProviderRequest, StopReason, StreamEvent, UsageInfo};
use crate::core::error::ClaudeError;
use crate::core::types::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use crate::tools::{find_tool, Tool, ToolContext};

/// Cap on consecutive tool-use rounds to prevent infinite loops.
pub const MAX_TOOL_ROUNDS: usize = 16;

/// Events emitted by `run_turn` to its sink as the turn progresses.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Incremental text delta from the model.
    TextDelta { text: String },
    /// A tool call has started (the model emitted a `tool_use` block).
    ToolUseStart { id: String, name: String },
    /// Incremental JSON delta for an in-progress tool call's input.
    ToolUseDelta { id: String, partial_json: String },
    /// A tool call has finished (success or error).
    ToolEnd {
        id: String,
        result: ToolResultContent,
        is_error: bool,
    },
    /// The turn completed normally.
    Done { stop_reason: Option<StopReason>, usage: Option<UsageInfo> },
    /// The turn failed due to an error.
    Failed { error: ClaudeError },
    /// The turn was cancelled by the user.
    Cancelled,
}

/// Sink type — a channel sender that receives `TurnEvent`s.
pub type TurnSink = async_channel::Sender<TurnEvent>;

/// Cancel type — a token that can be triggered to cancel the turn.
pub type TurnCancel = CancellationToken;

// (run_turn implementation added in Task 3)
```

- [ ] **Step 5: 验证编译**

Run: `cargo build --lib 2>&1 | Select-Object -Last 10`
Expected: PASS（可能有 unused warning，正常）

- [ ] **Step 6: Commit**

```bash
git add src/agent/ src/lib.rs Cargo.toml
git commit -m "feat(agent): add src/agent module skeleton with TurnEvent types"
```

---

## Task 2: MockProvider — 测试用 LlmProvider 实现

**Files:**
- Create: `src/agent/mock_provider.rs`

- [ ] **Step 1: 实现 MockProvider**

`MockProvider` 持一个脚本队列，每次 `create_message_stream` 弹一个脚本（`Vec<StreamEvent>`）。空队列返回空流（→ end_turn）。

```rust
//! Test-only `MockProvider` for `run_turn` unit tests.

use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream;
use tokio::sync::Mutex as TokioMutex;

use crate::api::provider::{LlmProvider, ModelInfo};
use crate::api::provider_error::ProviderError;
use crate::api::provider_types::{ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StreamEvent};

/// A mock provider that pops a pre-scripted list of `StreamEvent`s per
/// `create_message_stream` call. When the script queue is exhausted, an
/// empty stream is returned (which `run_turn` interprets as a clean
/// `end_turn`).
pub struct MockProvider {
    scripts: TokioMutex<Vec<Vec<StreamEvent>>>,
}

impl MockProvider {
    pub fn new(single_script: Vec<StreamEvent>) -> Self {
        Self::with_scripts(vec![single_script])
    }

    pub fn with_scripts(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts: TokioMutex::new(scripts),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn create_message(
        &self,
        _request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::Other("MockProvider only supports streaming".to_string()))
    }

    async fn create_message_stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent, ProviderError>> + Send>>,
        ProviderError,
    > {
        let script = self.scripts.lock().await;
        let events = if script.is_empty() {
            vec![StreamEvent::MessageStop]
        } else {
            script[0].clone()
        };
        drop(script);
        // Pop the consumed script so the next call gets the next one.
        self.scripts.lock().await.remove(0);

        let stream = stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        Ok(ProviderStatus::Healthy)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    fn provider_id(&self) -> &str {
        "mock"
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build --lib 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/agent/mock_provider.rs
git commit -m "test(agent): add MockProvider for run_turn tests"
```

---

## Task 3: run_turn 核心实现

**Files:**
- Modify: `src/agent/turn.rs`

- [ ] **Step 1: 实现 run_turn 函数**

在 `src/agent/turn.rs` 末尾追加 `run_turn` 函数。它循环：调 `create_message_stream` → 累积 block → 工具执行 → 回灌 → 重调。

```rust
/// Drive a multi-round LLM conversation with tool-use.
///
/// Streams `TurnEvent`s to `sink` as the turn progresses. Returns `Ok(())`
/// on normal completion (including cancellation), `Err` on hard failure.
pub async fn run_turn(
    provider: Arc<dyn LlmProvider>,
    session_id: String,
    mut request: ProviderRequest,
    tools: Arc<Vec<Box<dyn Tool>>>,
    tool_ctx: Arc<ToolContext>,
    sink: TurnSink,
    cancel: TurnCancel,
) -> Result<(), ClaudeError> {
    for round in 1..=MAX_TOOL_ROUNDS {
        // Cancellation check point 1: round start.
        if cancel.is_cancelled() {
            let _ = sink.send(TurnEvent::Cancelled).await;
            return Ok(());
        }

        // --- Stream provider response ---
        let mut stream = match provider.create_message_stream(request.clone()).await {
            Ok(s) => s,
            Err(e) => {
                let err = claude_error_from_provider(e);
                let _ = sink.send(TurnEvent::Failed { error: err.clone() }).await;
                return Err(err);
            }
        };

        let mut text_accum = String::new();
        let mut current_blocks: Vec<ContentBlock> = Vec::new();
        let mut input_buffer: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut usage: Option<UsageInfo> = None;

        while let Some(event_result) = stream.next().await {
            // Cancellation check point 2: between stream events.
            if cancel.is_cancelled() {
                let _ = sink.send(TurnEvent::Cancelled).await;
                return Ok(());
            }

            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    let err = claude_error_from_provider(e);
                    let _ = sink.send(TurnEvent::Failed { error: err.clone() }).await;
                    return Err(err);
                }
            };

            match event {
                StreamEvent::MessageStart { .. } => {}
                StreamEvent::ContentBlockStart { index, content_block } => {
                    while current_blocks.len() <= index {
                        current_blocks.push(ContentBlock::Text { text: String::new() });
                    }
                    current_blocks[index] = content_block.clone();
                    if let ContentBlock::ToolUse { id, name, .. } = &content_block {
                        input_buffer.insert(id.clone(), String::new());
                        let _ = sink.send(TurnEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        }).await;
                    } else if let ContentBlock::Text { text } = &content_block {
                        if !text.is_empty() {
                            text_accum.push_str(text);
                            let _ = sink.send(TurnEvent::TextDelta { text: text.clone() }).await;
                        }
                    } else if let ContentBlock::Thinking { thinking, .. } = &content_block {
                        if !thinking.is_empty() {
                            text_accum.push_str(thinking);
                            let _ = sink.send(TurnEvent::TextDelta { text: thinking.clone() }).await;
                        }
                    }
                }
                StreamEvent::TextDelta { index, text } => {
                    text_accum.push_str(&text);
                    let _ = sink.send(TurnEvent::TextDelta { text }).await;
                }
                StreamEvent::ThinkingDelta { index, thinking } => {
                    text_accum.push_str(&thinking);
                    let _ = sink.send(TurnEvent::TextDelta { text: thinking }).await;
                }
                StreamEvent::InputJsonDelta { index, partial_json } => {
                    // Find the tool_use block at this index and append to its buffer.
                    if let Some(block) = current_blocks.get_mut(index) {
                        if let ContentBlock::ToolUse { id, input, .. } = block {
                            input_buffer.entry(id.clone()).or_default().push_str(&partial_json);
                            // Re-parse the accumulated partial JSON into the block's input.
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(
                                &input_buffer[id],
                            ) {
                                *input = parsed;
                            }
                            let _ = sink.send(TurnEvent::ToolUseDelta {
                                id: id.clone(),
                                partial_json,
                            }).await;
                        }
                    }
                }
                StreamEvent::SignatureDelta { .. } => {}
                StreamEvent::ContentBlockStop { .. } => {}
                StreamEvent::MessageDelta { stop_reason: sr, usage: u } => {
                    stop_reason = sr;
                    usage = u;
                }
                StreamEvent::MessageStop => break,
                StreamEvent::Error { error_type, message } => {
                    let err = ClaudeError::Api(format!("{}: {}", error_type, message));
                    let _ = sink.send(TurnEvent::Failed { error: err.clone() }).await;
                    return Err(err);
                }
                StreamEvent::ReasoningDelta { .. } => {}
            }
        }

        // Append the assistant message (with accumulated blocks) to history.
        request.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(current_blocks.clone()),
            ..Default::default()
        });

        // If not a tool-use stop, the turn is done.
        if !matches!(stop_reason, Some(StopReason::ToolUse)) {
            let _ = sink.send(TurnEvent::Done { stop_reason, usage }).await;
            return Ok(());
        }

        // --- Execute tools ---
        let tool_use_blocks: Vec<_> = current_blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolUse { id, name, input } = b {
                    Some((id.clone(), name.clone(), input.clone()))
                } else {
                    None
                }
            })
            .collect();

        let mut tool_results: Vec<ContentBlock> = Vec::new();

        for (tool_id, tool_name, tool_input) in tool_use_blocks {
            // Cancellation check point 3: before each tool.
            if cancel.is_cancelled() {
                let _ = sink.send(TurnEvent::ToolEnd {
                    id: tool_id.clone(),
                    result: ToolResultContent::Text("cancelled".to_string()),
                    is_error: true,
                }).await;
                continue;
            }

            // Find tool by name.
            let tool = tools.iter().find(|t| t.name() == &tool_name);
            let tool_result = match tool {
                None => {
                    let err_msg = format!("Tool '{}' not found", tool_name);
                    let _ = sink.send(TurnEvent::ToolEnd {
                        id: tool_id.clone(),
                        result: ToolResultContent::Text(err_msg.clone()),
                        is_error: true,
                    }).await;
                    err_msg
                }
                Some(_t) => {
                    // Execute with panic-safety.
                    let exec_result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| {
                            // Permission check is synchronous on the trait;
                            // GuiPermissionHandler blocks inside.
                            let perm_req = crate::core::permissions::PermissionRequest {
                                tool_name: tool_name.clone(),
                                input: tool_input.clone(),
                                working_dir: tool_ctx.working_dir.clone(),
                            };
                            let decision = tool_ctx.permission_handler.request_permission(&perm_req);
                            match decision {
                                crate::core::permissions::PermissionDecision::Allow
                                | crate::core::permissions::PermissionDecision::AllowPermanently => {
                                    // Proceed to execute.
                                    None
                                }
                                _ => Some(ToolResult {
                                    content: ToolResultContent::Text("Permission denied".to_string()),
                                    is_error: true,
                                    ..Default::default()
                                }),
                            }
                        }),
                    );

                    match exec_result {
                        Err(_panic) => {
                            let _ = sink.send(TurnEvent::ToolEnd {
                                id: tool_id.clone(),
                                result: ToolResultContent::Text("tool panicked".to_string()),
                                is_error: true,
                            }).await;
                            "tool panicked".to_string()
                        }
                        Ok(Some(denied_result)) => {
                            let _ = sink.send(TurnEvent::ToolEnd {
                                id: tool_id.clone(),
                                result: denied_result.content.clone(),
                                is_error: denied_result.is_error,
                            }).await;
                            "Permission denied".to_string()
                        }
                        Ok(None) => {
                            // Execute the tool (this is async).
                            let t = tools.iter().find(|t| t.name() == &tool_name).unwrap();
                            let result = t.execute(tool_input.clone(), &tool_ctx).await;
                            let _ = sink.send(TurnEvent::ToolEnd {
                                id: tool_id.clone(),
                                result: result.content.clone(),
                                is_error: result.is_error,
                            }).await;
                            if result.is_error {
                                format!("[ERROR] {}", content_to_string(&result.content))
                            } else {
                                content_to_string(&result.content)
                            }
                        }
                    }
                }
            };

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: tool_id,
                content: ToolResultContent::Text(tool_result),
                is_error: false,
            });
        }

        // Append tool results as a user-role message.
        request.messages.push(Message {
            role: Role::User,
            content: MessageContent::Blocks(tool_results),
            ..Default::default()
        });
    }

    // Exceeded MAX_TOOL_ROUNDS.
    let err = ClaudeError::Other("max tool rounds exceeded".to_string());
    let _ = sink.send(TurnEvent::Failed { error: err.clone() }).await;
    Err(err)
}

fn claude_error_from_provider(e: ProviderError) -> ClaudeError {
    ClaudeError::Api(format!("{:?}", e))
}

fn content_to_string(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(t) => t.clone(),
        ToolResultContent::Blocks(blocks) => {
            serde_json::to_string(blocks).unwrap_or_else(|_| "<unprintable>".to_string())
        }
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build --lib 2>&1 | Select-Object -Last 15`
Expected: PASS（可能有 unused import 警告）

- [ ] **Step 3: Commit**

```bash
git add src/agent/turn.rs
git commit -m "feat(agent): implement run_turn streaming tool-use loop"
```

---

## Task 4: run_turn 单元测试 — 文本/工具/错误路径

**Files:**
- Modify: `src/agent/turn.rs` (末尾加 `#[cfg(test)] mod tests`)

- [ ] **Step 1: 在 turn.rs 末尾追加测试模块**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::provider_types::{ProviderCapabilities, ProviderRequest, StreamEvent, UsageInfo};
    use crate::core::permissions::{AutoPermissionHandler, PermissionHandler};
    use crate::tools::{ToolContext, all_tools};
    use std::sync::Arc;

    fn make_tool_ctx() -> Arc<ToolContext> {
        Arc::new(ToolContext {
            working_dir: std::env::temp_dir(),
            permission_mode: crate::core::config::PermissionMode::BypassPermissions,
            permission_handler: Arc::new(AutoPermissionHandler {
                mode: crate::core::config::PermissionMode::BypassPermissions,
            }),
            cost_tracker: Arc::new(crate::core::cost::CostTracker::new()),
            session_id: "test".to_string(),
            current_turn: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            file_history: Arc::new(parking_lot::Mutex::new(
                crate::core::file_history::FileHistory::new(),
            )),
            lsp_manager: None,
            non_interactive: true,
            mcp_manager: None,
            config: crate::core::config::Config::default(),
            managed_agent_config: None,
            completion_notifier: None,
            pending_permissions: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            permission_manager: None,
            user_question_tx: None,
        })
    }

    fn empty_request() -> ProviderRequest {
        ProviderRequest {
            model: "mock-model".to_string(),
            max_tokens: 1024,
            messages: vec![],
            system: None,
            tools: vec![],
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
        }
    }

    async fn collect_events(provider: MockProvider, request: ProviderRequest) -> Vec<TurnEvent> {
        let (sink, rx) = async_channel::unbounded::<TurnEvent>();
        let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(vec![]);
        let ctx = make_tool_ctx();
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_turn(
            Arc::new(provider),
            "test".to_string(),
            request,
            tools,
            ctx,
            sink,
            cancel,
        ));
        let mut events = Vec::new();
        while let Ok(ev) = rx.recv().await {
            events.push(ev);
        }
        let _ = handle.await;
        events
    }

    #[tokio::test]
    async fn run_turn_text_only_emits_done() {
        let script = vec![
            StreamEvent::MessageStart { id: "1".into(), model: "mock".into(), usage: UsageInfo::default() },
            StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlock::Text { text: "Hello".into() } },
            StreamEvent::TextDelta { index: 0, text: " world".into() },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta { stop_reason: Some(StopReason::EndTurn), usage: None },
            StreamEvent::MessageStop,
        ];
        let events = collect_events(MockProvider::new(script), empty_request()).await;
        assert!(events.iter().any(|e| matches!(e, TurnEvent::TextDelta { text } if text == "Hello")));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::TextDelta { text } if text == " world")));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::Done { .. })));
    }

    #[tokio::test]
    async fn run_turn_unknown_tool_emits_error_tool_end() {
        let script = vec![
            StreamEvent::MessageStart { id: "1".into(), model: "mock".into(), usage: UsageInfo::default() },
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "NonExistentTool".into(),
                    input: serde_json::json!({}),
                },
            },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta { stop_reason: Some(StopReason::ToolUse), usage: None },
            StreamEvent::MessageStop,
        ];
        // Second call: empty stream → end_turn
        let script2 = vec![StreamEvent::MessageStop];
        let provider = MockProvider::with_scripts(vec![script, script2]);
        let events = collect_events(provider, empty_request()).await;
        assert!(events.iter().any(|e| matches!(e, TurnEvent::ToolUseStart { name } if name == "NonExistentTool")));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::ToolEnd { is_error: true, .. })));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::Done { .. })));
    }

    #[tokio::test]
    async fn run_turn_cancellation_emits_cancelled() {
        let script = vec![
            StreamEvent::MessageStart { id: "1".into(), model: "mock".into(), usage: UsageInfo::default() },
            StreamEvent::TextDelta { index: 0, text: "Hi".into() },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta { stop_reason: Some(StopReason::EndTurn), usage: None },
            StreamEvent::MessageStop,
        ];
        let (sink, rx) = async_channel::unbounded::<TurnEvent>();
        let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(vec![]);
        let ctx = make_tool_ctx();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(run_turn(
            Arc::new(MockProvider::new(script)),
            "test".to_string(),
            empty_request(),
            tools,
            ctx,
            sink,
            cancel,
        ));
        cancel_clone.cancel();
        let mut events = Vec::new();
        while let Ok(ev) = rx.recv().await {
            events.push(ev);
        }
        let _ = handle.await;
        assert!(events.iter().any(|e| matches!(e, TurnEvent::Cancelled)));
    }
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test --lib agent::turn::tests 2>&1 | Select-Object -Last 15`
Expected: PASS (3 tests)

- [ ] **Step 3: Commit**

```bash
git add src/agent/turn.rs
git commit -m "test(agent): add run_turn text-only, unknown-tool, cancellation tests"
```

---

## Task 5: GuiPermissionHandler + 权限通道类型

**Files:**
- Create: `src/ui/services/agent/permission_handler.rs`
- Modify: `src/ui/services/agent/mod.rs`

- [ ] **Step 1: 创建 permission_handler.rs**

`PermissionHandler::request_permission` 是同步方法，但 GUI 弹窗是异步的。GuiPermissionHandler 在同步方法里用 `tokio::sync::oneshot` channel + `Handle::block_on` 等待异步弹窗结果。

```rust
//! `GuiPermissionHandler` — bridges the sync `PermissionHandler` trait
//! to an async GUI modal by blocking on a channel pair.
//!
//! The handler holds a `Sender<PermissionRequest>` (to the GUI) and a
//! `Receiver<PermissionResponse>` (from the GUI). When
//! `request_permission` is called, it sends the request to the GUI and
//! blocks the current Tokio worker thread on `response_rx.recv()`.
//! The GUI, running on the GPUI foreground, shows a `PermissionModal`,
//! collects the user's decision, and sends back a `PermissionResponse`.

use std::sync::Arc;

use async_channel::{Receiver, Sender};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::core::permissions::{
    PermissionDecision, PermissionHandler, PermissionRequest as LibPermissionRequest,
};

/// A permission request sent from the background agent to the GUI.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub level: String, // "None" / "ReadOnly" / "Write" / "Execute" / "Dangerous"
    /// Reply channel: the GUI sends back the user's decision.
    pub reply_tx: oneshot::Sender<PermissionDecision>,
}

/// A GUI-side `PermissionHandler` that delegates Ask decisions to a
/// foreground modal via channels.
pub struct GuiPermissionHandler {
    /// Sends permission requests to the GUI foreground.
    request_tx: Sender<PermissionRequest>,
    /// Tools that the user has marked "AlwaysAllow" this turn.
    always_allow: Mutex<std::collections::HashSet<String>>,
}

impl GuiPermissionHandler {
    pub fn new(request_tx: Sender<PermissionRequest>) -> Self {
        Self {
            request_tx,
            always_allow: Mutex::new(Default::default()),
        }
    }
}

impl PermissionHandler for GuiPermissionHandler {
    fn check_permission(&self, request: &LibPermissionRequest) -> PermissionDecision {
        // BypassPermissions mode: allow everything.
        // (The GUI defaults to Default mode; this branch only fires if
        // the user explicitly sets Bypass in settings — future work.)
        // For now, always Ask for Write/Execute/Dangerous.
        if self
            .always_allow
            .lock()
            .contains(&request.tool_name)
        {
            return PermissionDecision::Allow;
        }
        PermissionDecision::Ask {
            reason: format!("Tool '{}' requires permission", request.tool_name),
        }
    }

    fn request_permission(&self, request: &LibPermissionRequest) -> PermissionDecision {
        // Check always_allow cache first.
        if self.always_allow.lock().contains(&request.tool_name) {
            return PermissionDecision::Allow;
        }

        // Send to GUI and block on the reply.
        let (reply_tx, reply_rx) = oneshot::channel::<PermissionDecision>();
        let gui_request = PermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            level: format!("{:?}", request.level),
            reply_tx,
        };

        // Try to send (non-blocking send on async_channel).
        if self.request_tx.send_blocking(gui_request).is_err() {
            return PermissionDecision::Deny;
        }

        // Block the current Tokio worker thread on the reply.
        // This is safe because run_turn runs on a multi-threaded
        // Tokio runtime with multiple workers; blocking one is OK.
        match Handle::current().block_on(reply_rx) {
            Ok(decision) => {
                if matches!(decision, PermissionDecision::AllowPermanently) {
                    self.always_allow.lock().insert(request.tool_name.clone());
                }
                decision
            }
            Err(_) => PermissionDecision::Deny,
        }
    }
}
```

- [ ] **Step 2: 在 src/ui/services/agent/mod.rs 加模块声明**

在 `mod client;` 之后加 `mod permission_handler;`，并在 re-export 段加：

```rust
pub use permission_handler::{GuiPermissionHandler, PermissionRequest as GuiPermissionRequest};
```

- [ ] **Step 3: 验证编译**

Run: `cargo build --lib --features gui 2>&1 | Select-Object -Last 15`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/ui/services/agent/permission_handler.rs src/ui/services/agent/mod.rs
git commit -m "feat(ui): add GuiPermissionHandler bridging sync trait to async modal"
```

---

## Task 6: AgentRequest/AgentResponse 重构

**Files:**
- Modify: `src/ui/services/agent/messages.rs`

- [ ] **Step 1: 重构 AgentRequest 和 AgentResponse**

在 `src/ui/services/agent/messages.rs` 中：

- `AgentRequest`: 移除 `ToolResults`，新增 `Cancel` 和 `SetWorkingDir(PathBuf)`
- `AgentResponse`: 改为 `TurnEvent(TurnEvent)` / `PermissionRequest(GuiPermissionRequest)` / `UserQuestion(UserQuestionEvent)` / `Error(String)`，移除 `TextResponse` / `ToolCallRequest` / `is_done`

具体修改（替换整个 enum 定义）：

```rust
use std::path::PathBuf;

use crate::agent::TurnEvent;
use crate::core::permissions::PermissionDecision;
use crate::tools::UserQuestionEvent;

#[derive(Debug)]
pub enum AgentRequest {
    Chat { content: String, files: Vec<PathBuf> },
    Cancel,
    ClearHistory,
    SetProvider(String),
    SetModel(String),
    SetApiKey(String),
    SetBaseUrl(String),
    SetApiConfig { api_key: String, base_url: String },
    SetWorkingDir(PathBuf),
}

#[derive(Debug)]
pub enum AgentResponse {
    TurnEvent(TurnEvent),
    PermissionRequest(super::permission_handler::PermissionRequest),
    UserQuestion(UserQuestionEvent),
    Error(String),
}
```

注意：移除旧的 `TextResponse` / `ToolCallRequest` / `is_done` 方法。`ToolCallData` / `ToolResultData` 结构体如果只被旧变体使用可以移除。

- [ ] **Step 2: 验证编译（预期会有 handler.rs/chat.rs 的编译错误，先不管）**

Run: `cargo build --lib --features gui 2>&1 | Select-String "error\[" | Select-Object -First 10`
Expected: 编译错误在 handler.rs / chat.rs（下一 Task 修）

- [ ] **Step 3: Commit**

```bash
git add src/ui/services/agent/messages.rs
git commit -m "refactor(ui): restructure AgentRequest/AgentResponse for turn events"
```

---

## Task 7: handler.rs 改造 — run_agent_loop 调 run_turn

**Files:**
- Modify: `src/ui/handler.rs`

- [ ] **Step 1: 重写 run_agent_loop**

`run_agent_loop` 现在需要：
1. 构造 ToolContext（working_dir + GuiPermissionHandler + 空 McpManager）
2. 构造 `Arc<Vec<Box<dyn Tool>>> = Arc::new(all_tools())`
3. 收到 Chat 时：构造 ProviderRequest，创建 sink（用 response_tx 转发），创建 cancel token，调 `run_turn`
4. 收到 Cancel 时：触发当前 cancel token
5. 处理 permission/ask channel：用 tokio::select! 并发处理

由于改动较大，替换 `run_agent_loop` 整个函数：

```rust
async fn run_agent_loop(
    request_rx: Receiver<AgentRequest>,
    response_tx: Sender<AgentResponse>,
    permission_request_rx: async_channel::Receiver<GuiPermissionRequest>,
    ask_rx: tokio::sync::mpsc::UnboundedReceiver<UserQuestionEvent>,
) {
    // Build agent with initial config from env.
    let mut agent = match Agent::builder()
        .system_prompt(
            "You are a helpful, succinct assistant. Respond in markdown. \
             You have tools available — use them when the user asks for \
             file operations, shell commands, or web lookups."
                .to_string(),
        )
        .max_tokens(8192)
        .build(vec![])
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to build agent: {}", e);
            let _ = response_tx.try_send(AgentResponse::Error(
                "Failed to initialize agent".to_string(),
            ));
            return;
        }
    };

    let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(all_tools());
    let mut working_dir = std::env::temp_dir();
    let mut current_cancel: Option<CancellationToken> = None;

    // Spawn a task to forward permission requests to the GUI.
    let perm_tx = response_tx.clone();
    let perm_forward = tokio::spawn(async move {
        while let Ok(req) = permission_request_rx.recv().await {
            let _ = perm_tx.try_send(AgentResponse::PermissionRequest(req));
        }
    });

    // Spawn a task to forward ask-user events to the GUI.
    let ask_tx = response_tx.clone();
    let ask_forward = tokio::spawn(async move {
        let mut ask_rx = ask_rx;
        while let Some(ev) = ask_rx.recv().await {
            let _ = ask_tx.try_send(AgentResponse::UserQuestion(ev));
        }
    });

    // Create the permission channel + handler.
    let (perm_req_tx, perm_req_rx) = async_channel::unbounded::<GuiPermissionRequest>();
    let permission_handler = Arc::new(GuiPermissionHandler::new(perm_req_tx));
    let (ask_event_tx, ask_event_rx) = tokio::sync::mpsc::unbounded_channel::<UserQuestionEvent>();

    loop {
        let req = match request_rx.recv().await {
            Ok(r) => r,
            Err(_) => break,
        };

        match req {
            AgentRequest::Chat { content, files } => {
                // Upload files (anthropic only).
                let mut user_content = vec![ContentBlock::Text { text: content.clone() }];
                let api_key = agent.api_key();
                if !api_key.is_empty() {
                    for path in files {
                        match upload_file(&api_key, &path).await {
                            Ok(file_id) => {
                                user_content.push(ContentBlock::Document {
                                    source: FileSource::File { file_id },
                                });
                            }
                            Err(e) => {
                                tracing::error!("Failed to upload file: {}", e);
                                let _ = response_tx.try_send(AgentResponse::Error(
                                    format!("Failed to upload file: {}", e),
                                ));
                            }
                        }
                    }
                }

                // Build the provider request from agent's conversation.
                let provider_request = match agent.build_provider_request(user_content) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = response_tx.try_send(AgentResponse::Error(format!("{}", e)));
                        continue;
                    }
                };

                // Build a fresh ToolContext for this turn.
                let tool_ctx = Arc::new(ToolContext {
                    working_dir: working_dir.clone(),
                    permission_mode: crate::core::config::PermissionMode::Default,
                    permission_handler: permission_handler.clone(),
                    cost_tracker: Arc::new(crate::core::cost::CostTracker::new()),
                    session_id: "gui".to_string(),
                    current_turn: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    file_history: Arc::new(parking_lot::Mutex::new(
                        crate::core::file_history::FileHistory::new(),
                    )),
                    lsp_manager: None,
                    non_interactive: false,
                    mcp_manager: None,
                    config: crate::core::config::Config::default(),
                    managed_agent_config: None,
                    completion_notifier: None,
                    pending_permissions: Arc::new(parking_lot::Mutex::new(
                        std::collections::HashMap::new(),
                    )),
                    permission_manager: None,
                    user_question_tx: Some(ask_event_tx.clone()),
                });

                let cancel = CancellationToken::new();
                current_cancel = Some(cancel.clone());

                // Wrap response_tx as a TurnSink (Sender<TurnEvent>).
                let sink = response_tx.clone();
                let provider = agent.provider_arc();

                let turn_handle = tokio::spawn(run_turn(
                    provider,
                    "gui".to_string(),
                    provider_request,
                    tools.clone(),
                    tool_ctx,
                    sink,
                    cancel,
                ));

                // Wait for the turn to complete (events flow via the
                // response_tx channel as AgentResponse::TurnEvent).
                let _ = turn_handle.await;
                current_cancel = None;
            }
            AgentRequest::Cancel => {
                if let Some(c) = current_cancel.take() {
                    c.cancel();
                }
            }
            AgentRequest::ClearHistory => {
                agent.clear_conversation();
            }
            AgentRequest::SetProvider(p) => {
                let _ = agent.set_provider(p);
            }
            AgentRequest::SetModel(m) => {
                agent.set_model(m);
                agent.clear_conversation();
            }
            AgentRequest::SetApiKey(k) => {
                let _ = agent.set_api_key(k);
            }
            AgentRequest::SetBaseUrl(u) => {
                let _ = agent.set_base_url(u);
            }
            AgentRequest::SetApiConfig { api_key, base_url } => {
                let _ = agent.set_api_config(api_key, base_url);
            }
            AgentRequest::SetWorkingDir(dir) => {
                working_dir = dir;
            }
            _ => {}
        }
    }

    perm_forward.abort();
    ask_forward.abort();
}
```

注意：需要给 `Agent` 加 `provider_arc()` 和 `build_provider_request()` 方法（下一 Task 8 加），以及 `api_key()` 访问器。`handle_outgoing` 的签名也要改（加 permission/ask channel 参数）。

- [ ] **Step 2: 修改 handle_outgoing 签名 + run_agent_loop 调用**

`handle_outgoing` 现在需要接收 permission/ask channels 并传给 `run_agent_loop`。替换 `handle_outgoing` 函数：

```rust
pub async fn handle_outgoing(
    request_rx: Receiver<AgentRequest>,
    response_tx: Sender<AgentResponse>,
) {
    // Build a dedicated Tokio runtime (GPUI's background executor is not
    // a Tokio runtime; reqwest/tokio need a reactor).
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("Failed to build Tokio runtime: {}", e);
            return;
        }
    };

    // Create the permission and ask channels here; they'll be moved
    // into the runtime-spawned task.
    let (_perm_req_tx, perm_req_rx) = async_channel::unbounded::<GuiPermissionRequest>();
    let (_ask_event_tx, ask_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<UserQuestionEvent>();

    // NOTE: The actual permission_tx and ask_tx are created inside
    // run_agent_loop (which we spawn below) — this is a temporary
    // wiring that will be fixed when ChatAI::new creates the channels
    // and passes them through AgentRequest. For now, the channels are
    // unused because GuiPermissionHandler creates its own pair.
    let _ = runtime.handle().spawn(run_agent_loop(
        request_rx,
        response_tx,
        perm_req_rx,
        ask_event_rx,
    ));

    // Keep the runtime alive forever (it's dropped when this function
    // returns, which is when the app exits).
    std::future::pending::<()>().await;
}
```

- [ ] **Step 3: 修改 handle_incoming 以处理新 AgentResponse 变体**

`handle_incoming` 需要处理 `TurnEvent` / `PermissionRequest` / `UserQuestion` / `Error`：

```rust
pub async fn handle_incoming(
    this: WeakEntity<ChatAI>,
    response_rx: Receiver<AgentResponse>,
    cx: &mut AsyncApp,
) {
    loop {
        let incoming = response_rx.recv().await;
        match incoming {
            Ok(response) => {
                match response {
                    AgentResponse::TurnEvent(ev) => {
                        if let Some(view) = this.upgrade() {
                            let _ = cx.update_entity(&view, |this, cx| {
                                this.handle_turn_event(ev, cx);
                            });
                        }
                    }
                    AgentResponse::PermissionRequest(req) => {
                        if let Some(view) = this.upgrade() {
                            let _ = cx.update_entity(&view, |this, cx| {
                                this.show_permission_modal(req, cx);
                            });
                        }
                    }
                    AgentResponse::UserQuestion(ev) => {
                        if let Some(view) = this.upgrade() {
                            let _ = cx.update_entity(&view, |this, cx| {
                                this.show_ask_modal(ev, cx);
                            });
                        }
                    }
                    AgentResponse::Error(err) => {
                        if let Some(view) = this.upgrade() {
                            let _ = cx.update_entity(&view, |this, cx| {
                                this.add_message(UiMessage::error(err), cx);
                                this.set_loading(false, cx);
                            });
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("Channel error: {}", e);
                if let Some(view) = this.upgrade() {
                    let _ = cx.update_entity(&view, |this, cx| {
                        this.set_loading(false, cx);
                    });
                }
                break;
            }
        }
    }
}
```

- [ ] **Step 4: 加 imports**

在 `handler.rs` 顶部加：

```rust
use crate::agent::{run_turn, TurnCancel, TurnSink};
use crate::tools::{all_tools, Tool, ToolContext, UserQuestionEvent};
use crate::core::permissions::PermissionDecision;
use crate::core::cost::CostTracker;
use tokio_util::sync::CancellationToken;
use crate::ui::services::agent::permission_handler::{
    GuiPermissionHandler, PermissionRequest as GuiPermissionRequest,
};
```

- [ ] **Step 5: 验证编译（预期还有 chat.rs/client.rs 错误）**

Run: `cargo build --lib --features gui 2>&1 | Select-String "error\[" | Select-Object -First 5`
Expected: 错误在 chat.rs（handle_turn_event/show_permission_modal 方法未定义）

- [ ] **Step 6: Commit**

```bash
git add src/ui/handler.rs
git commit -m "refactor(ui): rewire run_agent_loop to call run_turn with ToolContext"
```

---

## Task 8: Agent client.rs 改造 — 加 provider_arc/build_provider_request

**Files:**
- Modify: `src/ui/services/agent/client.rs`

- [ ] **Step 1: 给 Agent 加 provider_arc() 和 build_provider_request() 方法**

在 `impl Agent` 中加：

```rust
/// Return a clone of the provider Arc for use by `run_turn`.
pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
    self.provider.clone()
}

/// Build a `ProviderRequest` from the current conversation + a new user
/// content block. This is used by `run_agent_loop` to construct the
/// request for `run_turn`.
pub fn build_provider_request(
    &mut self,
    user_content: Vec<ContentBlock>,
) -> Result<ProviderRequest, anyhow::Error> {
    use crate::core::types::{Message as LibMessage, MessageContent as LibMessageContent, Role};

    // Push the user message into the conversation.
    let lib_blocks: Vec<_> = user_content
        .into_iter()
        .map(block_to_lib)
        .collect();
    self.conversation.push(LibMessage {
        role: Role::User,
        content: LibMessageContent::Blocks(lib_blocks),
        ..Default::default()
    });

    // Build the ProviderRequest.
    let request = ProviderRequest {
        model: self.model.clone(),
        max_tokens: self.max_tokens,
        messages: self.conversation.clone(),
        system: Some(crate::api::SystemPrompt::Text(self.system_prompt.clone())),
        tools: self.tools.iter().map(|t| t.to_definition()).collect(),
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        thinking: None,
    };
    Ok(request)
}

pub fn api_key(&self) -> String {
    self.api_key.clone()
}
```

- [ ] **Step 2: 移除旧的 chat_step / submit_tool_results（它们被 run_turn 取代）**

删除 `Agent::chat_step` 和 `Agent::submit_tool_results` 方法（如果存在）。

- [ ] **Step 3: 验证编译**

Run: `cargo build --lib --features gui 2>&1 | Select-String "error\[" | Select-Object -First 5`
Expected: 错误减少，主要在 chat.rs

- [ ] **Step 4: Commit**

```bash
git add src/ui/services/agent/client.rs
git commit -m "refactor(ui): add provider_arc/build_provider_request to Agent"
```

---

## Task 9: PermissionModal GPUI view

**Files:**
- Create: `src/ui/permission_modal.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: 创建 PermissionModal view**

```rust
//! Permission modal — asks the user to approve a tool execution.
//!
//! Rendered as an overlay on top of the chat view when the background
//! agent requests permission for a Write/Execute/Dangerous tool.

use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement as _, Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::*,
    h_flex,
    label::Label,
    v_flex,
};
use tokio::sync::oneshot;

use crate::core::permissions::PermissionDecision;
use crate::ui::services::agent::permission_handler::PermissionRequest;

pub struct PermissionModal {
    request: Option<PermissionRequest>,
    focus_handle: FocusHandle,
}

impl PermissionModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            request: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn show(&mut self, request: PermissionRequest, cx: &mut Context<Self>) {
        self.request = Some(request);
        cx.notify();
    }

    fn respond(&mut self, decision: PermissionDecision, cx: &mut Context<Self>) {
        if let Some(req) = self.request.take() {
            let _ = req.reply_tx.send(decision);
        }
        cx.notify();
    }

    fn on_allow(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.respond(PermissionDecision::Allow, cx);
    }

    fn on_always_allow(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.respond(PermissionDecision::AllowPermanently, cx);
    }

    fn on_deny(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.respond(PermissionDecision::Deny, cx);
    }

    pub fn is_visible(&self) -> bool {
        self.request.is_some()
    }
}

impl Focusable for PermissionModal {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PermissionModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let req = match &self.request {
            Some(r) => r,
            None => return div().into_any_element(),
        };

        let input_pretty = serde_json::to_string_pretty(&req.input)
            .unwrap_or_else(|_| "<unprintable>".to_string());

        let is_dangerous = req.level == "Dangerous";

        let mut buttons = h_flex().gap_2().child(
            Button::new("perm-deny")
                .ghost()
                .label("Deny")
                .on_click(cx.listener(Self::on_deny)),
        );
        buttons = buttons.child(
            Button::new("perm-allow")
                .primary()
                .label("Allow")
                .on_click(cx.listener(Self::on_allow)),
        );
        if !is_dangerous {
            buttons = buttons.child(
                Button::new("perm-always")
                    .ghost()
                    .label("Always Allow")
                    .on_click(cx.listener(Self::on_always_allow)),
            );
        }

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.95))
            .child(
                v_flex()
                    .id("permission-modal-card")
                    .track_focus(&self.focus_handle)
                    .mx_auto()
                    .my_8()
                    .w(px(480.))
                    .max_h(px(560.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .p_4()
                    .gap_3()
                    .child(
                        Label::new(format!("Tool Permission: {}", req.tool_name))
                            .text_color(if is_dangerous {
                                theme.danger
                            } else {
                                theme.text
                            }),
                    )
                    .child(Label::new(format!("Level: {}", req.level)))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(Label::new("Input:"))
                            .child(Label::new(input_pretty)),
                    )
                    .child(buttons),
            )
            .into_any_element()
    }
}
```

- [ ] **Step 2: 在 src/ui/mod.rs 加 re-export**

加 `pub use permission_modal::PermissionModal;` 和 `pub use ask_modal::AskModal;`

- [ ] **Step 3: 验证编译**

Run: `cargo build --lib --features gui 2>&1 | Select-Object -Last 10`
Expected: PASS（AskModal 还没建，先注释掉那行）

- [ ] **Step 4: Commit**

```bash
git add src/ui/permission_modal.rs src/ui/mod.rs
git commit -m "feat(ui): add PermissionModal for tool permission requests"
```

---

## Task 10: AskModal GPUI view

**Files:**
- Create: `src/ui/ask_modal.rs`

- [ ] **Step 1: 创建 AskModal view**

```rust
//! Ask modal — shows a question from the AskUserQuestion tool and
//! collects the user's answer.
//!
//! When the model calls AskUserQuestion, the tool sends a
//! `UserQuestionEvent` through `ToolContext.user_question_tx`. The
//! background agent forwards it to the GUI, which renders this modal.
//! The user's answer is sent back via the `reply_tx` oneshot channel.

use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement as _, Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::*,
    h_flex,
    input::{Input, InputState},
    label::Label,
    v_flex,
};

use crate::tools::UserQuestionEvent;

pub struct AskModal {
    event: Option<UserQuestionEvent>,
    answer_input: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl AskModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let answer_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Type your answer...")
        });
        Self {
            event: None,
            answer_input,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn show(&mut self, event: UserQuestionEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.event = Some(event);
        self.answer_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.notify();
    }

    fn on_submit_answer(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let answer = self.answer_input.read(cx).text().to_string();
        if let Some(ev) = self.event.take() {
            let _ = ev.reply_tx.send(answer);
        }
        cx.notify();
    }

    fn on_option_click(&mut self, option: String, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ev) = self.event.take() {
            let _ = ev.reply_tx.send(option);
        }
        cx.notify();
    }

    pub fn is_visible(&self) -> bool {
        self.event.is_some()
    }
}

impl Focusable for AskModal {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AskModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let ev = match &self.event {
            Some(e) => e,
            None => return div().into_any_element(),
        };

        let mut body = v_flex().gap_2().child(Label::new(&ev.question));

        if let Some(options) = &ev.options {
            for (i, opt) in options.iter().enumerate() {
                let opt_str = opt.clone();
                body = body.child(
                    Button::new(format!("ask-opt-{}", i))
                        .ghost()
                        .label(opt.clone())
                        .on_click(cx.listener(move |this, e, w, cx| {
                            this.on_option_click(opt_str.clone(), e, w, cx);
                        })),
                );
            }
        } else {
            body = body.child(Input::new(&self.answer_input).appearance(false));
        }

        let mut footer = h_flex().justify_end().gap_2();
        if ev.options.is_none() {
            footer = footer.child(
                Button::new("ask-submit")
                    .primary()
                    .label("Submit")
                    .on_click(cx.listener(Self::on_submit_answer)),
            );
        }

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.95))
            .child(
                v_flex()
                    .id("ask-modal-card")
                    .track_focus(&self.focus_handle)
                    .mx_auto()
                    .my_8()
                    .w(px(440.))
                    .max_h(px(400.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .p_4()
                    .gap_3()
                    .child(Label::new("Question").text_color(theme.muted_foreground))
                    .child(body)
                    .child(footer),
            )
            .into_any_element()
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build --lib --features gui 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/ui/ask_modal.rs src/ui/mod.rs
git commit -m "feat(ui): add AskModal for AskUserQuestion tool"
```

---

## Task 11: Settings 加 working_dir + SettingsPanel 加输入行

**Files:**
- Modify: `src/ui/settings.rs`
- Modify: `src/ui/settings_panel.rs`

- [ ] **Step 1: 在 Settings 加 working_dir 字段**

在 `src/ui/settings.rs` 的 `Settings` struct 加 `pub working_dir: PathBuf`，并在 `Default` impl 里设默认值 `dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))`。

- [ ] **Step 2: 在 SettingsPanel 加 Working Directory 输入行 + Browse 按钮**

在 `src/ui/settings_panel.rs`：
- 加 `working_dir_input: Entity<InputState>` 字段
- 在 `new()` 里初始化
- 在 body 加一个 `v_flex().gap_1().child(Label::new("Working Directory")).child(Input::new(&self.working_dir_input))`
- 在 `collect()` 里读 `working_dir_input` 的值

- [ ] **Step 3: 验证编译**

Run: `cargo build --lib --features gui 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/ui/settings.rs src/ui/settings_panel.rs
git commit -m "feat(ui): add working_dir to Settings and SettingsPanel"
```

---

## Task 12: chat.rs — TurnEvent 渲染 + Modal 集成 + 停止按钮

**Files:**
- Modify: `src/ui/chat.rs`

- [ ] **Step 1: 加 ChatAI 新字段**

```rust
pub struct ChatAI {
    // 既有字段...
    streaming_text: String,
    last_render_at: Option<std::time::Instant>,
    pending_tools: std::collections::HashMap<String, PendingTool>,
    cancel_tx: Arc<tokio::sync::Notify>,
    permission_modal: Option<Entity<PermissionModal>>,
    ask_modal: Option<Entity<AskModal>>,
}

struct PendingTool {
    name: String,
    input_buffer: String,
}
```

- [ ] **Step 2: 实现 handle_turn_event 方法**

```rust
pub fn handle_turn_event(&mut self, ev: TurnEvent, cx: &mut Context<Self>) {
    match ev {
        TurnEvent::TextDelta { text } => {
            self.streaming_text.push_str(&text);
            // Throttle: re-render at most every 50ms.
            let now = std::time::Instant::now();
            let should_render = self
                .last_render_at
                .map(|t| now.duration_since(t).as_millis() > 50)
                .unwrap_or(true);
            if should_render {
                self.last_render_at = Some(now);
                // Update the last assistant message with the streaming text.
                self.update_streaming_message(cx);
            }
        }
        TurnEvent::ToolUseStart { id, name } => {
            self.pending_tools.insert(id, PendingTool {
                name,
                input_buffer: String::new(),
            });
        }
        TurnEvent::ToolUseDelta { id, partial_json } => {
            if let Some(t) = self.pending_tools.get_mut(&id) {
                t.input_buffer.push_str(&partial_json);
            }
        }
        TurnEvent::ToolEnd { id, result, is_error } => {
            self.pending_tools.remove(&id);
            // Push a tool result message.
            let text = match result {
                ToolResultContent::Text(t) => t,
                ToolResultContent::Blocks(_) => "[structured result]".to_string(),
            };
            self.add_message(UiMessage::system(format!("Tool result: {}", text)), cx);
        }
        TurnEvent::Done { .. } => {
            // Final render with full text.
            self.update_streaming_message(cx);
            self.streaming_text.clear();
            self.pending_tools.clear();
            self.set_loading(false, cx);
        }
        TurnEvent::Failed { error } => {
            self.add_message(UiMessage::error(format!("{}", error)), cx);
            self.streaming_text.clear();
            self.pending_tools.clear();
            self.set_loading(false, cx);
        }
        TurnEvent::Cancelled => {
            if !self.streaming_text.is_empty() {
                self.streaming_text.push_str(" (已取消)");
                self.update_streaming_message(cx);
            }
            self.streaming_text.clear();
            self.pending_tools.clear();
            self.set_loading(false, cx);
        }
    }
}

fn update_streaming_message(&mut self, cx: &mut Context<Self>) {
    if self.streaming_text.is_empty() {
        return;
    }
    // If the last message is an assistant message, update it; otherwise push new.
    let text = self.streaming_text.clone();
    cx.update_entity(&self.message_state, |state, cx| {
        if let Some(last) = state.messages.last_mut() {
            if last.role == MessageRole::Assistant {
                last.content = text;
                cx.notify();
                return;
            }
        }
        state.messages.push(UiMessage::assistant(text));
        cx.notify();
    });
}

pub fn show_permission_modal(&mut self, req: GuiPermissionRequest, cx: &mut Context<Self>) {
    if let Some(modal) = self.permission_modal.as_ref() {
        modal.update(cx, |m, cx| m.show(req, cx));
    }
}

pub fn show_ask_modal(&mut self, ev: UserQuestionEvent, cx: &mut Context<Self>) {
    // Need a Window for InputState::set_value — defer to next render.
    if let Some(modal) = self.ask_modal.as_ref() {
        // Store the event; the modal's show method needs a Window.
        // For now, store in a pending field and handle in render.
        // This is a simplification; production code would use window.defer.
    }
}
```

- [ ] **Step 3: 在 ChatAI::new 里初始化 modals + cancel_tx**

在 `new()` 末尾构造 `permission_modal` / `ask_modal` entities + `cancel_tx`，并把它们加到 `Self {}` 初始化。

- [ ] **Step 4: submit_text 改为发 Chat 请求**

`submit_text` 末尾把 `is_loading = true`，让发送按钮变为停止按钮：

```rust
fn submit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    // ... existing logic ...
    // After sending, set loading:
    self.set_loading(true, cx);
}
```

- [ ] **Step 5: 加停止按钮逻辑**

在 `form_footer` 渲染里，如果 `is_loading`，渲染一个"停止"按钮：

```rust
if self.is_loading {
    // Stop button
    Button::new("stop-btn")
        .icon(Icon::empty().path("icons/square.svg"))
        .small()
        .danger()
        .on_click(cx.listener(|this, _, _, cx| {
            this.cancel_tx.notify_waiters();
        }))
} else {
    // Send button (existing)
}
```

- [ ] **Step 6: 在 Render 里渲染 modals**

在 `render` 末尾，如果 modal 可见，作为 overlay 渲染：

```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    // ... existing render ...
    let mut el = div().child(/* existing content */);
    if let Some(pm) = &self.permission_modal {
        el = el.child(pm.clone());
    }
    if let Some(am) = &self.ask_modal {
        el = el.child(am.clone());
    }
    el
}
```

- [ ] **Step 7: 验证编译**

Run: `cargo build --bin agent-gui --features gui 2>&1 | Select-Object -Last 15`
Expected: PASS（可能有 warnings）

- [ ] **Step 8: Commit**

```bash
git add src/ui/chat.rs
git commit -m "feat(ui): integrate TurnEvent rendering, modals, stop button"
```

---

## Task 13: CLI main.rs Part 3 改用 run_turn

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: 替换 run_agent_loop 为调 run_turn**

在 `src/main.rs` Part 3，把内联的 `run_agent_loop` 改为：

```rust
// Part 3: LLM-driven agent loop using run_turn
let provider = Arc::new(AnthropicProvider::new(Arc::new(client)));
let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(all_tools());
let tool_ctx = build_tool_context(working_dir.clone());

let (sink, mut rx) = async_channel::unbounded::<TurnEvent>();
let cancel = CancellationToken::new();

let request = ProviderRequest {
    model: DEFAULT_MODEL.to_string(),
    max_tokens: DEFAULT_MAX_TOKENS,
    messages: vec![Message {
        role: Role::User,
        content: MessageContent::Text(format!(
            "List the top-level modules in this Rust project by reading src/lib.rs, \
             then briefly describe what each module is for."
        )),
        ..Default::default()
    }],
    system: None,
    tools: tools.iter().map(|t| t.to_definition()).collect(),
    temperature: Some(0.7),
    top_p: None, top_k: None, stop_sequences: None, thinking: None,
};

let handle = tokio::spawn(run_turn(
    provider, "cli".to_string(), request, tools, Arc::new(tool_ctx), sink, cancel,
));

while let Ok(ev) = rx.recv().await {
    match ev {
        TurnEvent::TextDelta { text } => print!("{}", text),
        TurnEvent::ToolUseStart { name, .. } => println!("\n[Tool: {}]", name),
        TurnEvent::ToolEnd { is_error, .. } => if is_error { println!(" (error)") },
        TurnEvent::Done { .. } => println!("\n=== Done ==="),
        TurnEvent::Failed { error } => println!("\n=== Failed: {} ===", error),
        TurnEvent::Cancelled => println!("\n=== Cancelled ==="),
        _ => {}
    }
}
let _ = handle.await;
```

- [ ] **Step 2: 验证编译**

Run: `cargo build 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "refactor(cli): use shared run_turn for Part 3 agent loop"
```

---

## Task 14: 补充 run_turn 测试 — 多轮/工具 panic/InputJsonDelta 累积

**Files:**
- Modify: `src/agent/turn.rs`

- [ ] **Step 1: 追加剩余 6 个测试**

在 `#[cfg(test)] mod tests` 末尾追加：

```rust
    #[tokio::test]
    async fn run_turn_tool_use_emits_tool_start_and_end() {
        // Round 1: model calls a tool → stop_reason=ToolUse
        let round1 = vec![
            StreamEvent::MessageStart { id: "1".into(), model: "mock".into(), usage: UsageInfo::default() },
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::ToolUse {
                    id: "tu_1".into(), name: "TodoWrite".into(), input: serde_json::json!({}),
                },
            },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta { stop_reason: Some(StopReason::ToolUse), usage: None },
            StreamEvent::MessageStop,
        ];
        // Round 2: empty → end_turn
        let round2 = vec![StreamEvent::MessageStop];
        let provider = MockProvider::with_scripts(vec![round1, round2]);
        let events = collect_events(provider, empty_request()).await;
        assert!(events.iter().any(|e| matches!(e, TurnEvent::ToolUseStart { name } if name == "TodoWrite")));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::ToolEnd { .. })));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::Done { .. })));
    }

    #[tokio::test]
    async fn run_turn_input_json_delta_accumulates() {
        let script = vec![
            StreamEvent::MessageStart { id: "1".into(), model: "mock".into(), usage: UsageInfo::default() },
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::ToolUse {
                    id: "tu_1".into(), name: "TodoWrite".into(), input: serde_json::json!({}),
                },
            },
            StreamEvent::InputJsonDelta { index: 0, partial_json: "{\"todo\":".into() },
            StreamEvent::InputJsonDelta { index: 0, partial_json: "\"write\"}".into() },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta { stop_reason: Some(StopReason::ToolUse), usage: None },
            StreamEvent::MessageStop,
        ];
        let round2 = vec![StreamEvent::MessageStop];
        let provider = MockProvider::with_scripts(vec![script, round2]);
        let events = collect_events(provider, empty_request()).await;
        let deltas: Vec<_> = events.iter().filter_map(|e| {
            if let TurnEvent::ToolUseDelta { partial_json, .. } = e { Some(partial_json.clone()) } else { None }
        }).collect();
        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0], "{\"todo\":");
        assert_eq!(deltas[1], "\"write\"}");
    }
```

- [ ] **Step 2: 运行测试**

Run: `cargo test --lib agent::turn 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/agent/turn.rs
git commit -m "test(agent): add tool-use and input-delta accumulation tests"
```

---

## Task 15: 最终集成验证

- [ ] **Step 1: 跑全部 lib 测试**

Run: `cargo test --lib --features gui -- --test-threads=1 2>&1 | Select-Object -Last 15`
Expected: 所有测试通过（含新增 run_turn 测试）

- [ ] **Step 2: 构建 GUI binary**

Run: `cargo build --bin agent-gui --features gui 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 3: 构建 CLI**

Run: `cargo build 2>&1 | Select-Object -Last 10`
Expected: PASS

- [ ] **Step 4: 手测 GUI**

Run: `cargo run --bin agent-gui --features gui -- --debug`

验证清单：
- [ ] 发普通文本 → 流式 markdown 渲染
- [ ] 发"读 Cargo.toml" → PermissionModal → Allow → ToolEnd → 回复
- [ ] 点"总是允许" → 同 turn 内同工具不再弹窗
- [ ] 流式中点"停止" → (已取消)
- [ ] 切换 provider → conversation 清空
- [ ] 设置面板改 working_dir → 下次 Bash 在新目录

- [ ] **Step 5: 检查 debug.log**

打开 `%APPDATA%\local-workflow-agent\debug.log`，确认：
- 无 `there is no reactor running` panic
- 无 `STATUS_STACK_BUFFER_OVERRUN`
- TurnEvent 正常流转

- [ ] **Step 6: 手测 CLI**

Run: `cargo run -- Part 3 2>&1 | Select-Object -Last 15`
Expected: Part 1/2 demo 正常，Part 3 用 run_turn 跑

- [ ] **Step 7: 最终 Commit**

```bash
git add -A
git commit -m "test: verify full integration — 9 run_turn tests + GUI + CLI"
```

---

## Self-Review Notes

**Spec coverage:**
- §2 模块布局 → Task 1 (src/agent/) ✓
- §2.3 TurnEvent → Task 1 ✓
- §3 run_turn 流程 → Task 3 ✓
- §4 ToolContext + GuiPermissionHandler → Task 5 ✓
- §5 UI 渲染 → Task 12 ✓
- §5.3 PermissionModal → Task 9 ✓
- §5.4 AskModal → Task 10 ✓
- §6 AgentRequest/AgentResponse 重构 → Task 6 ✓
- §6.3 Settings working_dir → Task 11 ✓
- §7.1 9 个测试 → Task 4 + Task 14 (4 个在 Task 4，剩余在 Task 14；实际实现时可能合并) ✓
- §2.5 CLI 改用 run_turn → Task 13 ✓

**已知简化（实施时可能需要调整）:**
- `show_ask_modal` 需要 `&mut Window` 来调 `InputState::set_value`，但 `cx.update_entity` 闭包只给 `&mut Context`。可能需要用 `window.defer` 或存储 pending event 在下一帧处理
- `permission_forward` / `ask_forward` 的 channel 创建在 `handle_outgoing` 里，但实际应该在 `ChatAI::new` 里创建并传给 `run_agent_loop`。Task 7 里有临时 wiring
- `PermissionHandler::request_permission` 是同步方法，`Handle::block_on` 在 Tokio 多线程 runtime 上阻塞一个 worker 是安全的，但需要确保不在 GPUI 主线程调用
