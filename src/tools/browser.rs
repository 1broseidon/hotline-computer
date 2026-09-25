use serde::Deserialize;
use serde_json::Value;

use crate::App;

use super::{ToolResult, action_error, text};

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    js: String,
    #[serde(default)]
    r#ref: String,
    #[serde(default)]
    button: String,
    text: Option<String>,
    secret: Option<String>,
    #[serde(default)]
    value: String,
    values: Option<Vec<String>>,
    #[serde(default)]
    uncheck: bool,
    index: Option<usize>,
    #[serde(default)]
    path: String,
}

pub async fn call(app: &App, arguments: Value, holder: &str) -> ToolResult {
    let input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    let mutating = !matches!(
        input.action.as_str(),
        "text" | "links" | "tabs" | "downloads"
    );
    let _guard = if mutating {
        Some(app.access.mutate(holder).await?)
    } else {
        None
    };
    let result = match input.action.as_str() {
        "navigate" => app.browser.navigate(&input.url).await,
        "text" => app.browser.text().await,
        "links" => app.browser.links().await,
        "eval" => app.browser.eval(&input.js).await,
        "click_ref" => {
            app.browser
                .click_ref(
                    &input.r#ref,
                    matches!(input.button.as_str(), "dbl" | "double"),
                )
                .await
        }
        "fill" => match resolve_fill(input.secret.as_deref(), input.text.as_deref())? {
            FillWith::Secret(reference) => {
                // The value is looked up here and typed by the browser; the
                // answer names it and never carries it.
                let filled = app.secrets.resolve(reference)?;
                app.browser.fill_secret(&input.r#ref, filled).await
            }
            FillWith::Text(text) => app.browser.fill(&input.r#ref, text).await,
        },
        "select" => {
            app.browser
                .select(
                    &input.r#ref,
                    &input.values.unwrap_or_else(|| vec![input.value]),
                )
                .await
        }
        "check" => app.browser.check(&input.r#ref, !input.uncheck).await,
        "hover" => app.browser.hover(&input.r#ref).await,
        "tabs" => app.browser.tabs().await,
        "tab_new" => app.browser.tab_new(&input.url).await,
        "tab_select" => {
            app.browser
                .tab_select(input.index.ok_or_else(|| "index is required".to_owned())?)
                .await
        }
        "tab_close" => app.browser.tab_close(input.index).await,
        "upload" => {
            if input.path.is_empty() {
                Err("path is required".into())
            } else {
                app.browser.upload(&input.r#ref, input.path.as_ref()).await
            }
        }
        "dialog_accept" => {
            app.browser
                .dialog(true, input.text.as_deref().unwrap_or(""))
                .await
        }
        "dialog_dismiss" => app.browser.dialog(false, "").await,
        "downloads" => downloads(app).await,
        "back" | "forward" | "reload" => app.browser.history(&input.action).await,
        action => Err(action_error(
            "browser",
            action,
            &[
                "navigate",
                "text",
                "links",
                "eval",
                "click_ref",
                "fill",
                "select",
                "check",
                "hover",
                "tabs",
                "tab_new",
                "tab_select",
                "tab_close",
                "upload",
                "dialog_accept",
                "dialog_dismiss",
                "downloads",
                "back",
                "forward",
                "reload",
            ],
        )),
    }?;
    Ok(text(result))
}

/// What `fill` should do once a supplied-but-empty `secret` has been told
/// apart from an actually-chosen one.
enum FillWith<'a> {
    Secret(&'a str),
    Text(&'a str),
}

/// The exactly-one rule for `fill`'s `text`/`secret` pair, kept separate from
/// the browser calls so it can be tested without a live browser.
///
/// A model that always emits both keys (some do, to satisfy strict JSON
/// schemas) sends the unused one as `""`; that must not read as "supplied".
/// So an empty (or absent) `secret` never counts, no matter what `text`
/// holds — only a non-empty `secret` alongside a non-empty `text` is the
/// ambiguous "both" case. `text` itself stays significant when empty:
/// `text:""` alone is the documented way to clear a field.
fn resolve_fill<'a>(
    secret: Option<&'a str>,
    text: Option<&'a str>,
) -> Result<FillWith<'a>, String> {
    let secret = secret.filter(|value| !value.is_empty());
    let text_in_use = text.filter(|value| !value.is_empty());
    match (secret, text_in_use) {
        (Some(_), Some(_)) => Err(
            "fill takes text or secret, not both; omit whichever one you are not using".to_owned(),
        ),
        (Some(reference), None) => Ok(FillWith::Secret(reference)),
        (None, _) => match text {
            Some(text) => Ok(FillWith::Text(text)),
            None => Err("fill requires text or secret; use text:\"\" to clear a field".to_owned()),
        },
    }
}

async fn downloads(app: &App) -> Result<String, String> {
    let path = app.config.home.join("Downloads");
    let mut entries = match tokio::fs::read_dir(&path).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("no downloads".into());
        }
        Err(error) => return Err(format!("downloads: {error}")),
    };
    let mut names = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(if names.is_empty() {
        "no downloads".into()
    } else {
        names.join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::{FillWith, resolve_fill};

    #[test]
    fn text_only_fills_with_text() {
        assert!(matches!(
            resolve_fill(None, Some("hi")),
            Ok(FillWith::Text("hi"))
        ));
    }

    #[test]
    fn secret_only_fills_with_secret() {
        assert!(matches!(
            resolve_fill(Some("GITHUB.password"), None),
            Ok(FillWith::Secret("GITHUB.password"))
        ));
    }

    #[test]
    fn an_unused_empty_secret_alongside_text_still_fills_with_text() {
        assert!(matches!(
            resolve_fill(Some(""), Some("hi")),
            Ok(FillWith::Text("hi"))
        ));
    }

    #[test]
    fn an_unused_empty_text_alongside_a_secret_still_fills_with_the_secret() {
        assert!(matches!(
            resolve_fill(Some("GITHUB.password"), Some("")),
            Ok(FillWith::Secret("GITHUB.password"))
        ));
    }

    #[test]
    fn an_empty_text_alone_clears_the_field() {
        assert!(matches!(
            resolve_fill(None, Some("")),
            Ok(FillWith::Text(""))
        ));
    }

    #[test]
    fn a_real_text_and_a_real_secret_together_is_an_error() {
        assert!(resolve_fill(Some("GITHUB.password"), Some("hi")).is_err());
    }

    #[test]
    fn neither_argument_is_an_error() {
        assert!(resolve_fill(None, None).is_err());
    }
}
