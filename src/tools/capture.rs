use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use rmcp::model::ContentBlock;
use serde::Deserialize;
use serde_json::Value;

use crate::{App, a11y, x11};

use super::{ToolResult, action_error, text};

/// Scaled down to this, a whole screen still reads, and costs few tokens.
const MAX_EDGE: u32 = 1568;

#[derive(Default, Deserialize)]
struct Input {
    #[serde(default)]
    mode: String,
    path: Option<String>,
    settle_ms: Option<u64>,
    window: Option<String>,
    region: Option<[i32; 4]>,
}

pub async fn call(app: &App, arguments: Value) -> ToolResult {
    let input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    tokio::time::sleep(std::time::Duration::from_millis(
        input.settle_ms.unwrap_or(100).min(2000),
    ))
    .await;
    let mut windows = x11::windows(&app.config.display)?;
    if let Some(wanted) = &input.window {
        windows = vec![one_window(windows, wanted)?];
    }
    // A region is what was asked to be seen; a window is its own bounds.
    let rect = input
        .region
        .or_else(|| input.window.as_ref().map(|_| windows[0].bounds));
    match input.mode.as_str() {
        "" | "tree" | "image" => {
            // Reading native accessibility can wait for the application's event loop.
            // Capture afterwards so the pixels are at least as recent as that read.
            let tree = if input.mode == "image" {
                None
            } else if input.window.is_some() {
                Some(a11y::tree(app, &windows).await)
            } else {
                // Unasked, a window under another is noise: two apps share
                // the pane, and the one beneath has the larger tree. Name it
                // so it can be asked for.
                let hidden = x11::hidden(&app.config.display, &windows).unwrap_or_default();
                let (behind, shown): (Vec<_>, Vec<_>) = windows
                    .iter()
                    .cloned()
                    .partition(|w| hidden.contains(&w.id));
                let mut tree = a11y::tree(app, &shown).await;
                if !behind.is_empty() {
                    let names: Vec<String> = behind
                        .iter()
                        .map(|w| format!("{} {}", w.id, w.title))
                        .collect();
                    tree.push_str(&format!(
                        "Behind other windows, not read (pass `window` to read one): {}\n",
                        names.join("; ")
                    ));
                }
                Some(tree)
            };
            let picture = x11::picture(&app.config.display, rect, Some(MAX_EDGE))?;
            let mut blocks = vec![
                ContentBlock::image(
                    base64::engine::general_purpose::STANDARD.encode(&picture.png),
                    "image/png",
                ),
                ContentBlock::text(picture.describe()),
            ];
            blocks.extend(tree.map(ContentBlock::text));
            Ok(blocks)
        }
        "png" => {
            let path = input.path.map_or_else(
                || {
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    std::env::temp_dir().join(format!("hotline-computer-{timestamp}.png"))
                },
                Into::into,
            );
            let picture = x11::picture(&app.config.display, rect, None)?;
            tokio::fs::write(&path, picture.png)
                .await
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(text(path.to_string_lossy()))
        }
        mode => Err(action_error("capture", mode, &["tree", "image", "png"])),
    }
}

/// A window by its id, or by the one title that contains `wanted`.
pub fn one_window(windows: Vec<x11::Window>, wanted: &str) -> Result<x11::Window, String> {
    let lower = wanted.to_lowercase();
    let listing = |windows: &[x11::Window]| {
        windows
            .iter()
            .map(|window| format!("{} {:?}", window.id, window.title))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(window) = windows.iter().find(|window| window.id == wanted) {
        return Ok(window.clone());
    }
    let matching: Vec<_> = windows
        .iter()
        .filter(|window| window.title.to_lowercase().contains(&lower))
        .cloned()
        .collect();
    match matching.as_slice() {
        [window] => Ok(window.clone()),
        [] => Err(format!(
            "no window is {wanted:?}; the windows are {}",
            listing(&windows)
        )),
        several => Err(format!(
            "{wanted:?} names several windows: {}; use an id",
            listing(several)
        )),
    }
}
