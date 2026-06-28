// ui::input::attachments — file ingestion pipeline.
// Drag-drop, file picker, and clipboard paste all funnel through `ingest_paths`.

use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::ui::model::{Attachment, AttachmentKind};

const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_DOC_BYTES:   u64 = 50 * 1024 * 1024;

pub fn classify(path: &Path) -> Option<AttachmentKind> {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    if mime.type_() == "image" { Some(AttachmentKind::Image) }
    else if mime == "application/pdf" { Some(AttachmentKind::Pdf) }
    else if mime.type_() == "text" { Some(AttachmentKind::Text) }
    else { None }
}

pub fn limit_for(kind: AttachmentKind) -> u64 {
    match kind {
        AttachmentKind::Image => MAX_IMAGE_BYTES,
        _ => MAX_DOC_BYTES,
    }
}

pub fn ingest_paths(
    paths: impl IntoIterator<Item = PathBuf>,
    attachments_dir: &Path,
) -> anyhow::Result<Vec<Attachment>> {
    let mut out = Vec::new();
    for p in paths {
        let Some(kind) = classify(&p) else { continue };
        let meta = std::fs::metadata(&p)?;
        if meta.len() > limit_for(kind) {
            anyhow::bail!("file too large: {} ({} bytes)", p.display(), meta.len());
        }
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("bin");
        let id = Uuid::new_v4().to_string();
        let dest = attachments_dir.join(format!("{id}.{ext}"));
        std::fs::copy(&p, &dest)?;
        let mime = mime_guess::from_path(&p).first_or_octet_stream().to_string();
        out.push(Attachment {
            id,
            kind,
            display_name: p.file_name()
                .and_then(|s| s.to_str()).unwrap_or("file").to_string(),
            mime,
            local_path: dest,
            size_bytes: meta.len(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn classify_known_types() {
        assert_eq!(classify(Path::new("a.png")), Some(AttachmentKind::Image));
        assert_eq!(classify(Path::new("a.pdf")), Some(AttachmentKind::Pdf));
        assert_eq!(classify(Path::new("a.txt")), Some(AttachmentKind::Text));
        assert_eq!(classify(Path::new("a.bin")), None);
    }

    #[test]
    fn ingest_copies_to_dir() {
        let tmp = TempDir::new().unwrap();
        let att_dir = tmp.path().join("att");
        std::fs::create_dir_all(&att_dir).unwrap();
        let src = tmp.path().join("hello.txt");
        std::fs::write(&src, b"hi").unwrap();
        let atts = ingest_paths([src], &att_dir).unwrap();
        assert_eq!(atts.len(), 1);
        assert!(atts[0].local_path.exists());
    }
}
