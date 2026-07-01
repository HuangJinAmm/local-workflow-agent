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
