//! GPUI asset source — a combined source that merges our chat-ai icons with
//! the icons bundled in the `gpui-component-assets` crate.
//!
//! Ported from `chat-ai/src/assets.rs`, then extended so that lookups for
//! `icons/window-minimize.svg`, `icons/window-maximize.svg`,
//! `icons/window-close.svg`, … (used by `gpui_component::TitleBar`) resolve
//! through `gpui_component_assets::Assets` when our local `assets/` does not
//! contain them.
//!
//! Why combine? `Application::new().with_assets(...)` installs a SINGLE
//! global `AssetSource`. If we register only our chat-ai icons, every
//! gpui-component internal icon lookup (window controls, selects, etc.)
//! fails. `gpui_component::init(cx)` cannot install its own asset source on
//! top of ours, so we have to chain the two sources ourselves.

use anyhow::anyhow;
use gpui::AssetSource;
use rust_embed::RustEmbed;

/// Bundled chat-ai icons under `assets/icons/`.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/assets"]
#[include = "icons/**/*"]
#[exclude = "*.DS_Store"]
pub struct LocalAssets;

/// Combined asset source: chat-ai icons first, then gpui-component-assets
/// for everything else (window controls, etc.).
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        // 1. Try our chat-ai icons first.
        if let Some(file) = LocalAssets::get(path) {
            return Ok(Some(file.data));
        }

        // 2. Fall back to the gpui-component-assets crate — this is where
        //    TitleBar's window-minimize / window-maximize / window-close and
        //    every other `IconName`-backed icon lives.
        if let Ok(Some(data)) = gpui_component_assets::Assets.load(path) {
            return Ok(Some(data));
        }

        Err(anyhow!("could not find asset at path \"{}\"", path))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        let mut out: Vec<gpui::SharedString> = LocalAssets::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();

        if let Ok(other) = gpui_component_assets::Assets.list(path) {
            for p in other {
                if !out.iter().any(|x: &gpui::SharedString| x.as_ref() == p.as_ref()) {
                    out.push(p);
                }
            }
        }

        Ok(out)
    }
}
