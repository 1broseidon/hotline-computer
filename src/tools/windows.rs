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
    primary_id: Option<String>,
    observer_id: Option<String>,
}

pub async fn call(app: &App, arguments: Value, holder: &str) -> ToolResult {
    let input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    if input.action == "list" {
        return json_text(x11::windows(&app.config.display)?);
    }
    let _guard = app.access.mutate(holder).await?;
    if input.action == "tile" {
        return tile(app, &input).await;
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

/// The line between the observer and the person's shell, as on the desktop.
const STACK_GAP: i32 = 8;

fn layout(
    windows: &[x11::Window],
    area: [i32; 4],
    primary: Option<usize>,
) -> Result<Vec<(String, [i32; 4])>, String> {
    let [left, top, width, height] = area;
    let right: Vec<_> = windows
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != primary)
        .collect();
    let right_min = right
        .iter()
        .map(|(_, w)| w.minimum_size[0])
        .max()
        .unwrap_or(0);
    let primary_min = primary
        .map(|index| windows[index].minimum_size[0])
        .unwrap_or(0);
    if primary_min + right_min > width
        || windows.iter().any(|w| w.minimum_size[1] > height)
        || right
            .iter()
            .map(|(_, w)| i64::from(w.minimum_size[1]))
            .sum::<i64>()
            > i64::from(height)
    {
        return Err("selected windows' minimum sizes do not fit the work area; close an observer, choose a smaller pair, or increase the screen size".into());
    }
    let split = if primary.is_some() {
        if right.is_empty() {
            width
        } else {
            // The observer column is a third of the screen, as when the
            // terminal opens on its own; the app under test keeps two thirds.
            (width * 2 / 3).max(primary_min).min(width - right_min)
        }
    } else {
        0
    };
    let mut expected = Vec::new();
    if let Some(index) = primary {
        expected.push((windows[index].id.clone(), [left, top, split, height]));
    }
    // The person's shell keeps the bottom third of the column under a line,
    // as the desktop places it; the rest of the column is shared above it.
    let (shells, column): (Vec<_>, Vec<_>) = right
        .iter()
        .partition(|(_, w)| w.class.to_ascii_lowercase().contains("hotlineshell"));
    let shell_height = if shells.is_empty() { 0 } else { height / 3 };
    let column_height = height - shell_height;
    let mut y = top;
    let mut available = column_height - column.iter().map(|(_, w)| w.minimum_size[1]).sum::<i32>();
    for (position, (_, window)) in column.iter().enumerate() {
        let extra = available / (column.len() - position) as i32;
        available -= extra;
        let row_height = window.minimum_size[1] + extra;
        expected.push((
            window.id.clone(),
            [left + split, y, width - split, row_height],
        ));
        y += row_height;
    }
    for (_, window) in shells {
        expected.push((
            window.id.clone(),
            [
                left + split,
                top + column_height + STACK_GAP,
                width - split,
                shell_height - STACK_GAP,
            ],
        ));
    }
    Ok(expected)
}

async fn tile(app: &App, input: &Input) -> ToolResult {
    let all = x11::windows(&app.config.display)?;
    let (windows, primary) = match (&input.primary_id, &input.observer_id) {
        (None, None) => {
            let primary = all
                .iter()
                .position(|w| w.class.to_ascii_lowercase().contains("chromium"));
            (all, primary)
        }
        (Some(primary), Some(observer)) if primary != observer => {
            let mut selected = Vec::new();
            for id in [primary, observer] {
                selected.push(
                    all.iter()
                        .find(|w| &w.id == id)
                        .cloned()
                        .ok_or_else(|| format!("window {id} is not present"))?,
                );
            }
            (selected, Some(0))
        }
        _ => return Err(
            "tile requires distinct primary_id and observer_id, or neither for automatic layout"
                .into(),
        ),
    };
    if windows.is_empty() {
        return Ok(text("no windows"));
    }
    let expected = layout(&windows, x11::workarea(&app.config.display)?, primary)?;
    for (id, bounds) in &expected {
        x11::maximize(&app.config.display, id, false)?;
        x11::place(
            &app.config.display,
            id,
            bounds[0],
            bounds[1],
            bounds[2] as u32,
            bounds[3] as u32,
        )?;
    }
    // Raise the selected pair above unrelated windows. Placement alone leaves
    // perfectly valid geometry hidden beneath whichever app was last active.
    for (id, _) in expected.iter().rev() {
        x11::activate(&app.config.display, id)?;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut settled = false;
    loop {
        let actual = x11::windows(&app.config.display)?;
        let complete = expected.iter().all(|(id, bounds)| {
            actual
                .iter()
                .any(|w| &w.id == id && !w.maximized && w.bounds == *bounds)
        });
        if complete && settled {
            return json_text(json!({"ok":true,"windows":actual}));
        }
        settled = complete;
        if tokio::time::Instant::now() >= deadline {
            return Err(json!({"ok":false,"error":"one or more windows refused the requested tile geometry","expected":expected,"windows":actual}).to_string());
        }
        // Chromium can submit its saved geometry after observing the restore.
        // Reapply placement once restored, and require it to survive a poll.
        for (id, bounds) in &expected {
            if actual
                .iter()
                .any(|w| &w.id == id && !w.maximized && w.bounds != *bounds)
            {
                x11::place(
                    &app.config.display,
                    id,
                    bounds[0],
                    bounds[1],
                    bounds[2] as u32,
                    bounds[3] as u32,
                )?;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window(id: &str, minimum_size: [i32; 2]) -> x11::Window {
        x11::Window {
            id: id.into(),
            title: id.into(),
            class: String::new(),
            bounds: [0; 4],
            focused: false,
            pid: None,
            maximized: false,
            minimum_size,
        }
    }
    #[test]
    fn a_large_application_gets_its_minimum_width_above_the_bar() {
        let result = layout(
            &[window("app", [1280, 860]), window("observer", [100, 100])],
            [0, 48, 1920, 1032],
            Some(0),
        )
        .unwrap();
        assert_eq!(result[0].1, [0, 48, 1280, 1032]);
        assert_eq!(result[1].1, [1280, 48, 640, 1032]);
    }
    #[test]
    fn the_persons_shell_takes_the_bottom_third_of_the_column_under_a_line() {
        let mut shell = window("shell", [1, 1]);
        shell.class = "HotlineShell.HotlineShell".into();
        let result = layout(
            &[window("app", [500, 87]), window("observer", [1, 1]), shell],
            [0, 36, 1920, 1044],
            Some(0),
        )
        .unwrap();
        assert_eq!(result[0].1, [0, 36, 1280, 1044]);
        assert_eq!(result[1].1, [1280, 36, 640, 696]);
        assert_eq!(result[2].1, [1280, 740, 640, 340]);
    }
    #[test]
    fn impossible_layouts_fail_before_moving_any_windows() {
        assert!(
            layout(
                &[window("a", [1280, 860]), window("b", [800, 100])],
                [0, 48, 1920, 1032],
                Some(0)
            )
            .is_err()
        );
        assert!(
            layout(
                &[window("a", [100, 600]), window("b", [100, 600])],
                [0, 48, 1920, 1032],
                None
            )
            .is_err()
        );
    }
}
