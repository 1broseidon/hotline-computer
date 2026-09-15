use atspi::CoordType;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::bus::BusProxy;
use atspi::proxy::proxy_ext::ProxyExt;

use crate::x11::Window;

const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 400;

#[derive(Debug)]
struct Node {
    depth: usize,
    role: String,
    name: String,
    bounds: [i32; 4],
}

pub async fn tree(app: &crate::App, windows: &[Window]) -> String {
    let queried = tokio::time::timeout(std::time::Duration::from_secs(5), query(app))
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
        if let [tree] = candidates.as_slice() {
            let nodes = &tree.nodes;
            for node in nodes {
                output.push_str(&"  ".repeat(node.depth + 1));
                output.push_str(&format!(
                    "[{}] {} {},{} {}x{}\n",
                    node.role,
                    truncate(&node.name, 200),
                    node.bounds[0],
                    node.bounds[1],
                    node.bounds[2],
                    node.bounds[3]
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

async fn accessible<'a>(
    connection: &'a atspi::zbus::Connection,
    reference: Reference,
) -> Result<AccessibleProxy<'a>, String> {
    AccessibleProxy::builder(connection)
        .destination(reference.0)
        .map_err(|e| e.to_string())?
        .path(reference.1)
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())
}

async fn query(app: &crate::App) -> Result<Vec<WindowTree>, String> {
    let connection = connection(app).await?;
    let registry = AccessibleProxy::builder(connection)
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
            let descendants = children(&window).await;
            let mut stack: Vec<_> = descendants
                .into_iter()
                .rev()
                .map(|child| (child, 0_usize))
                .collect();
            let mut nodes = Vec::new();
            while let Some((reference, depth)) = stack.pop() {
                if nodes.len() >= MAX_NODES || depth >= MAX_DEPTH {
                    continue;
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
                let bounds = match proxy.proxies().await {
                    Ok(proxies) => match proxies.component().await {
                        Ok(component) => component
                            .get_extents(CoordType::Screen)
                            .await
                            .map(|(x, y, width, height)| [x, y, width, height])
                            .unwrap_or([0; 4]),
                        Err(_) => [0; 4],
                    },
                    Err(_) => [0; 4],
                };
                if !name.is_empty() || bounds[2] > 0 || bounds[3] > 0 {
                    nodes.push(Node {
                        depth,
                        role,
                        name,
                        bounds,
                    });
                }
                let mut children = children(&proxy).await;
                children.reverse();
                stack.extend(children.into_iter().map(|child| (child, depth + 1)));
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
            });
        }
    }
    Ok(result)
}

fn matches_window(tree: &WindowTree, window: &Window) -> bool {
    let bounds_match = tree.bounds[2] > 0
        && tree
            .bounds
            .iter()
            .zip(window.bounds)
            .all(|(a, b)| (i64::from(*a) - i64::from(b)).abs() <= 4);
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
            title: "Toad Computer".into(),
            class: "chromium".into(),
            bounds: [0, 0, 800, 600],
            focused: true,
            pid: Some(100),
            maximized: false,
        };
        let mut tree = WindowTree {
            title: "Toad".into(),
            pid: Some(200),
            bounds: [0, 0, 800, 600],
            nodes: vec![],
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
