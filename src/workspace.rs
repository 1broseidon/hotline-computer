//! Workspace-owned Nix definitions, prepared through the managed job lifecycle.
use crate::{App, jobs::Start};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const NIXPKGS: &str = "ef34387ddd751e1ab8857adf4676492d32eb24ec";
const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum Definition {
    Packages {
        packages: Vec<String>,
        nixpkgs: String,
    },
    Flake {
        flake: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct Environment {
    format: u32,
    architecture: String,
    definition: Definition,
    env: BTreeMap<String, String>,
}

fn attribute(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-'".contains(&c))
        })
}

fn recipe(packages: &[String], nixpkgs: &str) -> Result<String, String> {
    if packages.is_empty() || packages.len() > 128 || !packages.iter().all(|p| attribute(p)) {
        return Err("packages must contain 1–128 Nixpkgs attribute paths, such as python312 or nodePackages.typescript".into());
    }
    if nixpkgs.len() != 40 || !nixpkgs.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("the saved nixpkgs pin must be a 40-character Git revision".into());
    }
    let packages = packages
        .iter()
        .map(|p| format!("pkgs.{p}"))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        r#"{{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/{nixpkgs}";
  outputs = {{ self, nixpkgs }}: {{
    devShells = nixpkgs.lib.genAttrs [ "aarch64-linux" "x86_64-linux" ] (system:
      let pkgs = import nixpkgs {{ inherit system; }};
      in {{ default = pkgs.mkShell {{
        packages = [ {packages} ];
        hardeningDisable = [ "fortify" "fortify3" ];
        NIX_ENFORCE_PURITY = "0";
      }}; }});
  }};
}}
"#
    ))
}

pub fn catalog() -> Value {
    json!({"version":env!("CARGO_PKG_VERSION"),"nixpkgs":NIXPKGS,
        "sources":["packages","flake"],
        "usage":"state prepare with workspace and either packages (Nixpkgs attribute names) or flake (local directory with optional #devShell). Omit both to reuse .toad/environment-spec.json, or the repository's flake.nix on first use. Wait for the returned job; shell cwd then inherits the prepared environment. No framework presets."})
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = path.with_extension(format!("{}-{nonce}.tmp", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn resolve_workspace(home: &Path, workspace: &Path) -> Result<PathBuf, String> {
    let workspace = if workspace.is_absolute() {
        workspace.to_owned()
    } else {
        home.join(workspace)
    };
    let mut ancestor = workspace.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(ancestor.file_name().ok_or("invalid workspace")?.to_owned());
        ancestor = ancestor.parent().ok_or("invalid workspace")?;
    }
    let mut resolved = ancestor.canonicalize().map_err(|e| e.to_string())?;
    if !resolved.starts_with(home) || missing.iter().any(|part| part == "..") {
        return Err("workspace must be under the computer home".into());
    }
    for part in missing.iter().rev() {
        resolved.push(part);
    }
    std::fs::create_dir_all(&resolved).map_err(|e| e.to_string())?;
    let resolved = resolved.canonicalize().map_err(|e| e.to_string())?;
    if !resolved.starts_with(home) {
        return Err("workspace must be under the computer home".into());
    }
    Ok(resolved)
}

fn flake_path(home: &Path, workspace: &Path, flake: &str) -> Result<(PathBuf, String), String> {
    let (directory, shell) = flake.split_once('#').unwrap_or((flake, ""));
    if directory.contains(['\0', '?']) || (!shell.is_empty() && !attribute(shell)) {
        return Err("flake must be a local directory with an optional #devShell name".into());
    }
    let directory = workspace
        .join(if directory.is_empty() { "." } else { directory })
        .canonicalize()
        .map_err(|e| format!("flake directory: {e}"))?;
    if !directory.starts_with(home) || !directory.join("flake.nix").is_file() {
        return Err("flake must contain flake.nix and be under the computer home".into());
    }
    Ok((directory, shell.to_owned()))
}

fn definition(
    home: &Path,
    workspace: &Path,
    packages: Option<Vec<String>>,
    flake: Option<String>,
) -> Result<Definition, String> {
    if packages.is_some() && flake.is_some() {
        return Err("choose packages or flake, not both".into());
    }
    let saved = workspace.join(".toad/environment-spec.json");
    let definition = if let Some(mut packages) = packages {
        packages.sort();
        packages.dedup();
        // Adding a dependency must not incidentally update all existing dependencies.
        let previous = if saved.exists() {
            Some(read_json::<Definition>(&saved)?)
        } else {
            None
        };
        let nixpkgs = match previous {
            Some(Definition::Packages { nixpkgs, .. }) => nixpkgs,
            _ => NIXPKGS.into(),
        };
        Definition::Packages { packages, nixpkgs }
    } else if let Some(flake) = flake {
        Definition::Flake { flake }
    } else if saved.exists() {
        read_json(&saved)?
    } else if workspace.join("flake.nix").is_file() {
        Definition::Flake { flake: ".".into() }
    } else {
        return Err(
            "provide packages or a local flake; this workspace has no saved environment definition"
                .into(),
        );
    };
    match &definition {
        Definition::Packages { packages, nixpkgs } => {
            recipe(packages, nixpkgs)?;
        }
        Definition::Flake { flake } => {
            flake_path(home, workspace, flake)?;
        }
    }
    Ok(definition)
}

fn cache(home: &Path, workspace: &Path, definition: &Definition) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let key = match definition {
        Definition::Packages { packages, nixpkgs } => recipe(packages, nixpkgs)?,
        Definition::Flake { flake } => format!("{}:{flake}", workspace.display()),
    };
    Ok(home.join(".cache/toad/environments").join(format!(
        "v{FORMAT}-{}-{:x}",
        std::env::consts::ARCH,
        Sha256::digest(key)
    )))
}

fn paths_exist(env: &BTreeMap<String, String>) -> bool {
    env.get("PATH").is_some_and(|path| {
        path.split(':')
            .filter(|p| p.starts_with("/nix/store/"))
            .all(|p| Path::new(p).exists())
    })
}

fn cached(directory: &Path, definition: &Definition) -> Option<Environment> {
    // Repository expressions and shell hooks can change independently of flake.lock.
    // Re-evaluate them on prepare; Nix still reuses downloaded and built packages.
    if matches!(definition, Definition::Flake { .. }) {
        return None;
    }
    let environment: Environment = read_json(&directory.join("environment.json")).ok()?;
    (environment.format == FORMAT
        && environment.architecture == std::env::consts::ARCH
        && &environment.definition == definition
        && paths_exist(&environment.env)
        && directory.join("profile").exists())
    .then_some(environment)
}

fn attach(workspace: &Path, directory: &Path, environment: &Environment) -> Result<(), String> {
    let pending: Definition = read_json(&workspace.join(".toad/environment-request.json"))?;
    if pending != environment.definition {
        return Err(
            "a newer preparation replaced this request; previous environment retained".into(),
        );
    }
    if matches!(environment.definition, Definition::Packages { .. }) {
        let target = workspace.join(".toad/nix");
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        for file in ["flake.nix", "flake.lock"] {
            std::fs::copy(directory.join(file), target.join(file)).map_err(|e| e.to_string())?;
        }
    }
    write_json(
        &workspace.join(".toad/environment-spec.json"),
        &environment.definition,
    )?;
    write_json(&workspace.join(".toad/environment.json"), environment)
}

pub async fn prepare(
    app: &App,
    packages: Option<Vec<String>>,
    flake: Option<String>,
    workspace: &Path,
    holder: &str,
) -> Result<Value, String> {
    let guard = app.access.mutate(holder).await?;
    let home = app.config.home.canonicalize().map_err(|e| e.to_string())?;
    let workspace = resolve_workspace(&home, workspace)?;
    let definition = definition(&home, &workspace, packages, flake)?;
    let directory = cache(&home, &workspace, &definition)?;
    write_json(
        &workspace.join(".toad/environment-request.json"),
        &definition,
    )?;
    if let Some(environment) = cached(&directory, &definition) {
        attach(&workspace, &directory, &environment)?;
        return Ok(
            json!({"ready":true,"cached":true,"workspace":workspace,"definition":definition}),
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
                    serde_json::to_string(&definition).map_err(|e| e.to_string())?,
                    workspace.to_string_lossy().into_owned(),
                    home.to_string_lossy().into_owned(),
                ],
                label: Some("Prepare workspace environment".into()),
                // A broken or missing old environment must not prevent its own repair.
                skip_workspace_environment: true,
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
        json!({"ready":false,"cached":false,"workspace":workspace,"definition":definition,"job":job,"observer_error":observer_error}),
    )
}

pub async fn build(home: &Path, specification: &str, workspace: &Path) -> Result<(), String> {
    let home = home.canonicalize().map_err(|e| e.to_string())?;
    let workspace = resolve_workspace(&home, workspace)?;
    let definition: Definition =
        serde_json::from_str(specification).map_err(|e| format!("environment definition: {e}"))?;
    let directory = cache(&home, &workspace, &definition)?;
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
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
    if let Some(environment) = cached(&directory, &definition) {
        attach(&workspace, &directory, &environment)?;
        println!("Ready: {} (cached)", workspace.display());
        return Ok(());
    }
    println!(
        "Preparing {}; Nix download and build progress follow.",
        workspace.display()
    );
    let mut command = tokio::process::Command::new("nix");
    let capture = directory.join("captured-env.json");
    match &definition {
        Definition::Packages { packages, nixpkgs } => {
            std::fs::write(directory.join("flake.nix"), recipe(packages, nixpkgs)?)
                .map_err(|e| e.to_string())?;
            command
                .args(["print-dev-env", "--json", "--profile"])
                .arg(directory.join("profile"))
                .arg(format!("path:{}", directory.display()));
        }
        Definition::Flake { flake } => {
            let (path, shell) = flake_path(&home, &workspace, flake)?;
            // Existing locks must not change silently. A first preparation may create one.
            command.arg("develop");
            if path.join("flake.lock").exists() {
                command.arg("--no-update-lock-file");
            }
            command
                .arg("--profile")
                .arg(directory.join("profile"))
                .arg(format!("path:{}#{shell}", path.display()))
                .args([
                    "--command",
                    "/usr/bin/python3",
                    "-I",
                    "-c",
                    "import json,os,sys; open(sys.argv[1],'w').write(json.dumps(dict(os.environ)))",
                ])
                .arg(&capture);
        }
    }
    let stdout = if matches!(definition, Definition::Packages { .. }) {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::inherit()
    };
    let output = command
        .current_dir(&workspace)
        .stdout(stdout)
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
    let mut env = BTreeMap::new();
    if matches!(definition, Definition::Packages { .. }) {
        let result: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        for (name, variable) in result["variables"]
            .as_object()
            .ok_or("Nix did not return environment variables")?
        {
            if variable["type"] == "exported"
                && let Some(value) = variable["value"].as_str()
            {
                env.insert(name.clone(), value.to_owned());
            }
        }
    } else {
        env = read_json(&capture)?;
        std::fs::remove_file(&capture).map_err(|e| e.to_string())?;
        // Keep exported hook changes, without persisting the service's inherited secrets.
        env.retain(|name, value| {
            name == "PATH" || std::env::var(name).ok().as_deref() != Some(value.as_str())
        });
    }
    for name in [
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
        "DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "TOAD_COMPUTER_TOKEN",
        "_",
    ] {
        env.remove(name);
    }
    env.entry("PATH".into())
        .and_modify(|p| p.push_str(":/usr/local/bin:/usr/bin:/bin"));
    let environment = Environment {
        format: FORMAT,
        architecture: std::env::consts::ARCH.into(),
        definition,
        env,
    };
    write_json(&directory.join("environment.json"), &environment)?;
    attach(&workspace, &directory, &environment)?;
    println!(
        "Ready: {}. Shell commands in this directory now inherit the environment.",
        workspace.display()
    );
    Ok(())
}

pub fn environment(home: &Path, cwd: &Path) -> Result<BTreeMap<String, String>, String> {
    let home = home.canonicalize().map_err(|e| e.to_string())?;
    for directory in cwd.ancestors().take_while(|dir| dir.starts_with(&home)) {
        let path = directory.join(".toad/environment.json");
        if path.exists() {
            let saved: Value = read_json(&path)?;
            // The old preset format also contains usable exported variables. Preserve
            // it across an image update; explicit prepare adopts the new definition.
            if let Some(format) = saved.get("format") {
                if format != FORMAT || saved["architecture"] != std::env::consts::ARCH {
                    return Err("workspace environment format or architecture changed; run state prepare again".into());
                }
            } else if saved.get("profile").is_none() {
                return Err("unrecognized workspace environment; run state prepare again".into());
            }
            let mut env: BTreeMap<String, String> =
                serde_json::from_value(saved["env"].clone()).map_err(|e| e.to_string())?;
            if !paths_exist(&env) {
                return Err(
                    "workspace Nix store paths are missing; run state prepare again".into(),
                );
            }
            env.insert("NIX_BUILD_TOP".into(), cwd.to_string_lossy().into_owned());
            return Ok(env);
        }
    }
    Ok(BTreeMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(home: &Path) -> App {
        App::new(crate::Config {
            addr: "127.0.0.1:0".into(),
            token: None,
            home: home.to_owned(),
            display: ":99".into(),
            screen: "800x600".into(),
        })
    }

    #[test]
    fn workspace_pins_survive_dependency_changes_and_unsafe_attributes_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let pin = "0123456789012345678901234567890123456789";
        write_json(
            &home.join(".toad/environment-spec.json"),
            &Definition::Packages {
                packages: vec!["hello".into()],
                nixpkgs: pin.into(),
            },
        )
        .unwrap();
        let selected = definition(
            &home,
            &home,
            Some(vec!["jq".into(), "hello".into(), "jq".into()]),
            None,
        )
        .unwrap();
        assert_eq!(
            selected,
            Definition::Packages {
                packages: vec!["hello".into(), "jq".into()],
                nixpkgs: pin.into()
            }
        );
        assert!(
            definition(
                &home,
                &home,
                Some(vec!["hello]; builtins.abort \"oops\"".into()]),
                None
            )
            .is_err()
        );
        assert!(definition(&home, &home, Some(vec!["hello".into()]), Some(".".into())).is_err());
        assert!(
            recipe(
                &["python312Packages.requests".into(), "pkg-config".into()],
                NIXPKGS
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn preparation_obeys_the_lease_and_home_boundary_through_the_state_tool() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let app = app(&home);
        crate::tools::call(&app, "state", json!({"action":"control"}), "alice")
            .await
            .unwrap();
        let result = crate::tools::call(
            &app,
            "state",
            json!({"action":"prepare","packages":["hello"],"workspace":"not-created"}),
            "bob",
        )
        .await;
        assert!(result.unwrap_err().contains("alice"));
        assert!(!home.join("not-created").exists());
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.join("escape")).unwrap();
        let result = crate::tools::call(
            &app,
            "state",
            json!({"action":"prepare","packages":["hello"],"workspace":"escape/new"}),
            "alice",
        )
        .await;
        assert!(result.unwrap_err().contains("under the computer home"));
        assert!(!outside.path().join("new").exists());
        let result = crate::tools::call(
            &app,
            "state",
            json!({"action":"prepare","name":"rust-tauri","workspace":"."}),
            "alice",
        )
        .await;
        assert!(result.unwrap_err().contains("packages or flake"));
    }

    #[test]
    fn flakes_use_the_requested_local_shell_and_cannot_escape_home() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        std::fs::create_dir(home.join("nix")).unwrap();
        std::fs::write(home.join("nix/flake.nix"), "{}").unwrap();
        let (path, shell) = flake_path(&home, &home, "nix#dev").unwrap();
        assert_eq!(path, home.join("nix"));
        assert_eq!(shell, "dev");
        assert!(flake_path(&home, &home, "nix#--impure").is_err());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("flake.nix"), "{}").unwrap();
        assert!(flake_path(&home, &home, outside.path().to_str().unwrap()).is_err());
    }

    #[tokio::test]
    async fn a_cached_definition_attaches_and_shell_jobs_inherit_it() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let workspace = home.join("project");
        std::fs::create_dir_all(workspace.join("child")).unwrap();
        let definition = Definition::Packages {
            packages: vec!["hello".into()],
            nixpkgs: NIXPKGS.into(),
        };
        let directory = cache(&home, &workspace, &definition).unwrap();
        std::fs::create_dir_all(directory.join("profile")).unwrap();
        std::fs::write(
            directory.join("flake.nix"),
            recipe(&["hello".into()], NIXPKGS).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.join("flake.lock"), "{}").unwrap();
        let environment = Environment {
            format: FORMAT,
            architecture: std::env::consts::ARCH.into(),
            definition,
            env: BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("WORKSPACE_PROOF".into(), "inherited".into()),
            ]),
        };
        write_json(&directory.join("environment.json"), &environment).unwrap();
        let app = app(&home);
        let result = prepare(&app, Some(vec!["hello".into()]), None, &workspace, "tester")
            .await
            .unwrap();
        assert_eq!(result["ready"], true);
        assert_eq!(result["cached"], true);
        assert!(workspace.join(".toad/nix/flake.lock").exists());
        let job = app
            .jobs
            .start(
                Start {
                    command: "sh".into(),
                    args: vec!["-c".into(), "test \"$WORKSPACE_PROOF\" = inherited".into()],
                    cwd: Some(workspace.join("child").to_string_lossy().into()),
                    ..Start::default()
                },
                "tester",
            )
            .await
            .unwrap();
        let done = app.jobs.wait(&job.id, 5000).await.unwrap();
        assert_eq!(done.exit_code, Some(0));
        assert_eq!(
            prepare(&app, None, None, &workspace, "tester")
                .await
                .unwrap()["cached"],
            true
        );
    }

    #[test]
    fn image_versions_do_not_invalidate_existing_environment_but_missing_store_paths_do() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let path = home.join(".toad/environment.json");
        write_json(&path, &json!({"version":"0.4.0","profile":"node","env":{"PATH":"/usr/bin:/bin","PROJECT_VALUE":"kept"}})).unwrap();
        assert_eq!(environment(&home, &home).unwrap()["PROJECT_VALUE"], "kept");
        write_json(&path, &json!({"version":"0.4.0","profile":"node","env":{"PATH":"/nix/store/this-path-does-not-exist/bin"}})).unwrap();
        assert!(
            environment(&home, &home)
                .unwrap_err()
                .contains("store paths are missing")
        );
    }
}
