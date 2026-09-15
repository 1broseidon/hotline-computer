//! A release-pinned Nix catalog produces reusable environments, never shell guesses.
use crate::{App, jobs::Start};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const NIXPKGS: &str = "ef34387ddd751e1ab8857adf4676492d32eb24ec";
const VERSION: &str = env!("CARGO_PKG_VERSION");
#[derive(Clone, Serialize, Deserialize)]
pub struct Environment {
    pub version: String,
    pub revision: String,
    pub profile: String,
    #[serde(default)]
    pub recipe: String,
    pub env: BTreeMap<String, String>,
}

fn packages(profile: &str) -> Result<&'static str, String> {
    match profile {
        "python" => Ok("python312 uv"),
        "go" => Ok("go gopls"),
        "node" => Ok("nodejs bun pnpm"),
        "rust" => Ok("rustc cargo clang cmake perl pkg-config openssl"),
        "rust-tauri" => Ok(
            "rustc cargo clang cmake perl pkg-config gnumake cargo-tauri nodejs bun openssl gtk3 webkitgtk_4_1 libsoup_3 libayatana-appindicator librsvg glib-networking gsettings-desktop-schemas mesa libglvnd",
        ),
        _ => Err("unknown environment; choose python, go, node, rust, or rust-tauri".into()),
    }
}
fn recipe(profile: &str) -> Result<String, String> {
    let packages = packages(profile)?;
    let graphics = if profile == "rust-tauri" {
        r#"LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [ gtk3 webkitgtk_4_1 libsoup_3 libayatana-appindicator librsvg mesa libglvnd ]); GIO_EXTRA_MODULES = "${pkgs.glib-networking}/lib/gio/modules"; XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share:${pkgs.gtk3}/share:/usr/share";"#
    } else {
        ""
    };
    Ok(format!(
        r#"{{ inputs.nixpkgs.url = "github:NixOS/nixpkgs/{NIXPKGS}"; outputs = {{self,nixpkgs}}: {{ devShells = nixpkgs.lib.genAttrs ["aarch64-linux" "x86_64-linux"] (system: let pkgs=import nixpkgs {{inherit system;}}; in {{default=pkgs.mkShell {{packages=with pkgs;[{packages}]; hardeningDisable = ["fortify" "fortify3"]; NIX_ENFORCE_PURITY = "0"; {graphics} }};}}); }}; }}"#
    ))
}
fn recipe_hash(profile: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(recipe(profile)?)))
}
pub fn catalog() -> Value {
    json!({"version":VERSION,"nixpkgs":NIXPKGS,"profiles":[
    {"name":"python","tools":["python3.12","uv"]},{"name":"go","tools":["go","gopls"]},
    {"name":"node","tools":["node","bun","pnpm"]},{"name":"rust","tools":["rustc","cargo","clang","pkg-config","openssl"]},
    {"name":"rust-tauri","tools":["Rust","Tauri CLI","Node","Bun","GTK3","WebKitGTK 4.1","AppIndicator","Mesa"]}],
    "usage":"state prepare with name and workspace; wait for its job to exit successfully, then shell commands with cwd inside that workspace inherit its environment automatically."})
}
fn cache(home: &Path, profile: &str) -> PathBuf {
    home.join(".cache/toad/environments")
        .join(format!("{VERSION}-{}-{profile}", std::env::consts::ARCH))
}
fn load(path: &Path) -> Result<Environment, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
fn cached(home: &Path, profile: &str) -> Option<Environment> {
    let environment = load(&cache(home, profile).join("environment.json")).ok()?;
    if environment.version != VERSION
        || environment.revision != NIXPKGS
        || environment.profile != profile
        || environment.recipe != recipe_hash(profile).ok()?
    {
        return None;
    }
    let path = environment.env.get("PATH")?;
    if !path
        .split(':')
        .filter(|p| p.starts_with("/nix/store/"))
        .all(|p| Path::new(p).exists())
    {
        return None;
    }
    Some(environment)
}
fn write_environment(path: &Path, environment: &Environment) -> Result<(), String> {
    let parent = path.parent().ok_or("environment path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(environment).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}
fn attach(workspace: &Path, environment: &Environment) -> Result<(), String> {
    write_environment(&workspace.join(".toad/environment.json"), environment)
}

pub async fn prepare(
    app: &App,
    profile: &str,
    workspace: &Path,
    holder: &str,
) -> Result<Value, String> {
    packages(profile)?;
    let guard = app.access.mutate(holder).await?;
    let home = app.config.home.canonicalize().map_err(|e| e.to_string())?;
    let workspace = if workspace.is_absolute() {
        workspace.to_owned()
    } else {
        home.join(workspace)
    };
    // Resolve the existing parent before creating directories, including symlinks.
    let mut ancestor = workspace.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(ancestor.file_name().ok_or("invalid workspace")?.to_owned());
        ancestor = ancestor.parent().ok_or("invalid workspace")?;
    }
    let mut resolved = ancestor.canonicalize().map_err(|e| e.to_string())?;
    if !resolved.starts_with(&home) || missing.iter().any(|part| part == "..") {
        return Err("workspace must be under the computer home".into());
    }
    for part in missing.iter().rev() {
        resolved.push(part);
    }
    std::fs::create_dir_all(&resolved).map_err(|e| e.to_string())?;
    let workspace = resolved.canonicalize().map_err(|e| e.to_string())?;
    if !workspace.starts_with(&home) {
        return Err("workspace must be under the computer home".into());
    }
    if let Some(environment) = cached(&home, profile) {
        attach(&workspace, &environment)?;
        return Ok(
            json!({"ready":true,"cached":true,"workspace":workspace,"profile":profile,"version":VERSION}),
        );
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let job = app
        .jobs
        .start(
            Start {
                command: executable.to_string_lossy().into_owned(),
                args: vec![
                    "prepare".into(),
                    profile.into(),
                    workspace.to_string_lossy().into_owned(),
                    home.to_string_lossy().into_owned(),
                ],
                label: Some(format!("Prepare {profile} workspace")),
                ..Start::default()
            },
            holder,
        )
        .await?;
    drop(guard);
    let observer_error = if app.display.is_some() {
        app.observer.show(false).await.err()
    } else {
        None
    };
    Ok(
        json!({"ready":false,"cached":false,"workspace":workspace,"profile":profile,"job":job,"observer_error":observer_error}),
    )
}

pub async fn build(home: &Path, profile: &str, workspace: &Path) -> Result<(), String> {
    packages(profile)?;
    if let Some(environment) = cached(home, profile) {
        attach(workspace, &environment)?;
        println!("Ready: {} ({profile}, cached)", workspace.display());
        return Ok(());
    }
    let directory = cache(home, profile);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| e.to_string())?;
    // flock lets a second prepare share the first build, including after cancellation.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join("lock"))
        .map_err(|e| e.to_string())?;
    use std::os::fd::AsRawFd;
    loop {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if let Some(environment) = cached(home, profile) {
        attach(workspace, &environment)?;
        println!("Ready: {} ({profile}, cached)", workspace.display());
        return Ok(());
    }
    let flake = recipe(profile)?;
    tokio::fs::write(directory.join("flake.nix"), flake)
        .await
        .map_err(|e| e.to_string())?;
    println!("Preparing {profile} from Nixpkgs {NIXPKGS}; downloads and build progress follow.");
    let output = tokio::process::Command::new("nix")
        .args(["print-dev-env", "--json", "--profile"])
        .arg(directory.join("profile"))
        .arg(&directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("nix: {e}"))?
        .wait_with_output()
        .await
        .map_err(|e| format!("nix: {e}"))?;
    if !output.status.success() {
        return Err(format!("Nix preparation failed: {}", output.status));
    }
    let result: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let mut env = BTreeMap::new();
    for (name, variable) in result["variables"]
        .as_object()
        .ok_or("Nix did not return environment variables")?
    {
        if variable["type"] != "exported"
            || [
                "HOME",
                "PWD",
                "OLDPWD",
                "TMP",
                "TMPDIR",
                "TEMP",
                "TEMPDIR",
                "NIX_BUILD_TOP",
                "NIX_LOG_FD",
                "SHLVL",
                "SHELL",
            ]
            .contains(&name.as_str())
        {
            continue;
        }
        if let Some(value) = variable["value"].as_str() {
            env.insert(name.clone(), value.to_owned());
        }
    }
    env.entry("PATH".into())
        .and_modify(|p| p.push_str(":/usr/local/bin:/usr/bin:/bin"));
    env.insert("CARGO_BUILD_JOBS".into(), "2".into());
    env.insert("CARGO_PROFILE_DEV_DEBUG".into(), "0".into());
    env.insert("CARGO_INCREMENTAL".into(), "0".into());
    let environment = Environment {
        version: VERSION.into(),
        revision: NIXPKGS.into(),
        profile: profile.into(),
        recipe: recipe_hash(profile)?,
        env,
    };
    write_environment(&directory.join("environment.json"), &environment)?;
    attach(workspace, &environment)?;
    println!(
        "Ready: {} ({profile}). Shell commands in this directory now inherit the environment.",
        workspace.display()
    );
    Ok(())
}

pub fn environment(home: &Path, cwd: &Path) -> Result<BTreeMap<String, String>, String> {
    let home = home.canonicalize().map_err(|e| e.to_string())?;
    for directory in cwd.ancestors().take_while(|dir| dir.starts_with(&home)) {
        let path = directory.join(".toad/environment.json");
        if path.exists() {
            let environment = load(&path)?;
            if environment.version != VERSION
                || environment.revision != NIXPKGS
                || environment.recipe != recipe_hash(&environment.profile)?
            {
                return Err("workspace environment belongs to another computer release; run state prepare again".into());
            }
            let mut env = environment.env;
            env.insert("NIX_BUILD_TOP".into(), cwd.to_string_lossy().into_owned());
            return Ok(env);
        }
    }
    Ok(BTreeMap::new())
}
