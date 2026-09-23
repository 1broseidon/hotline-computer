//! Pointer, keyboard, and clipboard, all from inside the process: XTEST for
//! the hands, the clipboard thread for text. Nothing here spawns a program.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::display::Display;
use crate::xtest::Hands;
use crate::{App, a11y, x11};

use super::{ToolResult, action_error, json_text, text};

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    x2: i32,
    #[serde(default)]
    y2: i32,
    #[serde(default)]
    clicks: i32,
    #[serde(default)]
    text: String,
    #[serde(default)]
    combo: String,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    steps: Vec<Value>,
    stop_on_error: Option<bool>,
    settle_ms: Option<u64>,
    #[serde(default)]
    capture_after: String,
    capture_on_error: Option<bool>,
}

pub async fn call(app: &App, arguments: Value, holder: &str) -> ToolResult {
    let input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    if input.action == "clipboard_read" {
        let display = app.display()?;
        let value = blocking(move || display.clipboard.read()).await?;
        return Ok(text(value));
    }
    if input.action == "batch" {
        return batch(app, holder, input).await;
    }
    let _guard = app.access.mutate(holder).await?;
    let result = one(
        app,
        &input.action,
        &json!({
            "x": input.x, "y": input.y, "x2": input.x2, "y2": input.y2,
            "clicks": input.clicks, "text": input.text, "combo": input.combo,
            "direction": input.direction
        }),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(
        input.settle_ms.unwrap_or(40).min(2000),
    ))
    .await;
    if input.capture_after == "final" || input.capture_after == "each" {
        return json_text(json!({"result":result,"capture":focused_tree(app).await}));
    }
    Ok(text(result))
}

/// After acting, the window acted in is the one worth reading; every
/// window's tree, with a browser open, is thousands of lines.
async fn focused_tree(app: &App) -> String {
    let windows = x11::windows(&app.config.display).unwrap_or_default();
    let focused: Vec<_> = windows.iter().filter(|w| w.focused).cloned().collect();
    a11y::tree(
        app,
        if focused.is_empty() {
            &windows
        } else {
            &focused
        },
    )
    .await
}

/// The wheel button and how many notches: `direction` up, down, left or
/// right with `clicks` notches (3 by default); without a direction, a
/// positive `clicks` scrolls up and a negative one down.
fn wheel(direction: &str, clicks: i16) -> Result<(u8, u32), String> {
    let notches = |default: u32| match clicks.unsigned_abs() {
        0 => default,
        count => u32::from(count),
    };
    match direction {
        "up" => Ok((4, notches(3))),
        "down" => Ok((5, notches(3))),
        "left" => Ok((6, notches(3))),
        "right" => Ok((7, notches(3))),
        "" if clicks != 0 => Ok((if clicks > 0 { 4 } else { 5 }, notches(0))),
        "" => Err("scroll needs direction (up, down, left or right)".into()),
        other => Err(format!(
            "unknown direction {other:?} (one of: up, down, left, right)"
        )),
    }
}

async fn settle_typing(app: &App) {
    let windows = x11::windows(&app.config.display).unwrap_or_default();
    a11y::settle_typing(app, &windows).await;
}

/// X round trips and the pauses between keystrokes happen off the runtime.
async fn blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| format!("input task: {error}"))?
}

async fn with_hands<T, F>(app: &App, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut Hands) -> Result<T, String> + Send + 'static,
{
    let display: Arc<Display> = app.display()?;
    blocking(move || {
        let mut hands = display
            .hands
            .lock()
            .map_err(|_| "the hands are poisoned".to_owned())?;
        work(&mut hands)
    })
    .await
}

async fn one(app: &App, action: &str, step: &Value) -> Result<String, String> {
    let integer = |name: &str| {
        step.get(name)
            .and_then(Value::as_i64)
            .unwrap_or_default()
            .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
    };
    let string = |name: &str| {
        step.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (x, y, x2, y2) = (integer("x"), integer("y"), integer("x2"), integer("y2"));
    match action {
        "click" => {
            with_hands(app, move |hands| {
                hands.move_to(x, y)?;
                hands.click(1, 1)?;
                Ok("clicked".into())
            })
            .await
        }
        "double_click" | "dclick" => {
            with_hands(app, move |hands| {
                hands.move_to(x, y)?;
                hands.click(1, 2)?;
                Ok("double-clicked".into())
            })
            .await
        }
        "right_click" | "rclick" => {
            with_hands(app, move |hands| {
                hands.move_to(x, y)?;
                hands.click(3, 1)?;
                Ok("right-clicked".into())
            })
            .await
        }
        "move" => {
            with_hands(app, move |hands| {
                hands.move_to(x, y)?;
                Ok("moved".into())
            })
            .await
        }
        "drag" => {
            with_hands(app, move |hands| {
                hands.move_to(x, y)?;
                hands.button(1, true)?;
                std::thread::sleep(Duration::from_millis(30));
                hands.move_to((x + x2) / 2, (y + y2) / 2)?;
                std::thread::sleep(Duration::from_millis(30));
                hands.move_to(x2, y2)?;
                std::thread::sleep(Duration::from_millis(30));
                hands.button(1, false)?;
                Ok("dragged".into())
            })
            .await
        }
        "scroll" => {
            let (button, notches) = wheel(&string("direction"), integer("clicks"))?;
            with_hands(app, move |hands| {
                hands.move_to(x, y)?;
                hands.click(button, notches)?;
                Ok("scrolled".into())
            })
            .await
        }
        "type" => {
            let content = string("text");
            with_hands(app, move |hands| {
                hands.type_text(&content)?;
                Ok::<_, String>(())
            })
            .await?;
            settle_typing(app).await;
            Ok("typed".into())
        }
        "key" => {
            let combo = string("combo");
            if combo.is_empty() {
                return Err("combo is required".into());
            }
            with_hands(app, move |hands| {
                hands.combo(&combo)?;
                Ok(format!("sent {combo}"))
            })
            .await
        }
        "paste" => {
            let content = string("text");
            let display = app.display()?;
            blocking(move || {
                display.clipboard.write(&content)?;
                let mut hands = display
                    .hands
                    .lock()
                    .map_err(|_| "the hands are poisoned".to_owned())?;
                hands.combo("ctrl+v")?;
                Ok::<_, String>(())
            })
            .await?;
            settle_typing(app).await;
            Ok("pasted".into())
        }
        "clipboard_write" => {
            let content = string("text");
            let display = app.display()?;
            blocking(move || {
                display.clipboard.write(&content)?;
                Ok("clipboard set".into())
            })
            .await
        }
        "focus" => {
            x11::activate(&app.config.display, &string("window_id"))?;
            Ok("focused".into())
        }
        "navigate" => app.browser.navigate(&string("url")).await,
        "wait" => {
            let wanted = string("text");
            let timeout = step
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .min(super::CALL_BUDGET_MS / 1000);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
            loop {
                let windows = x11::windows(&app.config.display).unwrap_or_default();
                let tree = a11y::tree(app, &windows).await;
                let page = app.browser.page_text_if_running().await.unwrap_or_default();
                if tree.contains(&wanted) || page.contains(&wanted) {
                    break Ok("found".into());
                }
                if tokio::time::Instant::now() >= deadline {
                    break Err(format!("text {wanted:?} did not appear within {timeout}s"));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        other => Err(action_error(
            "input batch",
            other,
            &[
                "click", "dclick", "rclick", "paste", "key", "type", "scroll", "drag", "move",
                "focus", "navigate", "wait",
            ],
        )),
    }
}

async fn batch(app: &App, holder: &str, input: Input) -> ToolResult {
    if input.steps.is_empty() {
        return Err("steps is required".into());
    }
    if input.steps.len() > 10 {
        return Err(format!("too many steps: got {}, max 10", input.steps.len()));
    }
    let _permit = app.access.run(holder).await?;
    let stop = input.stop_on_error.unwrap_or(true);
    let settle = Duration::from_millis(input.settle_ms.unwrap_or(40).min(2000));
    let capture_on_error = input.capture_on_error.unwrap_or(true);
    let capture_after = if input.capture_after.is_empty() {
        "final"
    } else {
        &input.capture_after
    };
    if !["final", "each", "none"].contains(&capture_after) {
        return Err("capture_after must be one of final, each, none".into());
    }
    let started = tokio::time::Instant::now();
    let mut results = Vec::new();
    let mut final_capture = None;
    for (index, step) in input.steps.iter().enumerate() {
        let step_started = tokio::time::Instant::now();
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| "step action is required".to_owned())?;
        let outcome = one(app, action, step).await;
        tokio::time::sleep(settle).await;
        let mut result = json!({"index":index,"ok":outcome.is_ok(),"duration_ms":step_started.elapsed().as_millis()});
        if let Err(error) = &outcome {
            result["error"] = json!(error);
            if capture_on_error {
                final_capture = Some(focused_tree(app).await);
            }
        }
        if capture_after == "each" {
            result["capture"] = json!(focused_tree(app).await);
        }
        results.push(result);
        if outcome.is_err() && stop {
            break;
        }
    }
    if capture_after == "final" {
        final_capture = Some(focused_tree(app).await);
    }
    json_text(
        json!({"steps":results,"capture":final_capture,"total_duration_ms":started.elapsed().as_millis()}),
    )
}

#[cfg(test)]
mod tests {
    use super::wheel;

    #[test]
    fn the_wheel_scrolls_the_way_it_is_told() {
        assert_eq!(wheel("down", 0), Ok((5, 3)));
        assert_eq!(wheel("up", 5), Ok((4, 5)));
        assert_eq!(wheel("right", -2), Ok((7, 2)));
        assert_eq!(wheel("", 2), Ok((4, 2)));
        assert_eq!(wheel("", -4), Ok((5, 4)));
        assert!(wheel("", 0).is_err());
        assert!(wheel("sideways", 1).is_err());
    }
}
