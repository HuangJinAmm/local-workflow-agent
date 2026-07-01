//! Files API client for uploading files to Anthropic.
//!
//! Ported from `chat-ai/src/services/agent/files.rs` and switched from
//! `smolhttp` (synchronous) to the library's already-vendored `reqwest`
//! crate so we do not need to pull in another HTTP stack.

use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct FileUploadResponse {
    id: String,
}

/// Get MIME type from file extension
fn get_mime_type(path: &PathBuf) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("md") => "text/plain",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Upload a file to the Anthropic Files API.
///
/// This is a network call — the caller should `await` it from an async
/// context (the background agent task in `handler.rs`).
pub async fn upload_file(api_key: &str, path: &PathBuf) -> Result<String> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let mime_type = get_mime_type(path);

    let file_bytes = std::fs::read(path)
        .map_err(|e| anyhow!("Failed to read file {}: {}", path.display(), e))?;

    // Build multipart form data manually — reqwest's `multipart` feature
    // is not enabled in this crate, so we hand-roll the body the same way
    // chat-ai did.
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
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", mime_type).as_bytes());
    body.extend_from_slice(&file_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.anthropic.com/v1/files")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "files-api-2025-04-14")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| anyhow!("File upload request failed: {}", e))?;

    let response_text = response.text().await.unwrap_or_default();

    if response_text.contains("\"error\"") {
        return Err(anyhow!("File upload error: {}", response_text));
    }

    let upload_response: FileUploadResponse = serde_json::from_str(&response_text).map_err(|e| {
        anyhow!(
            "Failed to parse upload response: {}. Response: {}",
            e,
            response_text
        )
    })?;

    tracing::debug!(
        "Uploaded file {} ({}) -> {}",
        path.display(),
        mime_type,
        upload_response.id
    );

    Ok(upload_response.id)
}
