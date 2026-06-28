# local-workflow-agent

A Rust agent framework for local workflows, with a chat-style desktop GUI on
top.

## What is in the box

- **Library** (`local_workflow_agent::...`) — wraps Anthropic and OpenAI
  (plus OpenAI-compatible) providers behind an `LlmProvider` trait, a
  `tools` registry, persistent SQLite storage, and an MCP client.
- **Non-GUI demo** (`cargo run`) — walks through three layers: direct
  tool invocation, building tool definitions, and an LLM-driven agent
  loop. Parts 1 + 2 work without an API key.
- **GUI** (`cargo run --bin agent-gui --features gui`) — three-pane
  desktop app (session list + chat + settings) on `gpui` 0.2.2 +
  `gpui-component` 0.5.1, with streaming, persistent sessions,
  attachments, markdown rendering, theme switch, and a tool-use loop.

## Status

GUI features shipped so far: history click-load, InputBar with
Enter-to-send, drag-and-drop + clipboard paste, theme switch
(Light / Dark / System), settings panel for provider / model / API
key, markdown rendering, tool-use loop (executes tool calls and
continues the turn until `end_turn`, `cancel`, or
`MAX_TOOL_ROUNDS = 16`).

`cargo test --features gui` — **422 / 422 passing** across the lib and
integration suites.

## Requirements

- Rust 2024 edition.
- Linux + Wayland (`xdg-portal`) recommended for the GUI's drag-and-drop
  and file dialogs. The GUI also builds on Windows / macOS, falling back
  to the native dialog backend there.
- An Anthropic or OpenAI API key to run the agent loop end-to-end.

## Build & run

```bash
# Non-GUI demo (parts 1 + 2 work without an API key)
cargo run

# GUI
cargo run --bin agent-gui --features gui

# All tests
cargo test --features gui
```

## Configuration

| Env var              | Effect                                                                       |
|----------------------|------------------------------------------------------------------------------|
| `LWA_DATA_DIR`       | Override the data directory.                                                 |
| `ANTHROPIC_API_KEY`  | Anthropic credential. Set in the GUI's Settings panel, or here.              |
| `OPENAI_API_KEY`     | Same, for the OpenAI provider.                                               |

Default data directory:

- **Library** — `~/.local-workflow-agent/`
- **GUI binary** — project-local `./.lwa-data/` (so the app works in
  sandboxed environments without writing to the home directory)
- **Both** — `LWA_DATA_DIR` wins.

API keys are stored in plaintext in `<data-dir>/settings.json`. Session
history lives in `<data-dir>/sessions.sqlite`; attachments are written
under `<data-dir>/attachments/`.

## Project layout

```
src/
  api/             # LlmProvider trait, Anthropic + OpenAI clients,
                   #   request/response types, model registry
  core/            # Config, cost tracker, file history, permissions,
                   #   SQLite store, auth, system prompt, token budget
  tools/           # Tool trait + built-in tools (bash, file_read /
                   #   write / edit, glob, grep, web_fetch, web_search,
                   #   apply_patch, todo_write, …)
  mcp/             # MCP client + registry
  query/           # Context compaction, session memory, command queue
  ui/              # GPUI app (gated behind the `gui` feature)
    input/         # InputBar + attachment ingest
    session/       # SessionListView, SessionView, tool rendering
    settings/      # Settings panel + persistence
    provider/      # Unified stream translator
    test_support/  # MockProvider, scripted per-call events
  bin/agent-gui.rs # GUI entry point
  main.rs          # Non-GUI demo entry point
tests/             # Integration tests
docs/superpowers/  # Design spec + implementation plan
```

## GUI key bindings

| Key            | Action                  |
|----------------|-------------------------|
| `Cmd/Ctrl+N`   | New session             |
| `Cmd/Ctrl+T`   | Toggle theme            |
| `Cmd/Ctrl+,`   | Open settings           |
| `Esc`          | Cancel in-flight turn   |
| `Cmd/Ctrl+V`   | Paste into input        |

## Architecture notes

- **`ui::turn::run_turn`** is the agent turn loop. It streams events
  from the provider, accumulates text + `tool_use` blocks per
  content-block index, and on `stop_reason == "tool_use"` executes the
  requested tool, appends a `User { Content: Blocks(vec![ToolResult]) }`,
  and re-calls the provider. `MAX_TOOL_ROUNDS = 16` is a safety cap.
- **`ui::provider::unified::Translator`** normalizes per-provider
  streaming events into a single `StreamEvent` enum consumed by the
  GUI.
- The three GPUI child views (`SessionListView`, `SessionView`,
  `SettingsPanel`) are `cx.new`-ed once in `AppView::new` and stored
  as fields. Recreating them in `Render` would discard selection and
  in-memory state on every redraw.
- **`SqliteSessionStore`** persists sessions and per-session message
  history. The GUI's `load_session` reads from this store; submitting
  a turn currently keeps message bodies in memory only (persisting
  the bodies is tracked as follow-up).

## Tool permissions

Tools that mutate the filesystem or shell are gated behind the
permission system in `core::permissions`. The GUI currently wires a
permissive `AutoPermissionHandler` so the tool-use loop runs without
interactive prompts. Tightening this — surfacing per-tool approval
dialogs and persisting the user's choice — is tracked in the spec
under `docs/superpowers/specs/2026-06-28-agent-gui-design.md`.
