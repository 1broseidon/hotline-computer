use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::App;

use super::{ToolResult, action_error, json_text, text};

#[derive(Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    name: String,
    duration: Option<u64>,
    workspace: Option<PathBuf>,
    packages: Option<Vec<String>>,
    flake: Option<String>,
    manifest: Option<Value>,
    #[serde(default)]
    upgrade: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedLogin {
    name: String,
    browser: String,
    created_at: String,
    #[serde(default)]
    last_used_at: String,
    cookies: Value,
    storage: Value,
}

pub async fn call(app: &App, arguments: Value, holder: &str) -> ToolResult {
    let input: Input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    match input.action.as_str() {
        "guide" => json_text(crate::guide::manifest()),
        "catalog" => json_text(crate::workspace::catalog()),
        "prepare" if !input.name.is_empty() => Err(
            "environment presets have been replaced: pass a manifest, or packages or flake, instead of name".into(),
        ),
        "prepare" => {
            let manifest = input
                .manifest
                .map(|value| crate::workspace::manifest_patch(&value))
                .transpose()?;
            json_text(
                crate::workspace::prepare(
                    app,
                    crate::workspace::Request {
                        packages: input.packages,
                        flake: input.flake,
                        manifest,
                        upgrade: input.upgrade,
                    },
                    input.workspace.as_deref().ok_or("workspace is required")?,
                    holder,
                )
                .await?,
            )
        }
        "manifest" => {
            let workspace = input.workspace.as_deref().ok_or(
                "workspace is required: the directory whose .hotline/manifest.json to describe",
            )?;
            let home = app.config.home.canonicalize().map_err(|e| e.to_string())?;
            let workspace = if workspace.is_absolute() {
                workspace.to_path_buf()
            } else {
                home.join(workspace)
            };
            let workspace = workspace
                .canonicalize()
                .map_err(|e| format!("{}: {e}", workspace.display()))?;
            if !workspace.starts_with(&home) {
                return Err("workspace must be under the computer home".into());
            }
            json_text(crate::manifest::describe(app, &workspace).await?)
        }
        "info" => json_text(
            json!({"version":env!("CARGO_PKG_VERSION"),"build":crate::guide::identity(),"architecture":std::env::consts::ARCH,"home":app.config.home,"display":app.config.display,"nixpkgs":crate::workspace::NIXPKGS,"base":crate::manifest::base().map(|b| b.version).ok(),"parallelism":crate::limits::environment(),"catalog":crate::workspace::catalog(),"executables":executables(),"capabilities":crate::tools::NAMES,"secrets":app.secrets.catalog(),"skill_sha256":crate::guide::manifest()["sha256"],"jobs":app.jobs.list().await?,"terminal":"Alacritty","graphics":"Mesa software rendering"}),
        ),
        "control" => control(app, holder, input.duration).await,
        "release" => release(app, holder).await,
        "login_list" => login_list(app).await,
        "snapshot_list" => snapshot_list(app).await,
        "login_save" | "login_load" | "login_delete" | "snapshot_save" | "snapshot_load"
        | "snapshot_delete" => {
            valid_name(&input.name)?;
            let _guard = app.access.mutate(holder).await?;
            match input.action.as_str() {
                "login_save" => login_save(app, &input.name).await,
                "login_load" => login_load(app, &input.name).await,
                "login_delete" => login_delete(app, &input.name).await,
                "snapshot_save" => snapshot_save(app, &input.name).await,
                "snapshot_load" => snapshot_load(app, &input.name).await,
                "snapshot_delete" => snapshot_delete(app, &input.name).await,
                _ => unreachable!(),
            }
        }
        action => Err(action_error(
            "state",
            action,
            &[
                "info",
                "guide",
                "catalog",
                "prepare",
                "manifest",
                "control",
                "release",
                "login_save",
                "login_load",
                "login_list",
                "login_delete",
                "snapshot_save",
                "snapshot_load",
                "snapshot_list",
                "snapshot_delete",
            ],
        )),
    }
}

fn executables() -> std::collections::BTreeMap<&'static str, String> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH").unwrap_or_default();
    [
        "bash",
        "git",
        "curl",
        "python3",
        "nix",
        "chromium",
        "alacritty",
    ]
    .into_iter()
    .filter_map(|name| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|path| {
                path.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
            .map(|path| (name, path.to_string_lossy().into_owned()))
    })
    .collect()
}

async fn control(app: &App, holder: &str, duration: Option<u64>) -> ToolResult {
    let (duration, expires) = app.access.control(holder, duration).await?;
    let expires = expires
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    json_text(json!({"granted":true,"holder":holder,"expires_at":expires,"duration_s":duration}))
}

async fn release(app: &App, holder: &str) -> ToolResult {
    let released = app.access.release(holder).await?;
    let mut value = json!({"released":released,"holder":holder});
    if !released {
        value["detail"] = json!("no control lease was held");
    }
    json_text(value)
}

async fn login_save(app: &App, name: &str) -> ToolResult {
    let directory = app.config.home.join(".hotline/logins");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| error.to_string())?;
    let saved = SavedLogin {
        name: name.to_owned(),
        browser: "chromium".into(),
        created_at: now(),
        last_used_at: String::new(),
        cookies: app.browser.cookies().await?,
        storage: app.browser.local_storage().await?,
    };
    let data = serde_json::to_vec(&saved).map_err(|error| error.to_string())?;
    tokio::fs::write(directory.join(format!("{name}.json")), data)
        .await
        .map_err(|error| error.to_string())?;
    json_text(json!({"saved":true,"name":name}))
}

async fn login_load(app: &App, name: &str) -> ToolResult {
    let path = app
        .config
        .home
        .join(".hotline/logins")
        .join(format!("{name}.json"));
    let data = tokio::fs::read(&path)
        .await
        .map_err(|_| format!("saved login {name:?} not found"))?;
    let mut saved: SavedLogin = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
    app.browser
        .restore(saved.cookies.clone(), saved.storage.clone())
        .await?;
    saved.last_used_at = now();
    tokio::fs::write(
        &path,
        serde_json::to_vec(&saved).map_err(|error| error.to_string())?,
    )
    .await
    .map_err(|error| error.to_string())?;
    json_text(json!({"loaded":true,"name":name}))
}

async fn login_list(app: &App) -> ToolResult {
    let directory = app.config.home.join(".hotline/logins");
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(text("no saved logins"));
        }
        Err(error) => return Err(format!("login_list: {error}")),
    };
    let mut logins = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        if entry.path().extension().and_then(|part| part.to_str()) != Some("json") {
            continue;
        }
        if let Ok(data) = tokio::fs::read(entry.path()).await
            && let Ok(login) = serde_json::from_slice::<SavedLogin>(&data)
        {
            logins.push(json!({"name":login.name,"browser":login.browser,"created_at":login.created_at,"last_used_at":login.last_used_at}));
        }
    }
    if logins.is_empty() {
        Ok(text("no saved logins"))
    } else {
        json_text(logins)
    }
}

async fn login_delete(app: &App, name: &str) -> ToolResult {
    let path = app
        .config
        .home
        .join(".hotline/logins")
        .join(format!("{name}.json"));
    tokio::fs::remove_file(&path)
        .await
        .map_err(|_| format!("saved login {name:?} not found"))?;
    json_text(json!({"deleted":true,"name":name}))
}

fn snapshot_path(app: &App, name: &str) -> PathBuf {
    app.config
        .home
        .join(".hotline/snapshots")
        .join(format!("{name}.tar.gz"))
}

async fn snapshot_save(app: &App, name: &str) -> ToolResult {
    let path = snapshot_path(app, name);
    tokio::fs::create_dir_all(path.parent().expect("snapshot has parent"))
        .await
        .map_err(|error| error.to_string())?;
    tar(&[
        "czf",
        path_str(&path)?,
        "--exclude=.hotline/snapshots",
        "-C",
        path_str(&app.config.home)?,
        ".",
    ])
    .await?;
    let size = tokio::fs::metadata(&path)
        .await
        .map_err(|error| error.to_string())?
        .len();
    if size > 500 * 1024 * 1024 {
        tokio::fs::remove_file(&path).await.ok();
        return Err(format!(
            "snapshot too large: {:.1} MB (max 500 MB)",
            size as f64 / 1_048_576.0
        ));
    }
    json_text(
        json!({"saved":true,"name":name,"size_mb":format!("{:.1}",size as f64/1_048_576.0),"browser_state":true}),
    )
}

async fn snapshot_load(app: &App, name: &str) -> ToolResult {
    if !desktop_fresh(&app.config.home).await? {
        return Err(format!(
            "snapshot_load requires a fresh desktop — user files already exist in {}/",
            app.config.home.display()
        ));
    }
    let path = snapshot_path(app, name);
    if !path.is_file() {
        return Err(format!("snapshot {name:?} not found"));
    }
    tar(&[
        "xzf",
        path_str(&path)?,
        "-C",
        path_str(&app.config.home)?,
        "--overwrite",
    ])
    .await?;
    json_text(json!({"loaded":true,"name":name,"browser_restored":true}))
}

async fn snapshot_list(app: &App) -> ToolResult {
    let directory = app.config.home.join(".hotline/snapshots");
    let mut entries = match tokio::fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(text("no snapshots"));
        }
        Err(error) => return Err(format!("snapshot_list: {error}")),
    };
    let mut snapshots = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(".tar.gz") else {
            continue;
        };
        let size = entry
            .metadata()
            .await
            .map_err(|error| error.to_string())?
            .len();
        snapshots.push(json!({"name":name,"size_mb":size as f64/1_048_576.0}));
    }
    if snapshots.is_empty() {
        Ok(text("no snapshots"))
    } else {
        json_text(snapshots)
    }
}

async fn snapshot_delete(app: &App, name: &str) -> ToolResult {
    tokio::fs::remove_file(snapshot_path(app, name))
        .await
        .map_err(|_| format!("snapshot {name:?} not found"))?;
    json_text(json!({"deleted":true,"name":name}))
}

async fn tar(arguments: &[&str]) -> Result<(), String> {
    let output = tokio::process::Command::new("tar")
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("tar: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "tar: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

async fn desktop_fresh(home: &Path) -> Result<bool, String> {
    let mut entries = tokio::fs::read_dir(home)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| error.to_string())?
    {
        if !entry.file_name().to_string_lossy().starts_with('.') {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name is required".into());
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("name may contain only letters, numbers, '-' and '_'".into());
    }
    Ok(())
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not UTF-8: {}", path.display()))
}

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_escape_the_state_directories() {
        assert!(valid_name("github-work").is_ok());
        assert!(valid_name("../outside").is_err());
    }
}
