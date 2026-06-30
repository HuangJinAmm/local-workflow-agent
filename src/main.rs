// local-workflow-agent — runnable example demonstrating the agent framework.
//
// This example shows three layers of the framework:
//   1. Direct tool invocation — use the tool system without an LLM (reads
//      Cargo.toml and globs for source files). Works without an API key.
//   2. Building tool definitions — converts tools into the API wire format.
//   3. LLM-driven agent loop — drives a multi-round conversation via the
//      shared `run_turn` helper (streaming + tool-use), printing events as
//      they arrive.
//
// Set ANTHROPIC_API_KEY to run part 3:
//     $env:ANTHROPIC_API_KEY = "sk-ant-..."
//     cargo run

use local_workflow_agent::agent::{run_turn, TurnEvent};
use local_workflow_agent::api::client::ClientConfig;
use local_workflow_agent::api::provider::LlmProvider;
use local_workflow_agent::api::provider_types::ProviderRequest;
use local_workflow_agent::api::providers::AnthropicProvider;
use local_workflow_agent::api::ApiToolDefinition;
use local_workflow_agent::core::config::{Config, PermissionMode};
use local_workflow_agent::core::constants::{
    ANTHROPIC_API_BASE, ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER, DEFAULT_MAX_TOKENS,
    DEFAULT_MODEL,
};
use local_workflow_agent::core::file_history::FileHistory;
use local_workflow_agent::core::permissions::{AutoPermissionHandler, PermissionHandler};
use local_workflow_agent::core::cost::CostTracker;
use local_workflow_agent::core::types::{Message, ToolResultContent};
use local_workflow_agent::tools::{
    all_tools, FileReadTool, GlobTool, GrepTool, Tool, ToolContext, ToolResult,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let working_dir = std::env::current_dir()?;
    println!("=== local-workflow-agent demo ===");
    println!("Working directory: {}\n", working_dir.display());

    // Build a ToolContext with a permissive handler so the demo tools can run
    // without interactive approval prompts.
    let tool_ctx = build_tool_context(working_dir.clone());

    // -----------------------------------------------------------------
    // Part 1: Direct tool invocation (no LLM required)
    // -----------------------------------------------------------------
    println!("--- Part 1: Direct tool invocation ---");

    // Read this project's Cargo.toml via the FileRead tool.
    let cargo_path = working_dir.join("Cargo.toml").display().to_string();
    let read_result = FileReadTool
        .execute(
            json!({ "file_path": cargo_path, "limit": 15 }),
            &tool_ctx,
        )
        .await;
    print_tool_result("Read Cargo.toml (first 15 lines)", &read_result);

    // Glob for Rust source files under src/.
    let glob_result = GlobTool
        .execute(json!({ "pattern": "src/**/*.rs", "path": working_dir.display().to_string() }), &tool_ctx)
        .await;
    print_tool_result("Glob src/**/*.rs", &glob_result);

    // Grep for the `pub mod` declarations in lib.rs.
    let grep_result = GrepTool
        .execute(
            json!({ "pattern": "pub mod", "path": working_dir.join("src/lib.rs").display().to_string(), "output_mode": "content", "-n": true }),
            &tool_ctx,
        )
        .await;
    print_tool_result("Grep 'pub mod' in lib.rs", &grep_result);

    // -----------------------------------------------------------------
    // Part 2: Build tool definitions for the API
    // -----------------------------------------------------------------
    println!("\n--- Part 2: Tool definitions ---");
    let tools: Vec<Box<dyn Tool>> = all_tools();
    println!("Registered {} built-in tools:", tools.len());
    for t in &tools {
        println!("  - {:<16} [{}] {}", t.name(), format!("{:?}", t.permission_level()), t.description().chars().take(60).collect::<String>());
    }

    // Convert the read-only tools into API wire-format definitions.
    let tool_defs: Vec<ApiToolDefinition> = tools
        .iter()
        .filter(|t| matches!(t.permission_level(), local_workflow_agent::tools::PermissionLevel::None | local_workflow_agent::tools::PermissionLevel::ReadOnly))
        .map(|t| ApiToolDefinition::from(&t.to_definition()))
        .collect();
    println!("\nExposed {} read-only tools to the LLM.", tool_defs.len());

    // -----------------------------------------------------------------
    // Part 3: LLM-driven agent loop (requires ANTHROPIC_API_KEY)
    //
    // Uses the shared `run_turn` helper (src/agent/turn.rs) instead of a
    // hand-rolled loop. This gives us streaming + tool-use for free.
    // -----------------------------------------------------------------
    println!("\n--- Part 3: LLM agent loop (via run_turn) ---");

    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_else(|_| {
        eprintln!("\nANTHROPIC_API_KEY not set — skipping LLM loop.");
        eprintln!("Set it to see the full agent loop with tool calling:");
        eprintln!("    $env:ANTHROPIC_API_KEY = \"sk-ant-...\"");
        std::process::exit(0);
    });

    let client_config = ClientConfig {
        api_key,
        api_base: ANTHROPIC_API_BASE.to_string(),
        api_version: ANTHROPIC_API_VERSION.to_string(),
        beta_features: ANTHROPIC_BETA_HEADER.to_string(),
        use_bearer_auth: false,
        ..Default::default()
    };
    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::from_config(client_config));

    let user_prompt = format!(
        "List the top-level modules in this Rust project by reading src/lib.rs, \
         then briefly describe what each module is for. The project root is {}.",
        working_dir.display()
    );

    // Build a ProviderRequest with all tools exposed to the model.
    let tool_definitions: Vec<_> = tools.iter().map(|t| t.to_definition()).collect();
    let request = ProviderRequest {
        model: DEFAULT_MODEL.to_string(),
        messages: vec![Message::user(user_prompt)],
        system_prompt: None,
        tools: tool_definitions,
        max_tokens: DEFAULT_MAX_TOKENS,
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: vec![],
        thinking: None,
        provider_options: serde_json::Value::Null,
    };

    // run_turn takes shared (Arc) handles for tools / context.
    let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(tools);
    let tool_ctx = Arc::new(tool_ctx);

    let (sink, rx) = async_channel::unbounded::<TurnEvent>();
    let cancel = CancellationToken::new();

    let session_id = "demo".to_string();
    let handle = tokio::spawn(run_turn(
        provider,
        session_id,
        request,
        tools,
        tool_ctx,
        sink,
        cancel,
    ));

    // Consume the TurnEvent stream and print incremental progress.
    while let Ok(event) = rx.recv().await {
        match event {
            TurnEvent::TextDelta { text } => {
                print!("{}", text);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            TurnEvent::ToolUseStart { id, name } => {
                println!("\n[Tool call] {} ({})", name, id);
            }
            TurnEvent::ToolUseDelta { partial_json, .. } => {
                print!("{}", partial_json);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            TurnEvent::ToolEnd { result, is_error, .. } => {
                let content = match result {
                    ToolResultContent::Text(t) => t,
                    ToolResultContent::Blocks(_) => "[complex content]".to_string(),
                };
                let preview = if content.len() > 800 {
                    format!("{}...(truncated, {} chars total)", &content[..800], content.len())
                } else {
                    content
                };
                let label = if is_error { "ERROR" } else { "result" };
                println!("  {}: {}", label, preview);
            }
            TurnEvent::Done { stop_reason, usage } => {
                println!("\n=== Agent finished ===");
                if let Some(sr) = stop_reason {
                    println!("Stop reason: {:?}", sr);
                }
                if let Some(u) = usage {
                    println!(
                        "Tokens: input={}, output={}",
                        u.input_tokens, u.output_tokens
                    );
                }
            }
            TurnEvent::Failed { error } => {
                eprintln!("\n[Agent failed] {:?}", error);
            }
            TurnEvent::Cancelled => {
                println!("\n[Agent cancelled]");
            }
        }
    }

    // Surface a panic from the run_turn task if one occurred.
    if let Err(e) = handle.await {
        eprintln!("[run_turn task panicked] {:?}", e);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a permissive ToolContext rooted at `working_dir`.
fn build_tool_context(working_dir: PathBuf) -> ToolContext {
    let handler: Arc<dyn PermissionHandler> = Arc::new(AutoPermissionHandler {
        mode: PermissionMode::BypassPermissions,
    });
    ToolContext {
        working_dir,
        permission_mode: PermissionMode::BypassPermissions,
        permission_handler: handler,
        cost_tracker: CostTracker::new(),
        session_id: "demo".to_string(),
        current_turn: Arc::new(AtomicUsize::new(0)),
        non_interactive: true,
        mcp_manager: None,
        lsp_manager: None,
        file_history: Arc::new(parking_lot::Mutex::new(FileHistory::new())),
        config: Config::default(),
        managed_agent_config: None,
        completion_notifier: None,
        pending_permissions: None,
        permission_manager: None,
        user_question_tx: None,
    }
}

/// Print a tool result with a header; truncates long output.
fn print_tool_result(header: &str, result: &ToolResult) {
    println!("\n> {}", header);
    if result.is_error {
        println!("  [ERROR] {}", result.content);
        return;
    }
    let content = &result.content;
    if content.len() > 600 {
        println!("{}...(truncated, {} chars total)", &content[..600], content.len());
    } else {
        println!("{}", content);
    }
}
