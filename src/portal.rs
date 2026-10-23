//! The desktop portal's file chooser, answered by the computer's own picker.
//! Chromium, Electron and GTK apps that ask the portal for a file get
//! `files_ui`'s window instead of GTK's dialog: one look, and a tree an agent
//! can read. `xdg-desktop-portal` routes FileChooser here by the image's
//! `portals.conf`.

use std::collections::HashMap;

use zbus::interface;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::App;
use crate::files_ui::{Answer, Filter, Request, encode, port};

const NAME: &str = "org.freedesktop.impl.portal.desktop.hotline";
const PATH: &str = "/org/freedesktop/portal/desktop";
/// Portal responses: the person chose, or cancelled.
const CHOSEN: u32 = 0;
const CANCELLED: u32 = 1;

type Results = HashMap<String, OwnedValue>;

struct FileChooser {
    app: App,
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    #[zbus(out_args("response", "results"))]
    async fn open_file(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        self.ask(request("open", title, &options)).await
    }

    #[zbus(out_args("response", "results"))]
    async fn save_file(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        self.ask(request("save", title, &options)).await
    }

    /// Saving several files into one folder: the folder is what is picked.
    #[zbus(out_args("response", "results"))]
    async fn save_files(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        let mut request = request("open", title, &options);
        request.directory = true;
        request.multiple = false;
        let names: Vec<String> = options
            .get("files")
            .map(|files| array(files).iter().filter_map(bytes_text).collect())
            .unwrap_or_default();
        let (response, results) = self.ask(request).await;
        let Some(folder) = results
            .get("uris")
            .and_then(|uris| array(uris).first().and_then(string))
        else {
            return (response, results);
        };
        let uris: Vec<String> = names
            .iter()
            .map(|name| format!("{folder}/{}", encode(name, false)))
            .collect();
        (response, uris_result(uris))
    }
}

impl FileChooser {
    async fn ask(&self, request: Request) -> (u32, Results) {
        match self
            .app
            .files
            .pick(request, port(&self.app.config.addr))
            .await
        {
            Answer::Chosen { paths, .. } => (
                CHOSEN,
                uris_result(
                    paths
                        .iter()
                        .map(|path| format!("file://{}", encode(path, true)))
                        .collect(),
                ),
            ),
            Answer::Cancelled => (CANCELLED, Results::new()),
        }
    }
}

fn uris_result(uris: Vec<String>) -> Results {
    match OwnedValue::try_from(Value::from(uris)) {
        Ok(uris) => Results::from([("uris".to_owned(), uris)]),
        Err(_) => Results::new(),
    }
}

/// Serves the file chooser on the session bus for as long as the computer runs.
pub async fn serve(app: App) {
    let connection = zbus::connection::Builder::session()
        .and_then(|builder| builder.name(NAME))
        .and_then(|builder| builder.serve_at(PATH, FileChooser { app }));
    match connection {
        Ok(builder) => match builder.build().await {
            Ok(connection) => {
                std::future::pending::<()>().await;
                drop(connection);
            }
            Err(error) => eprintln!("file chooser portal: {error}"),
        },
        Err(error) => eprintln!("file chooser portal: {error}"),
    }
}

/// What an app asked for, from the portal's options: the flags, the
/// filters, and for a save the name and folder it suggests.
fn request(mode: &'static str, title: String, options: &HashMap<String, OwnedValue>) -> Request {
    let flag = |name: &str| {
        options
            .get(name)
            .is_some_and(|value| matches!(&**value, Value::Bool(true)))
    };
    let filters: Vec<Filter> = options
        .get("filters")
        .map(|f| array(f).iter().filter_map(filter).collect())
        .unwrap_or_default();
    let current = options
        .get("current_filter")
        .and_then(|value| filter(value));
    Request {
        mode,
        title,
        accept_label: options
            .get("accept_label")
            .and_then(|v| string(v))
            .filter(|l| !l.is_empty()),
        multiple: flag("multiple"),
        directory: flag("directory"),
        current_filter: current.and_then(|current| filters.iter().position(|f| *f == current)),
        filters,
        current_name: options
            .get("current_name")
            .and_then(|v| string(v))
            .filter(|n| !n.is_empty()),
        current_folder: options
            .get("current_folder")
            .and_then(|v| bytes_text(v))
            .or_else(|| {
                options
                    .get("current_file")
                    .and_then(|v| bytes_text(v))
                    .and_then(|file| {
                        std::path::Path::new(&file)
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                    })
            }),
    }
}

fn array<'a>(value: &'a Value<'a>) -> &'a [Value<'a>] {
    match value {
        Value::Array(array) => array.inner(),
        Value::Value(inner) => array(inner),
        _ => &[],
    }
}

fn string(value: &Value) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.to_string()),
        Value::Value(inner) => string(inner),
        _ => None,
    }
}

/// A path as the portal sends it: bytes, ending in a NUL.
fn bytes_text(value: &Value) -> Option<String> {
    let bytes: Vec<u8> = array(value)
        .iter()
        .map(|byte| match byte {
            Value::U8(b) => Some(*b),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(bytes).into_owned())
}

/// `(sa(us))`: a name and its rules, each a glob (0) or a MIME type (1).
fn filter(value: &Value) -> Option<Filter> {
    let fields = match value {
        Value::Structure(structure) => structure.fields(),
        Value::Value(inner) => return filter(inner),
        _ => return None,
    };
    let [name, rules] = fields else { return None };
    let mut out = Filter {
        name: string(name)?,
        ..Filter::default()
    };
    for rule in array(rules) {
        let rule = match rule {
            Value::Value(inner) => &**inner,
            other => other,
        };
        let Value::Structure(rule) = rule else {
            continue;
        };
        if let [Value::U32(kind), pattern] = rule.fields() {
            let Some(pattern) = string(pattern) else {
                continue;
            };
            if *kind == 0 {
                out.patterns.push(pattern);
            } else {
                out.mimes.push(pattern);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::StructureBuilder;

    fn owned(value: Value) -> OwnedValue {
        OwnedValue::try_from(value).unwrap()
    }

    fn rule_set(name: &str, rules: &[(u32, &str)]) -> Value<'static> {
        let rules: Vec<Value> = rules
            .iter()
            .map(|(kind, pattern)| {
                Value::from(
                    StructureBuilder::new()
                        .add_field(*kind)
                        .add_field(pattern.to_string())
                        .build()
                        .unwrap(),
                )
            })
            .collect();
        Value::from(
            StructureBuilder::new()
                .add_field(name.to_owned())
                .append_field(Value::Array(zbus::zvariant::Array::from(rules)))
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn the_portals_options_become_what_the_picker_asks() {
        let images = rule_set("Images", &[(0, "*.png"), (0, "*.jpg"), (1, "image/png")]);
        let text = rule_set("Text", &[(0, "*.txt")]);
        let filters = zbus::zvariant::Array::from(vec![images.try_clone().unwrap(), text]);
        let options = HashMap::from([
            ("multiple".to_owned(), owned(Value::Bool(true))),
            ("accept_label".to_owned(), owned(Value::from("Add project"))),
            ("filters".to_owned(), owned(Value::Array(filters))),
            ("current_filter".to_owned(), owned(images)),
            (
                "current_folder".to_owned(),
                owned(Value::from(b"/home/agent/src\0".to_vec())),
            ),
        ]);
        let asked = request("open", "Open Folder".into(), &options);
        assert!(asked.multiple && !asked.directory);
        assert_eq!(asked.accept_label.as_deref(), Some("Add project"));
        assert_eq!(asked.filters.len(), 2);
        assert_eq!(asked.filters[0].patterns, ["*.png", "*.jpg"]);
        assert_eq!(asked.filters[0].mimes, ["image/png"]);
        assert_eq!(asked.current_filter, Some(0));
        assert_eq!(asked.current_folder.as_deref(), Some("/home/agent/src"));
        let saved = request(
            "save",
            "Save".into(),
            &HashMap::from([
                ("current_name".to_owned(), owned(Value::from("notes.md"))),
                (
                    "current_file".to_owned(),
                    owned(Value::from(b"/home/agent/workspace/old.md\0".to_vec())),
                ),
            ]),
        );
        assert_eq!(saved.current_name.as_deref(), Some("notes.md"));
        assert_eq!(
            saved.current_folder.as_deref(),
            Some("/home/agent/workspace")
        );
        assert_eq!(request("open", String::new(), &HashMap::new()).filters, []);
    }
}
