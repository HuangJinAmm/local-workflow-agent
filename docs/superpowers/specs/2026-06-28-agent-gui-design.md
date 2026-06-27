# Agent GUI — Design Spec

**Date:** 2026-06-28
**Status:** Draft (awaiting user review)
**Target stack:** Rust 2024 edition · `gpui` 0.2.2 · `gpui-component` 0.5.1

---

## 1. Goal & scope

Add a desktop GUI to `local-workflow-agent` that gives the existing CLI agent
a chat-style UX. The GUI is an additional binary (`agent-gui`) — the CLI
demo in `src/main.rs` is preserved unchanged.

The GUI must support:

- Multi-session chat with persistence
- Real-time streaming from Anthropic and OpenAI
- Collapsible thinking chain (Anthropic `thinking` blocks, OpenAI reasoning)
- File/image attachments (drag-drop, picker, paste)
- Full tool-use integration with the existing `tools::all_tools()` registry
- Markdown rendering for assistant output and user input preview

Non-goals (out of scope for this spec):

- Multi-window / tabs across windows
- Voice input
- Mobile/touch-specific layouts
- Plugin system for the GUI itself
- i18n string extraction (interface is English-only in v1)

---

## 2. Architecture

### 2.1 High-level topology

```
src/bin/agent-gui.rs                 Entry: init GPUI, register themes, open window
src/ui/
  app.rs                  AppState            Top-level Entity
  app_view.rs             AppView             Three-pane Root
  session/
    session_list.rs       SessionListView     Left pane
    session_view.rs       SessionView         Middle pane (current session)
    message.rs            MessageView         One per UiMessage
    block.rs              BlockView           text / thinking / tool_use / tool_result
                                                      (Attachments rendered inline at top of MessageView)
  input/
    input_bar.rs          InputBar            Bottom input + attachments
    attachments.rs        AttachmentChip
  settings/
    settings_panel.rs     SettingsPanel       Right drawer
  stream.rs                                 Unified StreamEvent
  provider/
    mod.rs
    anthropic.rs                            AnthropicStreamEvent -> StreamEvent
    openai.rs                               OpenAI chunks -> StreamEvent
  storage.rs                                MessageStore (ui_* tables)
  theme.rs
  markdown.rs
  tool_icons.rs                             name -> IconName map
```

**Boundary rules**

- `ui/*` may import `gpui`, `gpui-component`, and pure-Rust abstractions from
  `api/`, `core/`, `tools/`. It does **not** import `tokio::spawn` directly;
  the runtime is owned by `AppState`.
- `api::provider::LlmProvider` is the only networking surface the UI talks to.
  Concrete `AnthropicStreamEvent` is hidden behind `ui::stream::StreamEvent`.

### 2.2 Entity / async model

GPUI is the front of house; tokio is the back of house. We start **one**
`tokio::runtime::Runtime(worker_threads=4)` inside `AppState::new()` and
hand out `Arc<Runtime>` to anything that needs to talk to the network.

Each `SessionView` runs zero or one turn at a time. A turn is driven by a
single `tokio::spawn` future that:

1. Picks an `LlmProvider` from the `ProviderRegistry` based on the session's
   `provider` setting.
2. Calls `provider.stream(request) -> Pin<Box<dyn Stream<Item=StreamEvent> + Send>>`.
3. Forwards every event back to the UI through `cx.update(|cx| session.update(cx, |s, cx| s.on_event(event, cx)))`.
4. On `MessageStop`, if `stop_reason == "tool_use"`, executes the collected
   `tool_use` blocks via `Tool::execute`, appends the `tool_result` blocks
   to the next request, and loops back to step 2.
5. On `MessageStop` with `end_turn` or any cancel signal, returns.

`inflight: RwLock<HashMap<SessionId, CancellationToken>>` in `AppState` lets
the UI cancel an in-flight turn.

### 2.3 UI state model

```rust
// ui::model
pub struct UiSession {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub provider: String,          // "anthropic" | "openai"
    pub model: String,
    pub messages: Vec<UiMessage>,
}

pub struct UiMessage {
    pub id: String,
    pub role: Role,
    pub blocks: Vec<UiBlock>,
    pub created_at: i64,
}

pub enum UiBlock {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Attachments { items: Vec<Attachment> },
}

pub struct Attachment {
    pub id: String,
    pub kind: AttachmentKind,      // Image | Text | Pdf
    pub display_name: String,
    pub mime: String,
    pub local_path: PathBuf,       // ~/.local-workflow-agent/attachments/<uuid><ext>
    pub size_bytes: u64,
}
```

`SessionView` keeps additional transient state:

```rust
enum TurnPhase {
    Idle,
    Streaming { accumulator: StreamAccumulator, current: UiMessage },
    AwaitingTool { tool_calls: Vec<ToolUse> },
    Cancelling,
}
```

### 2.4 Unified streaming events

```rust
// ui::stream
pub enum StreamEvent {
    MessageStart { id: String, model: String },
    TextDelta { block: usize, text: String },
    ThinkingDelta { block: usize, text: String },
    ToolUseStart { block: usize, id: String, name: String },
    ToolUseDelta { block: usize, partial_json: String },
    MessageStop { stop_reason: String, usage: UsageInfo },
    Error { message: String, retryable: bool },
}
```

Adapters live in `src/ui/provider/` (one per provider) and translate
`AnthropicStreamEvent` / OpenAI `chat.completions` chunks into `StreamEvent`.
This keeps the UI free of provider-specific types and makes future providers
(DeepSeek, Ollama, etc.) a one-file addition.

---

## 3. Data model & persistence

### 3.1 Reuse of `core::SessionStorage`

`SessionStorage` (in `src/core/session_storage.rs`) already owns the
sessions table (id/title/created_at/updated_at/provider/model). The GUI
**does not** add columns or duplicate that table. It adds three new
UI-specific tables in the same SQLite file:

```sql
CREATE TABLE ui_message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    ordinal INTEGER NOT NULL
);
CREATE INDEX idx_ui_message_session ON ui_message(session_id, ordinal);

CREATE TABLE ui_block (
    message_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    kind TEXT NOT NULL,         -- text | thinking | tool_use | tool_result | attachments
    payload BLOB NOT NULL,      -- bincode-encoded JSON
    PRIMARY KEY(message_id, ordinal)
);

CREATE TABLE ui_attachment_ref (
    attachment_id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    local_path TEXT NOT NULL
);
```

`MessageStore` is the `ui/storage.rs` wrapper.

### 3.2 Attachments on disk

- Physical path: `~/.local-workflow-agent/attachments/<uuid><ext>`.
- DB stores only the path (never base64).
- Limits: images ≤ 5 MB, text/PDF ≤ 50 MB.
- Sweep: at startup, list `attachments/`, delete files not referenced in
  `ui_attachment_ref`. Runs in a background task; failure is logged, not fatal.

### 3.3 Settings storage

- Use `gpui-component`'s `Preferences` API. File: `settings.json` under the
  app's standard config dir.
- API keys are stored **in plaintext** in `settings.json` (per user decision
  in brainstorming §2). No `keyring` dependency.
- Settings keys: `theme`, `anthropic_api_key`, `openai_api_key`,
  `default_provider`, `default_model`, `thinking_budget_tokens`,
  `tool_policy`.

### 3.4 Loading flow

1. `AppState::new()` opens the SQLite DB, creates `ui_*` tables if missing,
   opens `SessionStorage`, reads `Preferences`, registers themes.
2. `SessionListView` calls `storage.list_sessions()` to render the left pane.
3. On session click: `app.load_session(id)` → reads `ui_message` /
   `ui_block` rows → hands them to a new `SessionView` entity.

---

## 4. Streaming & async bridge

### 4.1 Channel topology

```
tokio task ──mpsc::Sender(StreamEvent)──▶ cx.update ──▶ SessionView::on_event
```

- Channel capacity: 256. Backpressure is automatic; the provider side slows
  down naturally.
- Multiple `cx.notify()` calls within a frame are coalesced by GPUI.

### 4.2 Event-to-block mapping

`SessionView::on_event` uses `StreamAccumulator` to track the current
`block_index` and dispatches:

- `TextDelta(block, text)` → push to `UiBlock::Text { text }` at `block`.
- `ThinkingDelta(block, text)` → push to `UiBlock::Thinking { thinking, signature: None }`.
- `ToolUseStart / ToolUseDelta` → create or extend `UiBlock::ToolUse` at
  `block`; the input JSON is accumulated in a private buffer and parsed on
  `MessageStop` (fall back to `raw` on parse failure).
- `MessageStop` → finalize the current `UiMessage`, write to SQLite, and
  if `stop_reason == "tool_use"`, run tools then loop.

### 4.3 Cancellation

- **Soft cancel** (⏹ button or `Esc` when not in input): set the
  `CancellationToken`; the tokio future drops its stream and returns; UI
  marks the partial message `[cancelled]` and persists it.
- **Hard cancel** (Shift+⏹ or context menu "Discard turn"): same as soft
  cancel plus the partial message is hidden from view (DB row stays, with a
  flag for "show discarded").

### 4.4 Markdown rendering

`UiBlock::Text` is rendered with `gpui_component::markdown::Markdown`.
Streaming text is split into "stable" (rendered) and "tail" (plain text)
portions at the last line break, so the markdown tree isn't rebuilt on
every character. `Thinking` blocks are also rendered with `Markdown` so
code blocks inside thinking are formatted consistently.

### 4.5 Error handling

- **Network errors** → `StreamEvent::Error { retryable }` → UI marks the
  message red, shows a "Retry" button that re-runs the same turn.
- **Provider 4xx** → turn ends; modal `Dialog` shows the error with a
  "Copy" button. 401 includes a "Open settings" button.
- **Tool errors** → `UiBlock::ToolResult { is_error: true }` with red badge;
  the agent loop continues so the LLM can react.

---

## 5. Thinking chain & attachments

### 5.1 Thinking block UX

- `BlockView` holds `collapsed: bool` in memory only (not persisted).
- Collapsed: header shows `▶ Thinking · N chars` plus a spinner while
  streaming.
- Expanded: header becomes `▼ Thinking · N chars`, body shows the
  accumulated `Markdown`.
- `code_theme` matches the assistant message theme.

### 5.2 Attachments

Three input methods (all implemented):

1. **Drag-and-drop** onto `InputBar` (GPUI `WindowEvent::DroppedFiles`).
2. **📎 button** → system file picker via `rfd` crate.
3. **Paste** from clipboard (including images, via `Pasting` event).

Pipeline per file:

1. Guess mime via `mime_guess` (already a dependency).
2. Classify: `Image` for `image/*`, `Pdf` for `application/pdf`, else
   `Text` (`.txt`, `.md`, `.rs`, etc., checked against an allowlist).
3. Refuse if larger than the per-kind limit; show a `Notification`.
4. Copy to `~/.local-workflow-agent/attachments/<uuid><ext>`.
5. Return `Attachment`.

Rendering in the message stream:

- Images: `img(path)` element at 64 px height, rounded, with a tooltip
  showing the full filename.
- Text/PDF: chip with a file-type icon and the display name.

Sending to providers:

- **Anthropic**: image → `image` block (base64); PDF → `document` block
  (base64); text → `text` block with the contents.
- **OpenAI**: image → `image_url` with data URL; PDF → `input_file`
  (Responses API) or `text`; text → `text`.

---

## 6. Tool use

### 6.1 Block layout

Each `ToolUse` + its eventual `ToolResult` share a `ToolPair` view:

```
┌─ ⏵ bash · 1.2s · [ok] ──────────────────── v ─┐
│ $ ls -la                                      │
│ ...                                           │
└───────────────────────────────────────────────┘
```

- Header: tool icon (per `tool_icons.rs`), name, status badge
  (`running` / `ok` / `error`), elapsed time, chevron.
- Body (when expanded): JSON-highlighted `input`, scrollable `result` with
  a "show all" toggle for long output. Bash output uses a terminal theme.
- Streaming input: badge says `parsing` and the body shows a small dots
  animation. Expansion is disabled until `MessageStop`.

### 6.2 Tool policy

`AppState::tool_policy: ToolPolicy`:

```rust
pub struct ToolPolicy {
    pub enabled: HashSet<String>,                // empty = all enabled
    pub require_confirmation: HashSet<String>,
    pub working_dir: PathBuf,
}
```

Defaults: `enabled = {}` (all allowed); `require_confirmation =
{ "bash", "powershell", "file_write", "file_edit", "apply_patch" }`.

`SettingsPanel` renders one row per tool from `all_tools()` with toggles
for "enabled" and "ask before running".

Confirmation UX: modal `Dialog` with the tool name, JSON-highlighted
input, and three buttons: "Allow once", "Allow for session", "Deny".

### 6.3 Performance

A typical tool loop produces 1–10 blocks per turn; a worst-case long
session may have 50+ blocks per message. `MessageView` uses
`gpui_component::uniform_list` for virtualization; each block is a small
`Entity<UiBlockState>` so single-block updates don't re-render siblings.

---

## 7. Layout, theme, shortcuts

### 7.1 Three-pane layout

```
┌──────────┬─────────────────────────────┬──────────────┐
│ Sessions │           Chat              │   Settings   │
│ (left)   │           (middle)          │   (right,    │
│          │                             │   drawer)    │
│          │  ┌─────────────────────┐    │              │
│          │  │ MessageView …       │    │              │
│          │  └─────────────────────┘    │              │
│          │  …                          │              │
│          │  ┌─────────────────────┐    │              │
│          │  │ InputBar            │    │              │
│          │  └─────────────────────┘    │              │
└──────────┴─────────────────────────────┴──────────────┘
```

- Left pane: `SessionListView` — list of sessions with title + relative
  timestamp; "+" button to create a new session; right-click for
  rename/delete.
- Middle pane: `SessionView` — scrollable message list (top-down), then a
  sticky `InputBar` at the bottom. Above the input: attachment chips.
- Right pane: a slide-in `SettingsPanel` (drawer). Hidden by default;
  toggled by `Cmd/Ctrl+,` or the gear icon.

### 7.2 Theme

Three modes: `light`, `dark`, `system`. The default theme is `system`.
In `system` mode the app watches `window.appearance()` and re-themes
immediately on change. Themes are `gpui_component::theme::Theme` instances
inserted into `ThemeRegistry` at startup.

All colors are referenced via `theme::foreground`, `theme::muted`,
`theme::accent`, `theme::border`, `theme::danger`, etc. No hard-coded RGB.

### 7.3 Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Enter` (in input) | Send |
| `Shift+Enter` | Newline |
| `Cmd/Ctrl+K` | Focus input |
| `Cmd/Ctrl+Shift+K` | Focus session list (type to filter) |
| `Cmd/Ctrl+N` | New session |
| `Cmd/Ctrl+B` | Toggle session list |
| `Cmd/Ctrl+,` | Toggle settings drawer |
| `Cmd/Ctrl+T` | Cycle theme (light → dark → system) |
| `Esc` (focus not in input) | Soft cancel current turn |
| `Cmd/Ctrl+R` (focus on assistant) | Retry that turn |
| `Cmd/Ctrl+L` | Clear current session (confirm) |
| `Cmd/Ctrl+S` | Flush SQLite |

---

## 8. Cargo changes

```toml
[features]
gui = ["dep:gpui", "dep:gpui-component"]

[dependencies]
gpui = { version = "0.2.2", optional = true }
gpui-component = { version = "0.5.1", optional = true, features = ["markdown"] }
rfd = { version = "0.14", optional = true, default-features = false, features = ["xdg-portal", "tokio"] }
bincode = "1"
```

Run with `cargo run --bin agent-gui --features gui`. The default
`cargo build` does **not** pull GPUI, so CI for the CLI stays lean.

---

## 9. Testing strategy

- **Unit tests** for:
  - `StreamAccumulator` (already tested) — keep passing.
  - `MessageStore` (CRUD + sweep).
  - Stream event adapter per provider (Anthropic, OpenAI) using
    captured fixture SSE bytes.
- **Integration tests** for the turn loop: spawn a fake `LlmProvider`
  returning a scripted `StreamEvent` sequence; assert that
  `SessionView` produces the expected `UiMessage` sequence.
- **Manual tests** documented in `docs/manual-test.md`:
  - Drag-drop an image; verify it appears in the next assistant turn.
  - Paste a screenshot.
  - Enable thinking on a Claude 4.6 model; verify the chain expands.
  - Confirm a `bash` tool call.
  - Soft cancel a streaming response; verify partial content persists.
  - Hard cancel; verify the message is hidden.
  - Switch to OpenAI gpt-4o; verify attachments still render.

---

## 10. Open questions

None at design time. All scope decisions resolved during brainstorming
(see commit history for the chat log).
