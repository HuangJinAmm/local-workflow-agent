// local-workflow-agent — runnable example demonstrating the agent framework.
//
// This example shows three layers of the framework:
//   1. Direct tool invocation — use the tool system without an LLM (reads
//      Cargo.toml and globs for source files). Works without an API key.
//   2. Building tool definitions — converts tools into the API wire format.
//   3. LLM-driven agent loop — sends tools to the model, executes the
//      tool_use blocks it returns, and loops until the turn ends.
//
// Set ANTHROPIC_API_KEY to run part 3:
//     $env:ANTHROPIC_API_KEY = "sk-ant-..."
//     cargo run

use local_workflow_agent::api::client::{AnthropicClient, ClientConfig};
use local_workflow_agent::api::{ApiMessage, ApiToolDefinition, CreateMessageRequest};
use local_workflow_agent::core::config::{Config, PermissionMode};
use local_workflow_agent::core::constants::{
    ANTHROPIC_API_BASE, ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER, DEFAULT_MAX_TOKENS,
    DEFAULT_MODEL,
};
use local_workflow_agent::core::file_history::FileHistory;
use local_workflow_agent::core::permissions::{AutoPermissionHandler, PermissionHandler};
use local_workflow_agent::core::cost::CostTracker;
use local_workflow_agent::tools::{
    all_tools, find_tool, FileReadTool, GlobTool, GrepTool, Tool, ToolContext, ToolResult,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

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
    // -----------------------------------------------------------------
    println!("\n--- Part 3: LLM agent loop ---");

    let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_else(|_| {
        eprintln!("\nANTHROPIC_API_KEY not set — skipping LLM loop.");
        eprintln!("Set it to see the full agent loop with tool calling:");
        eprintln!("    $env:ANTHROPIC_API_KEY = \"sk-ant-...\"");
        std::process::exit(0);
    });

    let client = AnthropicClient::new(ClientConfig {
        api_key,
        api_base: ANTHROPIC_API_BASE.to_string(),
        api_version: ANTHROPIC_API_VERSION.to_string(),
        beta_features: ANTHROPIC_BETA_HEADER.to_string(),
        use_bearer_auth: false,
        ..Default::default()
    })?;

    run_agent_loop(&client, &tool_defs, &tools, &tool_ctx, &working_dir).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Agent loop: send messages → execute tool_use blocks → loop until end_turn
// ---------------------------------------------------------------------------

async fn run_agent_loop(
    client: &AnthropicClient,
    tool_defs: &[ApiToolDefinition],
    tools: &[Box<dyn Tool>],
    tool_ctx: &ToolContext,
    working_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let user_prompt = format!(
        "List the top-level modules in this Rust project by reading src/lib.rs, \
         then briefly describe what each module is for. The project root is {}.",
        working_dir.display()
    );

    let mut messages: Vec<ApiMessage> = vec![ApiMessage {
        role: "user".to_string(),
        content: json!(user_prompt),
    }];

    const MAX_TURNS: usize = 8;

    for turn in 1..=MAX_TURNS {
        println!("\n=== Turn {} ===", turn);
        let request = CreateMessageRequest {
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            messages: messages.clone(),
            system: None,
            tools: Some(tool_defs.to_vec()),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: false,
            thinking: None,
        };

        let response = match client.create_message(request).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("API error on turn {}: {:?}", turn, e);
                return Ok(());
            }
        };

        let stop = response.stop_reason.as_deref().unwrap_or("unknown");
        println!("Stop reason: {}", stop);

        // Collect text and tool_use blocks from the response.
        let mut assistant_text = String::new();
        let mut tool_use_blocks: Vec<(String, String, Value)> = Vec::new();

        for block in &response.content {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        assistant_text.push_str(t);
                    }
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    tool_use_blocks.push((id, name, input));
                }
                _ => {}
            }
        }

        if !assistant_text.is_empty() {
            println!("\n[Assistant]\n{}", assistant_text);
        }

        // No tool calls → turn is complete.
        if tool_use_blocks.is_empty() {
            println!("\n=== Agent finished ===");
            println!(
                "Tokens: input={}, output={}",
                response.usage.input_tokens, response.usage.output_tokens
            );
            break;
        }

        // Append the assistant message (with its tool_use blocks) to history.
        messages.push(ApiMessage {
            role: "assistant".to_string(),
            content: serde_json::to_value(&response.content).unwrap_or(Value::Null),
        });

        // Execute each tool_use block and build tool_result content.
        let mut tool_results: Vec<Value> = Vec::new();
        for (tool_id, tool_name, tool_input) in &tool_use_blocks {
            println!("\n[Tool call] {} ({})", tool_name, tool_id);
            println!("  input: {}", tool_input);

            let result = execute_tool_by_name(tool_name, tool_input, tools, tool_ctx).await;
            let content = if result.is_error {
                format!("[ERROR] {}", result.content)
            } else {
                result.content.clone()
            };

            // Truncate very long tool output for readability (preview only).
            let preview = if content.len() > 800 {
                format!("{}...(truncated, {} chars total)", &content[..800], content.len())
            } else {
                content.clone()
            };
            println!("  result: {}", preview);

            tool_results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_id,
                "content": content,
                "is_error": result.is_error,
            }));
        }

        // Append the tool results as a user message.
        messages.push(ApiMessage {
            role: "user".to_string(),
            content: Value::Array(tool_results),
        });

        if turn == MAX_TURNS {
            println!("\nReached max turns ({}), stopping.", MAX_TURNS);
        }
    }

    Ok(())
}

/// Execute a tool by name, falling back to an error result if unknown.
async fn execute_tool_by_name(
    name: &str,
    input: &Value,
    tools: &[Box<dyn Tool>],
    ctx: &ToolContext,
) -> ToolResult {
    // Look up the tool in the provided slice first.
    for t in tools {
        if t.name() == name {
            return t.execute(input.clone(), ctx).await;
        }
    }
    // Fall back to the global registry (covers tools not in the slice).
    if let Some(t) = find_tool(name) {
        return t.execute(input.clone(), ctx).await;
    }
    ToolResult::error(format!("Unknown tool: {}", name))
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
