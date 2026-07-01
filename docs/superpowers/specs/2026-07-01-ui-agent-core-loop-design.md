# UI Agent 核心交互闭环重构设计

- **日期**: 2026-07-01
- **范围**: A 簇（核心交互闭环）—— 首期子项目
- **状态**: 设计已获批，待 spec 自审 + 用户审阅

## 1. 背景与动机

当前 `src/ui/services/agent/` 走的是最窄路径：

- **非流式** `create_message`：模型每说一个字都得等整段返回
- **零工具**：`tools = vec![]`，模型一旦返回 `ToolUse`，UI 只把它渲染成 `🔧 Tool call: ...` 文本就结束
- **无会话持久化**：关窗即丢（C 簇关注点，首期不接）
- **手写 provider match**：没用 `ProviderRegistry` / `ModelRegistry`（B 簇关注点）
- **绕过 transformer**：直接调 provider，丢掉 `AnthropicTransformer` / `OpenAiChatTransformer`（B 簇）
- **错误当字符串**：`ProviderError` 的 10 种分类、`is_context_overflow`、`RetryConfig` 全没用（B 簇）
- **系统提示无 cache_control**、**无 ThinkingConfig**（F 簇）
- 库里 13 个 `ContentBlock` 变体，UI 只识别 7 个，其余 6 个全归为 `[unsupported]`（F 簇）

项目库已存在但未接入的能力可归为 7 簇（A 核心闭环 / B Provider-Model 智能 / C 持久化 / D 扩展工具生态 / E 上下文治理 / F 格式覆盖 / G stub 死代码）。本设计只覆盖 **A 簇**，B/C/D/E/F 后续各自成期。G 簇（OAuth stub / SseStreamParser stub / 孤立工具）不接。

**A 簇目标**：让 GUI agent 真正能干活——流式响应、tool-use 闭环、21 个内置工具、GUI 权限弹窗、AskUserQuestion 弹窗、可配置工作目录。

## 2. 模块布局与架构

### 2.1 新增共享模块 `src/agent/`

库级模块，CLI 与 UI 共用，避免 turn 循环逻辑分叉。

```
src/agent/
├── mod.rs           // pub re-exports: run_turn, TurnEvent, TurnSink, TurnCancel
├── turn.rs          // run_turn + TurnEvent + MAX_TOOL_ROUNDS=16
├── mock_provider.rs // 测试用 MockProvider（with_scripts 弹脚本）
└── (无其他)
```

### 2.2 `run_turn` 签名

```rust
pub async fn run_turn(
    provider: Arc<dyn LlmProvider>,
    session_id: String,
    request: ProviderRequest,        // 已含历史 messages + system + tools
    tools: Arc<Vec<Box<dyn Tool>>>,
    tool_ctx: Arc<ToolContext>,
    sink: TurnSink,                  // Sender<TurnEvent> 或等价 wrapper
    cancel: TurnCancel,              // CancellationToken
) -> Result<(), ClaudeError>
```

### 2.3 `TurnEvent` 枚举

```rust
pub enum TurnEvent {
    TextDelta { text: String },
    ToolUseStart { id: String, name: String },
    ToolUseDelta { id: String, partial_json: String },
    ToolEnd { id: String, result: ToolResultContent, is_error: bool },
    Done { stop_reason: StopReason, usage: UsageInfo },
    Failed { error: ClaudeError },
    Cancelled,
}
```

- `ThinkingDelta` 也作为 `TextDelta` emit（让 UI 显示思考链）
- `MAX_TOOL_ROUNDS = 16`，超限 emit `Failed`
- 流式走 `provider.create_message_stream`，按 content-block index 累积 text / tool_use 块

### 2.4 UI 侧改动总览

| 文件 | 改动 |
|---|---|
| `src/ui/services/agent/client.rs` | `Agent::chat_step` 不再自己调 provider；改为构造 `ProviderRequest` 后调 `run_turn`，把 sink 事件流转成 `AgentResponse` |
| `src/ui/services/agent/messages.rs` | `AgentResponse` 改为 `TurnEvent(TurnEvent)` / `PermissionRequest` / `UserQuestion` / `Error`；移除 `TextResponse` / `ToolCallRequest` / `is_done`；`AgentRequest` 移除 `ToolResults`，新增 `Cancel` / `SetWorkingDir` |
| `src/ui/services/agent/permission_handler.rs`（新） | `GuiPermissionHandler` 实现 `PermissionHandler`，持 channel 端，`request_permission` 阻塞在 `response_rx.recv()` |
| `src/ui/handler.rs` | `run_agent_loop` 构造 `ToolContext`（working_dir / `GuiPermissionHandler` / 空 `McpManager` / `ask_tx`）并调 `run_turn`；处理 `Cancel` / `SetWorkingDir` |
| `src/ui/permission_modal.rs`（新） | GUI 模态框：tool_name + input JSON + Allow/Deny/AlwaysAllow 按钮 |
| `src/ui/ask_modal.rs`（新） | AskUserQuestion 模态框：question + options 按钮 / 自由输入 |
| `src/ui/chat.rs` | 订阅 `TurnEvent`：TextDelta 追加 + 50ms 节流 markdown 重渲染；ToolUseStart/Delta/End 更新工具块；Done 强制重渲染 + 清 loading；Failed/Cancelled 处理；"停止"按钮触发 cancel |
| `src/ui/settings.rs` + `settings_panel.rs` | `Settings` 加 `working_dir: PathBuf`（默认 `dirs::home_dir()`）；面板加 Working Directory 行 + Browse 按钮 |

### 2.5 CLI 侧改动

`src/main.rs` Part 3 的内联 `run_agent_loop` 改为调 `run_turn`（用 `AutoPermissionHandler`、stdout sink 打印事件）。Part 1/2 demo 行为不变。这是附带收益，避免分叉。

## 3. run_turn 内部流程

### 3.1 状态机

```
run_turn(provider, session_id, request, tools, tool_ctx, sink, cancel)
│
├─ for round in 1..=MAX_TOOL_ROUNDS(16):
│    │
│    ├─ check cancel; if cancelled → emit Cancelled, return Ok
│    │
│    ├─ provider.create_message_stream(request, &cancel)
│    │    │
│    │    └─ while let Some(event) = stream.next().await:
│    │         │
│    │         ├─ MessageStart { model, .. }    → no-op
│    │         ├─ ContentBlockStart { index, content_block }:
│    │         │    Text { text }              → emit TextDelta { text }
│    │         │    ToolUse { id, name, .. }   → emit ToolUseStart { id, name }
│    │         │                                            + init input_buffer[id]=""
│    │         │    Thinking { thinking, .. }  → 非空时 emit TextDelta { thinking }
│    │         │    其他                       → ignore
│    │         ├─ ContentBlockDelta { index, delta }:
│    │         │    TextDelta { text }              → emit TextDelta { text }
│    │         │    InputJsonDelta { partial_json }  → input_buffer[id].push_str
│    │         │                                          re-parse into block.input
│    │         │                                          emit ToolUseDelta { id, partial_json }
│    │         │    ThinkingDelta { thinking }       → emit TextDelta { thinking }
│    │         │    SignatureDelta { .. }            → ignore
│    │         ├─ ContentBlockStop { index }         → no-op
│    │         ├─ MessageDelta { stop_reason, usage } → 记录
│    │         ├─ MessageStop                        → break inner while
│    │         └─ Error { error_type, message }      → emit Failed, return Err
│    │
│    ├─ 组装本轮 assistant Message（blocks = 已累积的 text/tool_use/thinking 块）
│    │   推入 request.messages
│    │
│    ├─ if stop_reason != "tool_use":
│    │    emit Done { stop_reason, usage }
│    │    return Ok
│    │
│    ├─ 工具执行阶段（对所有 tool_use 块）:
│    │    │
│    │    ├─ find_tool(name, &tools) → 未找到:
│    │    │     构造 error ToolResult
│    │    │     emit ToolEnd { id, result: Text(err), is_error: true }
│    │    │     skip execute
│    │    │
│    │    ├─ check cancel before each tool; if cancelled:
│    │    │     emit ToolEnd { id, result: Text("cancelled"), is_error: true }
│    │    │     skip this tool
│    │    │
│    │    ├─ tool_ctx.permission_handler.request_permission(...)
│    │    │   None/ReadOnly → Allow
│    │    │   Write/Execute/Dangerous → 抛 PermissionRequest 给 GUI
│    │    │     阻塞在 response_rx.recv()
│    │    │     Deny → 构造 error ToolResult，emit ToolEnd { is_error: true }
│    │    │
│    │    ├─ tool.execute(input, tool_ctx).await
│    │    │   （panic 用 AssertUnwindSafe + catch_unwind 兜底）
│    │    │
│    │    ├─ emit ToolEnd { id, result: tool_result.content.clone(), is_error }
│    │    │
│    │    └─ 构造 ToolResult ContentBlock，推入 user_role 消息块
│    │
│    ├─ 把本轮所有 ToolResult 装成一个 User 角色消息推入 request.messages
│    │
│    └─ 继续下一轮
│
└─ MAX_TOOL_ROUNDS 仍未 end_turn:
     emit Failed { ClaudeError::Other("max tool rounds exceeded") }
     return Err
```

### 3.2 关键状态

- `input_buffer: HashMap<String, String>` — 缓冲每个 tool_use 块的原始 partial JSON，每个 delta 重 parse
- `text_accum: String` — 累积当前轮的 text deltas
- `current_blocks: Vec<ContentBlock>` — 按 index 累积的所有 block
- `stop_reason: Option<StopReason>` / `usage: Option<UsageInfo>` — 从 MessageDelta 拿到
- `current_tool_results: Vec<(String, ContentBlock)>` — 本轮 tool_use_id → ToolResult 块

### 3.3 Cancellation

- `TurnCancel` = `tokio_util::sync::CancellationToken`
- 检查点：(1) 每轮开始前、(2) 每次工具执行前、(3) 流读取 `stream.next().await` 之间
- GUI "停止" 按钮触发 cancel，turn 循环在最近的检查点 emit `Cancelled` 后退出
- 当前正在执行的 provider HTTP 调用会继续到完成（首期不取消 HTTP），但工具结果不回灌、不开始下一轮。硬中断留到 B 簇接 stream_parser 时

### 3.4 错误处理（首期最小）

- provider 调用失败 → emit `Failed { ClaudeError }`，return Err
- 单个工具 panic 用 `AssertUnwindSafe + catch_unwind` 兜底，转 ToolEnd { is_error: true }，turn 继续
- 单个工具错误不终止 turn，继续执行剩余工具并把 error 作为 ToolResult 回灌

## 4. ToolContext 构造与权限/AskUser 通道

### 4.1 ToolContext 装配

```rust
fn build_tool_context(
    working_dir: PathBuf,
    permission_tx: Sender<PermissionRequest>,
    permission_rx: Receiver<PermissionResponse>,
    ask_tx: Sender<UserQuestionEvent>,
    session_id: String,
) -> ToolContext {
    let permission_handler = Arc::new(GuiPermissionHandler::new(permission_tx, permission_rx));

    ToolContext {
        working_dir,
        permission_mode: PermissionMode::Default,
        permission_handler,
        cost_tracker: Arc::new(CostTracker::new()),
        session_id,
        current_turn: 0,
        file_history: Arc::new(FileHistory::new()),
        lsp_manager: None,
        non_interactive: false,
        mcp_manager: Arc::new(McpManager::new()),  // 空 manager
        config: Arc::new(Config::default()),
        managed_agent_config: None,
        completion_notifier: None,
        pending_permissions: Arc::new(Mutex::new(HashMap::new())),
        permission_manager: None,
        user_question_tx: ask_tx,
    }
}
```

### 4.2 GuiPermissionHandler

```rust
pub struct GuiPermissionHandler {
    request_tx: Sender<PermissionRequest>,
    response_rx: Receiver<PermissionResponse>,
    always_allow: Mutex<HashSet<String>>,  // turn 级缓存
}

impl PermissionHandler for GuiPermissionHandler {
    fn check_permission(&self, tool_name: &str, level: PermissionLevel) -> PermissionDecision {
        if matches!(level, PermissionLevel::None | PermissionLevel::ReadOnly) {
            return PermissionDecision::Allow;
        }
        if self.always_allow.lock().unwrap().contains(tool_name) {
            return PermissionDecision::Allow;
        }
        PermissionDecision::Ask
    }

    async fn request_permission(
        &self, tool_name: String, input: Value, level: PermissionLevel,
    ) -> PermissionResponse {
        // 生成 id, send PermissionRequest, 阻塞 recv PermissionResponse。
        // 若 decision == AlwaysAllow，把 tool_name 加入 always_allow 缓存
        // （后续同工具直接 Allow，不再弹窗）。
    }
}
```

### 4.3 PermissionRequest / PermissionResponse

```rust
pub struct PermissionRequest {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
    pub level: PermissionLevel,
}
pub struct PermissionResponse {
    pub id: String,
    pub decision: PermissionDecision,  // Allow / Deny / AlwaysAllow
}
```

### 4.4 AskUserQuestion 通道

复用库现有 `UserQuestionEvent { question, options, reply_tx: oneshot::Sender<String> }`，不新建消息类型。

- `ToolContext.user_question_tx = ask_tx`
- AskUserQuestion 工具调用时 `tx.send(event)`
- `run_agent_loop` 持 `ask_rx`，收到 event → 转成 `AgentResponse::UserQuestion(event)` 推给前台
- 前台弹 `AskModal`，用户答完通过 `reply_tx` 回传字符串
- 整个 turn 循环阻塞在 `reply_rx.await` 上

### 4.5 通道拓扑

```
background run_agent_loop
  │
  ├── run_turn(provider, ..., tools, tool_ctx, sink, cancel)
  │     │
  │     ├── provider.create_message_stream → stream events → emit TurnEvent via sink
  │     │
  │     ├── GuiPermissionHandler.request_permission
  │     │     └── permission_tx.send(PermissionRequest) ─┐
  │     │                                                   │
  │     └── AskUserQuestion tool execute                   │
  │           └── user_question_tx.send(UserQuestionEvent) ─┤
  │                                                         │
  │   sink (TurnEvent) ─────────────────────────────────── ─┤
  │                                                         │
  └── response_tx (AgentResponse) ─────────────────────── ──┤
                                                            ▼
                          ┌─────────────────────────────────────────┐
                          │  GPUI foreground (ChatAI view)         │
                          │                                         │
                          │  • TurnEvent → render text / tool block │
                          │  • PermissionRequest → PermissionModal  │
                          │  • UserQuestion → AskModal             │
                          │                                         │
                          │  PermissionResponse ──▶ permission_rx  │
                          │  answer string ──▶ reply_tx (oneshot)   │
                          └─────────────────────────────────────────┘
```

### 4.6 关键约定

- **AlwaysAllow 缓存**：turn 级，重启 agent 重置
- **McpManager 首期空载**：`McpManager::new()` 无 server，`all_tool_definitions()` 返回空，`Skill` / `ListMcpResources` / `ReadMcpResource` 工具调用返回友好错误，不阻塞 turn
- **Task\* subagent 工具**：复用同一个 `ToolContext`，subagent 内部用同一 `provider`（由 `TaskCreate` 工具内部实现决定，不在 run_turn 范围内做特殊处理），按现有逻辑跑，GUI 不专门做 subagent UI（输出作为 tool result 回传主 turn）

## 5. UI 渲染与事件流

### 5.1 ChatAI 状态字段扩展

```rust
pub struct ChatAI {
    // 既有字段...
    text_input, message_state, list_state, request_tx,
    attached_files, is_loading, has_api_key,
    settings, show_settings, settings_panel,

    // 新增
    streaming_text: String,
    last_render_at: Option<std::time::Instant>,
    pending_tools: HashMap<String, PendingTool>,
    cancel_tx: Arc<tokio::sync::Notify>,
}

struct PendingTool {
    name: String,
    input_buffer: String,
}
```

### 5.2 TurnEvent → UI 行为

| TurnEvent | UI 行为 |
|---|---|
| `TextDelta { text }` | `streaming_text.push_str(text)`；若 `now - last_render_at > 50ms`，重渲染；MessageStop 后强制重渲染一次 |
| `ToolUseStart { id, name }` | 追加 "🔧 调用工具: `{name}`…" 提示块；`pending_tools.insert(id, ...)` |
| `ToolUseDelta { id, partial_json }` | `pending_tools[id].input_buffer.push_str(partial_json)`；实时更新（50ms 节流） |
| `ToolEnd { id, result, is_error }` | 替换为完成状态：`✓ {name}` 或 `✗ {name} (error)`；新起 `Role::ToolResult` 消息；`pending_tools.remove(id)` |
| `Done { stop_reason, usage }` | 强制重渲染当前 assistant 消息；清 `streaming_text` / `pending_tools`；`set_loading(false)` |
| `Failed { error }` | 推 `UiMessage::error(...)`；清状态；`set_loading(false)` |
| `Cancelled` | `streaming_text` 末尾追加 `（已取消）`；清 `pending_tools`；`set_loading(false)` |

### 5.3 PermissionModal

- 订阅 `AgentResponse::PermissionRequest`
- 内容：tool_name 标题（Execute 级别标红）+ input 的 pretty JSON + Allow/Deny/AlwaysAllow 按钮
- **dangerous 级别隐藏 AlwaysAllow 按钮**（避免误信危险工具）
- 同时只显示一个 modal；当前 modal 处理完后从 channel 取下一个

### 5.4 AskModal

- 订阅 `AgentResponse::UserQuestion`
- 内容：question 标题 + options 按钮列表（若有）或自由输入 Input（若无）
- 同一时刻一个

### 5.5 停止按钮

- `is_loading=true` 时发送按钮变为"停止"图标按钮
- 点击 → `cancel_tx.notify_waiters()`
- 软取消：完成当前 HTTP，不开始下一轮；硬中断留到 B 簇

### 5.6 消息角色扩展

- `MessageRole::ToolCall`：渲染为 "🔧 调用 `{name}`" 灰色块（进行中）
- `MessageRole::ToolResult`：渲染为 "✓ {name} 返回" 灰色块 + 折叠 result
- `MessageRole::System`：现有行为（assistant 一样 markdown）

## 6. AgentRequest / AgentResponse 重构

### 6.1 AgentRequest

```rust
pub enum AgentRequest {
    Chat { content: String, files: Vec<PathBuf> },
    Cancel,                              // 新
    ClearHistory,
    SetProvider(String),
    SetModel(String),
    SetApiKey(String),
    SetBaseUrl(String),
    SetApiConfig { api_key: String, base_url: String },
    SetWorkingDir(PathBuf),              // 新
}
// 移除 ToolResults（run_turn 内部自闭环）
```

### 6.2 AgentResponse

```rust
pub enum AgentResponse {
    TurnEvent(TurnEvent),
    PermissionRequest { id, tool_name, input, level },
    UserQuestion(UserQuestionEvent),
    Error(String),
}
// 移除 TextResponse / ToolCallRequest / is_done（被 TurnEvent::Done 取代）
```

### 6.3 Settings 扩展

```rust
pub struct Settings {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub working_dir: PathBuf,  // 新，默认 dirs::home_dir()
}
```

### 6.4 SettingsPanel 扩展

新增 Working Directory 行：`InputState` 显示路径 + `Browse...` 按钮（触发 `PathPromptOptions`）

### 6.5 启动序列

1. load settings
2. 检查 has_api_key
3. 创建 channels: request/response, permission request/response, ask
4. spawn `run_agent_loop`（传 permission/ask channels）
5. spawn `handle_incoming`（订阅 response_rx → 转发到 ChatAI entity）
6. 推送初始配置: SetProvider / SetApiConfig / SetModel / SetWorkingDir

### 6.6 文件上传

`upload_file` 首期保留现状，但只在 provider==anthropic 时调用；其他 provider 把文件路径作为文本提示附在消息里。

## 7. 测试策略与验证

### 7.1 库级单元测试（`src/agent/turn.rs` + `src/agent/mock_provider.rs`）

`MockProvider::with_scripts(Vec<Vec<StreamEvent>>)` 每次 `create_message_stream` 弹一个脚本；空队列返回空流（→ end_turn）。

| 用例 | 验证点 |
|---|---|
| `run_turn_text_only_emits_done` | 单轮 TextDelta → Done，无工具 |
| `run_turn_tool_use_emits_tool_start_and_end` | 单轮 ToolUseStart → ToolEnd → 下一轮 Done |
| `run_turn_unknown_tool_emits_error_tool_end` | 未知工具 → ToolEnd { is_error: true } → Done |
| `run_turn_multi_round_tool_use` | 2-3 轮 tool_use 后 end_turn |
| `run_turn_max_rounds_exceeded_emits_failed` | 16 轮全 tool_use → Failed |
| `run_turn_cancellation_emits_cancelled` | 触发 cancel → Cancelled |
| `run_turn_tool_panic_caught` | 工具 panic → ToolEnd { is_error: true }，turn 继续 |
| `run_turn_input_json_delta_accumulates` | 多个 InputJsonDelta → block.input 完整 |
| `run_turn_parallel_tools_same_round` | 同轮 2 个 tool_use → 2 个 ToolStart + 2 个 ToolEnd |

### 7.2 UI 端验证（手测清单）

- [ ] 发普通文本 → 流式 markdown + 50ms 节流
- [ ] 发"读 Cargo.toml" → Read → PermissionModal（Write/Execute 级）→ Allow → ToolEnd → 模型继续 → Done
- [ ] 点"总是允许" → 同 turn 内同工具不再弹窗
- [ ] AskUserQuestion 被调 → AskModal → 输入答案 → 回复被工具收到
- [ ] 流式中点"停止" → 软取消，UI 显示 (已取消)
- [ ] 切换 provider → conversation 清空
- [ ] 设置面板改 working_dir → 下次 Bash 工具在新目录执行
- [ ] 非 anthropic provider 下附件 → 文本路径提示

### 7.3 回归保护

- 现有 373 个测试不能挂
- `src/main.rs` Part 3 改用 `run_turn` 后，Part 1/2 行为不变

### 7.4 最终验证清单

```
[ ] cargo test --lib 全过（含新增 9 个 run_turn 测试）
[ ] cargo build --bin agent-gui --features gui 通过
[ ] cargo run --bin agent-gui --features gui -- --debug 手测 8 场景
[ ] debug.log 无 panic / no reactor 错误
[ ] CLI: cargo run -- Part 3 仍能跑
```

## 8. 范围边界

### 8.1 本期做（A 簇）

- 共享 `src/agent/turn.rs`（流式 + tool-use 闭环）
- 21 个内置工具注册 + ToolContext 装配
- GuiPermissionHandler + PermissionModal
- AskUserQuestion 通道 + AskModal
- Settings 加 working_dir
- run_turn 9 个单元测试
- CLI Part 3 改用 run_turn

### 8.2 本期不做（后续簇）

- **B 簇**：ProviderRegistry / ModelRegistry / transformer / ProviderError 分类重试 / RetryConfig
- **C 簇**：SqliteSessionStore / JSONL transcript / Attachment / memdir memory
- **D 簇**：MCP 实际连接 / Skills 发现 / 附件本地落盘
- **E 簇**：token_budget / truncate / compact / CostTracker 接入 UI
- **F 簇**：ContentBlock 全 13 变体渲染 / SystemPrompt::Blocks cache_control / ThinkingConfig UI 开关
- **G 簇**：OAuth / SseStreamParser stub / 孤立工具（Cron/LSP/computer_use 等不在 all_tools() 中的）

### 8.3 已知妥协

- McpManager 首期空载，`Skill` / `ListMcpResources` / `ReadMcpResource` 调用返回友好错误
- HTTP 不可硬中断（B 簇接 stream_parser 后补）
- 当前非 anthropic provider 下附件只作为文本路径提示
