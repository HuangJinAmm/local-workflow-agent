// Minimal runnable example for the local-workflow-agent framework.
//
// Demonstrates:
//   1. Building an Anthropic API client from an API key.
//   2. Constructing a simple chat request.
//   3. Sending the request and printing the assistant's reply.
//
// Set the `ANTHROPIC_API_KEY` environment variable before running:
//     $env:ANTHROPIC_API_KEY = "sk-ant-..."
//     cargo run --example hello_agent
//
// Or run the default binary:
//     cargo run

use local_workflow_agent::api::client::{AnthropicClient, ClientConfig};
use local_workflow_agent::api::{ApiMessage, CreateMessageRequest};
use local_workflow_agent::core::constants::{DEFAULT_MODEL, DEFAULT_MAX_TOKENS, ANTHROPIC_API_BASE, ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Read API key from environment.
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .unwrap_or_else(|_| {
            eprintln!("Warning: ANTHROPIC_API_KEY not set; using empty key (calls will fail).");
            String::new()
        });

    // Build the client.
    let config = ClientConfig {
        api_key,
        api_base: ANTHROPIC_API_BASE.to_string(),
        api_version: ANTHROPIC_API_VERSION.to_string(),
        beta_features: ANTHROPIC_BETA_HEADER.to_string(),
        use_bearer_auth: false,
        ..Default::default()
    };
    let client = AnthropicClient::new(config)?;

    // Build a minimal chat request.
    let request = CreateMessageRequest {
        model: DEFAULT_MODEL.to_string(),
        max_tokens: DEFAULT_MAX_TOKENS,
        messages: vec![ApiMessage {
            role: "user".to_string(),
            content: json!("Hello! Please reply with one short sentence."),
        }],
        system: None,
        tools: None,
        temperature: Some(0.7),
        top_p: None,
        top_k: None,
        stop_sequences: None,
        stream: false,
        thinking: None,
    };

    // Send the request.
    println!("Sending request to model {}...", DEFAULT_MODEL);
    match client.create_message(request).await {
        Ok(response) => {
            println!("\n--- Assistant reply ---");
            for block in &response.content {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    println!("{}", text);
                }
            }
            println!("\nStop reason: {:?}", response.stop_reason);
            println!(
                "Tokens: input={}, output={}",
                response.usage.input_tokens, response.usage.output_tokens
            );
        }
        Err(e) => {
            eprintln!("API error: {:?}", e);
        }
    }

    Ok(())
}
