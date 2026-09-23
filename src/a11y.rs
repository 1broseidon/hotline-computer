use atspi::CoordType;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::bus::BusProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::zbus::proxy::CacheProperties;

use crate::x11::Window;

const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 400;
/// Folded wrappers are not listed, so this bounds the walk itself.
const MAX_VISITED: usize = 5000;
/// Containers that only group: without a name, a value or focus they add a
/// line and a level of indentation and nothing a reader can act on.
const WRAPPERS: [&str; 6] = [
    "panel",
    "section",
    "filler",
    "unknown",
    "redundant object",
    "invalid",
];

#[derive(Debug)]
struct Node {
    depth: usize,
    role: String,
    name: String,
    bounds: [i32; 4],
    states: String,
    value: Option<String>,
}

pub async fn tree(app: &crate::App, windows: &[Window]) -> String {
    let queried = tokio::time::timeout(std::time::Duration::from_secs(5), query(app, windows))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let mut output = String::new();
    for window in windows {
        output.push_str(&format!(
            "[{} {} {},{} {}x{}]\n",
            window.id,
            truncate(&window.title, 60),
            window.bounds[0],
            window.bounds[1],
            window.bounds[2],
            window.bounds[3]
        ));
        let candidates: Vec<_> = queried
            .iter()
            .filter(|tree| matches_window(tree, window))
            .collect();
        // A single application may have several windows with the same title.
        // Prefer the matching geometry before treating a title as ambiguous.
        let exact: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|tree| bounds_match(tree, window))
            .collect();
        let candidates = if exact.is_empty() { candidates } else { exact };
        if let [tree] = candidates.as_slice() {
            let nodes = &tree.nodes;
            for node in nodes {
                output.push_str(&"  ".repeat(node.depth + 1));
                output.push_str(&format!(
                    "[{}] {} {},{} {}x{} states=[{}]{}\n",
                    node.role,
                    truncate(&node.name, 200),
                    node.bounds[0],
                    node.bounds[1],
                    node.bounds[2],
                    node.bounds[3],
                    node.states,
                    node.value
                        .as_ref()
                        .map(|value| format!(" value={:?}", truncate(value, 200)))
                        .unwrap_or_default()
                ));
            }
            if tree.truncated {
                output.push_str(&format!(
                    "  [more than {MAX_NODES} elements; the first {MAX_NODES} are listed]\n"
                ));
            }
        }
        if candidates.len() != 1 {
            output.push_str(
                "  [accessibility unavailable: no unambiguous application/window match]\n",
            );
        }
        output.push('\n');
    }
    if output.is_empty() {
        output.push_str("[desktop no windows]\n");
    }
    output
}

struct WindowTree {
    title: String,
    pid: Option<u32>,
    bounds: [i32; 4],
    nodes: Vec<Node>,
    truncated: bool,
}

pub async fn connection(app: &crate::App) -> Result<&atspi::zbus::Connection, String> {
    app.a11y
        .get_or_try_init(|| async {
            let session = atspi::zbus::Connection::session()
                .await
                .map_err(|error| format!("session D-Bus: {error}"))?;
            let status = atspi::proxy::bus::StatusProxy::new(&session)
                .await
                .map_err(|e| e.to_string())?;
            status
                .set_is_enabled(true)
                .await
                .map_err(|e| e.to_string())?;
            status
                .set_screen_reader_enabled(true)
                .await
                .map_err(|e| e.to_string())?;
            let address = BusProxy::new(&session)
                .await
                .map_err(|error| error.to_string())?
                .get_address()
                .await
                .map_err(|error| error.to_string())?;
            let connection = atspi::zbus::connection::Builder::address(address.as_str())
                .map_err(|error| error.to_string())?
                .build()
                .await
                .map_err(|error| error.to_string())?;
            // WebKit enables its tree while an accessibility client remains registered.
            // Keep this bus connection for the computer lifetime, including between captures.
            connection
                .call_method(
                    Some("org.a11y.atspi.Registry"),
                    "/org/a11y/atspi/registry",
                    Some("org.a11y.atspi.Registry"),
                    "RegisterEvent",
                    &("object:children-changed", Vec::<&str>::new(), ""),
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(connection)
        })
        .await
}

// WebKit's embedded root uses a well-known bus name, while atspi::ObjectRef
// currently accepts only unique names. Both are valid destinations on D-Bus.
type Reference = (String, atspi::zbus::zvariant::OwnedObjectPath);

async fn children(proxy: &AccessibleProxy<'_>) -> Vec<Reference> {
    let mut children: Vec<Reference> = proxy
        .inner()
        .call("GetChildren", &())
        .await
        .unwrap_or_default();
    for (name, _) in &mut children {
        if name.is_empty() {
            *name = proxy.inner().destination().to_string();
        }
    }
    children
}

// Properties are read one at a time: Qt's bridge answers no GetAll, which a
// property cache sends first, and Qt 6.8 crashes the application on it.
async fn accessible<'a>(
    connection: &'a atspi::zbus::Connection,
    reference: Reference,
) -> Result<AccessibleProxy<'a>, String> {
    AccessibleProxy::builder(connection)
        .cache_properties(CacheProperties::No)
        .destination(reference.0)
        .map_err(|e| e.to_string())?
        .path(reference.1)
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())
}

/// The trees of the applications that own `windows`; others are not walked.
async fn query(app: &crate::App, windows: &[Window]) -> Result<Vec<WindowTree>, String> {
    let connection = connection(app).await?;
    let registry = AccessibleProxy::builder(connection)
        .cache_properties(CacheProperties::No)
        .destination("org.a11y.atspi.Registry")
        .map_err(|error| error.to_string())?
        .path("/org/a11y/atspi/accessible/root")
        .map_err(|error| error.to_string())?
        .build()
        .await
        .map_err(|error| error.to_string())?;

    let bus = atspi::zbus::fdo::DBusProxy::new(connection)
        .await
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for application in children(&registry).await {
        let pid = match atspi::zbus::names::BusName::try_from(application.0.as_str()) {
            Ok(name) => bus.get_connection_unix_process_id(name).await.ok(),
            Err(_) => None,
        };
        let owns = |pid: u32| {
            windows.iter().any(|window| {
                window
                    .pid
                    .is_none_or(|owner| owner == pid || process_descends_from(pid, owner))
            })
        };
        if pid.is_some_and(|pid| !owns(pid)) {
            continue;
        }
        let Ok(application) = accessible(connection, application).await else {
            continue;
        };
        for window_ref in children(&application).await {
            let Ok(window) = accessible(connection, window_ref).await else {
                continue;
            };
            let title = window.name().await.unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            // Chromium builds its tree once a client reads a window's attributes.
            let _ = window.get_attributes().await;
            let descendants = children(&window).await;
            let mut stack: Vec<_> = descendants
                .into_iter()
                .rev()
                .map(|child| (child, 0_usize, 0_usize))
                .collect();
            let mut nodes = Vec::new();
            let mut truncated = false;
            let mut visited = 0;
            // `level` is how deep the node really is; `depth` how far it is
            // indented once wrappers are folded.
            while let Some((reference, level, depth)) = stack.pop() {
                if level >= MAX_DEPTH {
                    continue;
                }
                visited += 1;
                if nodes.len() >= MAX_NODES || visited > MAX_VISITED {
                    truncated = true;
                    break;
                }
                let Ok(proxy) = accessible(connection, reference).await else {
                    continue;
                };
                let name = proxy.name().await.unwrap_or_default();
                let role = proxy
                    .get_role()
                    .await
                    .map(|role| role.name().to_owned())
                    .unwrap_or_else(|_| "unknown".to_owned());
                let states: Vec<String> = proxy
                    .get_state()
                    .await
                    .map(|states| states.iter().map(|state| state.to_string()).collect())
                    .unwrap_or_default();
                // Neither on screen nor able to be, as Chromium's whole hidden
                // toolbars are. What scrolls out of view stays visible. Its
                // children are still read: WebKit's scroll pane says it is
                // hidden while the page inside it is on screen.
                let hidden =
                    !states.is_empty() && !states.iter().any(|s| s == "showing" || s == "visible");
                let mut value = None;
                let bounds = match proxy.proxies().await {
                    Ok(proxies) => {
                        // Password fields may expose their contents through Text.
                        // Only return ordinary editable text and numeric controls.
                        if role != "password text" {
                            if let Ok(numeric) = proxies.value().await {
                                value = numeric
                                    .current_value()
                                    .await
                                    .ok()
                                    .map(|value| value.to_string());
                            } else if matches!(role.as_str(), "text" | "entry")
                                && let Ok(text) = proxies.text().await
                            {
                                value = text.get_text(0, 200).await.ok();
                            }
                        }
                        match proxies.component().await {
                            Ok(component) => component
                                .get_extents(CoordType::Screen)
                                .await
                                .map(|(x, y, width, height)| [x, y, width, height])
                                .unwrap_or([0; 4]),
                            Err(_) => [0; 4],
                        }
                    }
                    Err(_) => [0; 4],
                };
                let wrapper = hidden
                    || WRAPPERS.contains(&role.as_str())
                        && name.is_empty()
                        && value.is_none()
                        && !states.iter().any(|s| s == "focusable");
                let listed = !wrapper && (!name.is_empty() || bounds[2] > 0 || bounds[3] > 0);
                if listed {
                    nodes.push(Node {
                        depth,
                        role,
                        name,
                        bounds,
                        states: states.join(","),
                        value,
                    });
                }
                let below = if wrapper { depth } else { depth + 1 };
                let mut children = children(&proxy).await;
                children.reverse();
                stack.extend(children.into_iter().map(|child| (child, level + 1, below)));
            }
            let bounds = match window.proxies().await {
                Ok(proxies) => match proxies.component().await {
                    Ok(component) => component
                        .get_extents(CoordType::Screen)
                        .await
                        .map(|(x, y, w, h)| [x, y, w, h])
                        .unwrap_or([0; 4]),
                    Err(_) => [0; 4],
                },
                Err(_) => [0; 4],
            };
            result.push(WindowTree {
                title,
                pid,
                bounds,
                nodes,
                truncated,
            });
        }
    }
    Ok(result)
}

/// Waits until the text field with focus stops changing. An application
/// that handles each keystroke slowly, such as a development build
/// re-rendering, can still be working through typed text when the next
/// click arrives; the click moves focus and the rest of the text is lost.
/// Where no focused field can be read, it waits briefly instead.
pub async fn settle_typing(app: &crate::App, windows: &[Window]) {
    use std::time::Duration;
    const POLL: Duration = Duration::from_millis(50);
    const STEADY_POLLS: usize = 3;
    const LONGEST: Duration = Duration::from_secs(10);
    let Some(window) = windows.iter().find(|window| window.focused) else {
        return;
    };
    let field = tokio::time::timeout(Duration::from_secs(2), focused_text(app, window))
        .await
        .ok()
        .flatten();
    let Some(field) = field else {
        tokio::time::sleep(POLL * STEADY_POLLS as u32).await;
        return;
    };
    let deadline = tokio::time::Instant::now() + LONGEST;
    let (mut last, mut steady) = (None, 0);
    while tokio::time::Instant::now() < deadline {
        let count = field.character_count().await.ok();
        if count == last {
            steady += 1;
            if steady >= STEADY_POLLS {
                return;
            }
        } else {
            (last, steady) = (count, 0);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The editable text with focus in the application that owns `window`.
async fn focused_text<'a>(
    app: &'a crate::App,
    window: &Window,
) -> Option<atspi::proxy::text::TextProxy<'a>> {
    use atspi::State;
    let connection = connection(app).await.ok()?;
    let registry = accessible(
        connection,
        (
            "org.a11y.atspi.Registry".to_owned(),
            atspi::zbus::zvariant::OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root")
                .ok()?,
        ),
    )
    .await
    .ok()?;
    let bus = atspi::zbus::fdo::DBusProxy::new(connection).await.ok()?;
    for application in children(&registry).await {
        let pid = match atspi::zbus::names::BusName::try_from(application.0.as_str()) {
            Ok(name) => bus.get_connection_unix_process_id(name).await.ok(),
            Err(_) => None,
        };
        let owns = match (pid, window.pid) {
            (Some(pid), Some(owner)) => pid == owner || process_descends_from(pid, owner),
            _ => false,
        };
        if !owns {
            continue;
        }
        let mut stack = vec![application];
        let mut visited = 0;
        while let Some(reference) = stack.pop() {
            visited += 1;
            if visited > MAX_VISITED {
                break;
            }
            let Ok(proxy) = accessible(connection, reference).await else {
                continue;
            };
            let states = proxy.get_state().await.unwrap_or_default();
            if states.contains(State::Focused) && states.contains(State::Editable) {
                return proxy.proxies().await.ok()?.text().await.ok();
            }
            stack.extend(children(&proxy).await);
        }
    }
    None
}

fn bounds_match(tree: &WindowTree, window: &Window) -> bool {
    tree.bounds[2] > 0
        && tree
            .bounds
            .iter()
            .zip(window.bounds)
            .all(|(a, b)| (i64::from(*a) - i64::from(b)).abs() <= 4)
}

fn matches_window(tree: &WindowTree, window: &Window) -> bool {
    let bounds_match = bounds_match(tree, window);
    let title_match = tree.title == window.title;
    match (tree.pid, window.pid) {
        (Some(a), Some(b)) => {
            (a == b || process_descends_from(a, b)) && (title_match || bounds_match)
        }
        _ => title_match && bounds_match,
    }
}

fn process_descends_from(mut child: u32, parent: u32) -> bool {
    for _ in 0..8 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else {
            return false;
        };
        let Some((_, tail)) = stat.rsplit_once(')') else {
            return false;
        };
        let Some(pid) = tail
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u32>().ok())
        else {
            return false;
        };
        if pid == parent {
            return true;
        }
        if pid <= 1 || pid == child {
            return false;
        }
        child = pid;
    }
    false
}

fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!(
            "{}...",
            prefix
                .chars()
                .take(max.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_titles_never_cross_application_boundaries() {
        let window = Window {
            id: "0x1".into(),
            title: "Hotline Computer".into(),
            class: "chromium".into(),
            bounds: [0, 0, 800, 600],
            focused: true,
            pid: Some(100),
            maximized: false,
            minimum_size: [1, 1],
        };
        let mut tree = WindowTree {
            title: "Hotline".into(),
            pid: Some(200),
            bounds: [0, 0, 800, 600],
            nodes: vec![],
            truncated: false,
        };
        assert!(!matches_window(&tree, &window));
        tree.pid = Some(100);
        assert!(matches_window(&tree, &window));
        tree.bounds = [800, 0, 800, 600];
        assert!(!matches_window(&tree, &window));
        tree.pid = None;
        tree.title = window.title.clone();
        assert!(!matches_window(&tree, &window));
    }
}
