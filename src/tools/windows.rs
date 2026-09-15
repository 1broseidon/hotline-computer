use serde::Deserialize;
use serde_json::{Value, json};

use crate::{App, x11};

use super::{ToolResult, action_error, json_text, text};

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    window_id: String,
    #[serde(default)]
    unmaximize: bool,
}

pub async fn call(app: &App, arguments: Value, holder: &str) -> ToolResult {
    let input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    if input.action == "list" {
        return json_text(x11::windows(&app.config.display)?);
    }
    let _guard = app.access.mutate(holder).await?;
    if input.action == "tile" {
        return tile(app).await;
    }
    let id = required_id(&input)?;
    if !x11::windows(&app.config.display)?
        .iter()
        .any(|w| w.id == id)
    {
        return Err(format!("window {id} is not present"));
    }
    match input.action.as_str() {
        "focus" => x11::activate(&app.config.display, id)?,
        "close" => x11::close(&app.config.display, id)?,
        "maximize" => x11::maximize(&app.config.display, id, !input.unmaximize)?,
        action => {
            return Err(action_error(
                "windows",
                action,
                &["list", "focus", "close", "maximize", "tile"],
            ));
        }
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let windows = x11::windows(&app.config.display)?;
        let current = windows.iter().find(|w| w.id == id);
        let complete = match input.action.as_str() {
            "close" => current.is_none(),
            "focus" => current.is_some_and(|w| w.focused),
            "maximize" => current.is_some_and(|w| w.maximized != input.unmaximize),
            _ => false,
        };
        if complete {
            return json_text(
                json!({"ok":true,"action":input.action,"window_id":id,"windows":windows}),
            );
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(json!({"ok":false,"action":input.action,"window_id":id,"error":"window operation did not complete; inspect remaining windows or a confirmation dialog","windows":windows}).to_string());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

fn required_id(input: &Input) -> Result<&str, String> {
    (!input.window_id.is_empty())
        .then_some(input.window_id.as_str())
        .ok_or_else(|| "window_id is required".to_owned())
}

async fn tile(app: &App) -> ToolResult {
    let windows = x11::windows(&app.config.display)?;
    if windows.is_empty() {
        return Ok(text("no windows"));
    }
    let [left, top, width, height] = x11::workarea(&app.config.display)?;
    let browser = windows
        .iter()
        .position(|window| window.class.to_ascii_lowercase().contains("chromium"));
    let right_count = windows.len() - usize::from(browser.is_some());
    let midpoint = if browser.is_some() && right_count > 0 {
        width / 2
    } else {
        0
    };
    let right_height = height / right_count.max(1) as i32;
    let mut expected = Vec::new();
    let mut right_index = 0_i32;
    for (index, window) in windows.iter().enumerate() {
        let (x, y, width, height) = if Some(index) == browser {
            (
                left,
                top,
                if right_count == 0 { width } else { midpoint },
                height,
            )
        } else {
            let geometry = (
                left + midpoint,
                top + right_index * right_height,
                width - midpoint,
                right_height,
            );
            right_index += 1;
            geometry
        };
        expected.push((window.id.clone(), [x, y, width, height]));
        x11::maximize(&app.config.display, &window.id, false)?;
        x11::place(
            &app.config.display,
            &window.id,
            x,
            y,
            width as u32,
            height as u32,
        )?;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let actual = x11::windows(&app.config.display)?;
        let complete = expected
            .iter()
            .all(|(id, bounds)| actual.iter().any(|w| &w.id == id && w.bounds == *bounds));
        if complete {
            return json_text(json!({"ok":true,"windows":actual}));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(json!({"ok":false,"error":"one or more windows refused the requested tile geometry","expected":expected,"windows":actual}).to_string());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}
