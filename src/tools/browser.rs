use serde::Deserialize;
use serde_json::Value;

use crate::App;
use crate::holds::Pass;
use crate::secrets::{Origin, Use};

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
    let intent = if input.action == "click_ref" && matches!(input.button.as_str(), "dbl" | "double")
    {
        Some("double-clicking")
    } else {
        super::gesture_intent(&input.action)
    };
    // CDP acts on the element without moving the X pointer, so the viewer's
    // agent cursor would point wherever the last native input left it. Put
    // the real pointer on the element first: the cursor, its label and the
    // page's own hover state then all agree with what is being acted on.
    let target = if matches!(
        input.action.as_str(),
        "click_ref" | "fill" | "hover" | "check" | "select"
    ) {
        element_point(app, &input.r#ref).await
    } else {
        None
    };
    if let (Some(intent), Some(display)) = (intent, app.display.clone()) {
        // "clicking Sign in": the verb, then what a person would call the
        // element. Page text, so it is redacted like any other output.
        let intent = match &target {
            Some((_, _, label)) if !label.is_empty() => format!("{intent} {label}"),
            _ => intent.to_owned(),
        };
        let intent = app.secrets.redact(&intent).into_owned();
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(mut hands) = display.hands.lock() {
                match target {
                    Some((x, y, _)) => {
                        let _ = hands.with_intent(Some(intent), |hands| hands.move_to(x, y));
                    }
                    None => hands.announce(Some(intent)),
                }
            }
        })
        .await;
    }
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
                let origin_matched = filled.sites.is_some();
                let label = app.secrets.redact(&filled.label).into_owned();
                let (name, field) = reference.split_once('.').unwrap_or((reference, ""));
                let login = filled.sites.clone();
                let mut site = String::new();
                let mut how = None;
                if let Some(sites) = &login {
                    let url = app.browser.current_url().await.unwrap_or_default();
                    site = crate::bridge::host_of(&url);
                    let on_site = Origin::of_page(&url)
                        .is_ok_and(|page| sites.iter().any(|site| site.allows(&page)));
                    // A username is who, not a secret: only what signs in waits.
                    if on_site
                        && field != "username"
                        && app.secrets.use_of(name) == Some(Use::Ask)
                        && !crate::bridge::person_has_it(app).await
                    {
                        let who = crate::bridge::who(app).await;
                        let path = crate::bridge::path_on(&url, &site);
                        match app.holds.fill(name, &site, &path, &who, crate::jobs::now()) {
                            Pass::Within => how = Some("within the 10 min"),
                            Pass::Answered(outcome) => how = Some(outcome.how()),
                            Pass::Held { id, lapsed } => {
                                return Err(waiting(name, &site, &id, lapsed));
                            }
                            Pass::Denied => {
                                return Err(format!(
                                    "The person denied this use of {name} on {site}. Nothing was typed; do not try another way, and ask them in the chat if it is still needed."
                                ));
                            }
                        }
                    }
                }
                let result = app.browser.fill_secret(&input.r#ref, filled).await;
                // Each use leaves a line in the card's record and a note on
                // the page, as the extension's own fills do.
                if login.is_some() {
                    match &result {
                        Ok(_) if field != "username" => {
                            crate::bridge::record(app, &site, "filled a login", true, how).await;
                        }
                        Err(error) if error.contains(" is typed only on ") => {
                            crate::bridge::record(app, &site, "was refused a login", false, None)
                                .await;
                        }
                        _ => {}
                    }
                }
                if result.is_ok()
                    && let Ok(Some((_, bounds))) = tokio::time::timeout(
                        std::time::Duration::from_millis(200),
                        crate::a11y::focused_metadata(app),
                    )
                    .await
                {
                    // Variables have no site restriction, so their receipt
                    // must not claim that an origin was checked.
                    let area = crate::x11::windows(&app.config.display)
                        .ok()
                        .and_then(|windows| {
                            windows
                                .into_iter()
                                .find(|w| w.class.to_ascii_lowercase().contains("chromium"))
                        })
                        .map(|window| window.bounds);
                    app.badges
                        .show(&app.config.display, bounds, area, label, origin_matched);
                }
                result
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

/// What a fill of an ask-me login answers while the person is asked: at
/// once, so no tool call waits on them.
fn waiting(name: &str, site: &str, id: &str, lapsed: bool) -> String {
    format!(
        "{}Waiting for the person to approve {name} on {site} (use {id}). Nothing was typed. Try this fill again after they answer at the desk.",
        if lapsed {
            "The earlier ask went unanswered and lapsed. "
        } else {
            ""
        }
    )
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

/// Where an element referenced by the last snapshot sits on the screen, in
/// X coordinates, scrolled into view. Refs are generated tokens; anything
/// else is refused rather than spliced into script.
async fn element_point(app: &App, reference: &str) -> Option<(i16, i16, String)> {
    if reference.is_empty()
        || !reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let js = format!(
        "(()=>{{const e=document.querySelector('[data-hotline-ref=\"{reference}\"]');\
         if(!e)return null;e.scrollIntoView({{block:'nearest',inline:'nearest'}});\
         const r=e.getBoundingClientRect();\
         const l=e.labels&&e.labels[0];\
         const t=((l&&l.innerText)||e.getAttribute('aria-label')||e.innerText||\
         e.getAttribute('placeholder')||'').trim().split('\\n')[0].slice(0,32);\
         return [[r.left+Math.min(r.width/2,r.width-12),r.top+r.height/2,\
         window.outerWidth-window.innerWidth,window.outerHeight-window.innerHeight,\
         window.devicePixelRatio||1],t]}})()"
    );
    let raw = tokio::time::timeout(std::time::Duration::from_millis(500), app.browser.eval(&js))
        .await
        .ok()?
        .ok()?;
    let (v, label): (Vec<f64>, String) = serde_json::from_str(&raw).ok()?;
    let [x, y, chrome_x, chrome_y, scale] = v.as_slice() else {
        return None;
    };
    let window = crate::x11::windows(&app.config.display)
        .ok()?
        .into_iter()
        .find(|w| w.class.to_ascii_lowercase().contains("chromium"))?;
    let [wx, wy, ww, wh] = window.bounds.map(f64::from);
    let sx = wx + (chrome_x / 2.0 + x) * scale;
    let sy = wy + (chrome_y + y) * scale;
    (sx >= wx && sx < wx + ww && sy >= wy && sy < wy + wh).then_some((sx as i16, sy as i16, label))
}

/// What a person would call the element a ref points at: its label, text or
/// placeholder, short. For the Activity window only; it is page text, so it
/// passes the same redaction as everything else there.
pub(super) async fn element_label(app: &App, reference: &str) -> Option<String> {
    if reference.is_empty()
        || !reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let js = format!(
        "(()=>{{const e=document.querySelector('[data-hotline-ref=\"{reference}\"]');\
         if(!e)return null;const l=e.labels&&e.labels[0];\
         return ((l&&l.innerText)||e.getAttribute('aria-label')||e.innerText||\
         e.getAttribute('placeholder')||e.getAttribute('title')||'').trim().split('\\n')[0]}})()"
    );
    let raw = tokio::time::timeout(std::time::Duration::from_millis(300), app.browser.eval(&js))
        .await
        .ok()?
        .ok()?;
    let label: String = serde_json::from_str::<Option<String>>(&raw).ok()??;
    let label: String = label.chars().take(40).collect();
    (!label.is_empty()).then_some(label)
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
