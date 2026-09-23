//! The project manifest: what a workspace needs, and the names of what it runs.
//!
//! Three layers compose into one environment. The image's base (its Nixpkgs
//! pin, the platforms and services it knows how to provide) comes first, the
//! repository's `.hotline/manifest.json` next, and whatever the teammate
//! passes to `state prepare` is merged into that file last. Lists concatenate,
//! maps merge, and scalars are last-wins. Builds, tests and launches are then
//! named entries the teammate runs, not shell strings it invents each time.
use crate::{App, jobs::Start};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Where a repository keeps its manifest, relative to the workspace.
pub const FILE: &str = ".hotline/manifest.json";
const ACTIVATION: &str = ".hotline/state/activation.json";
const EMBEDDED_BASE: &str = include_str!("../assets/base.json");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvValue {
    /// Replaces the variable. `$WORKSPACE` expands to the workspace directory.
    Text(String),
    /// Extends a search path: entries go in front of what is already there,
    /// relative ones resolved against the workspace. PATH is always a list.
    List(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    #[serde(default = "enabled")]
    pub enable: bool,
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// Run once per workspace after its first successful preparation, in name order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub create: BTreeMap<String, Vec<String>>,
    /// Started after every preparation unless already running: watchers, dev servers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub start: BTreeMap<String, Vec<String>>,
}

impl Hooks {
    fn is_empty(&self) -> bool {
        self.create.is_empty() && self.start.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A build, test or other command that finishes.
    #[default]
    Task,
    /// A native app; it opens windows on the desktop.
    Desktop,
    /// A server; it receives a free port as `$PORT` and the answer carries its URL.
    Web,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Nixpkgs attribute names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    /// A repository flake to take the environment from instead of packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flake: Option<String>,
    /// Runtime support the image provides by name, such as gl, gtk or webkit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platform: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, EnvValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub services: BTreeMap<String, Service>,
    #[serde(default, skip_serializing_if = "Hooks::is_empty")]
    pub hooks: Hooks,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runs: BTreeMap<String, Run>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub packages: Vec<String>,
    /// Packages whose libraries are loaded at run time (dlopen), so they go on
    /// the library path. Linked libraries already resolve through Nix's rpath.
    #[serde(default)]
    pub libraries: Vec<String>,
    /// `${attribute}` expands to that package's store path.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDefinition {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub packages: Vec<String>,
    pub command: Vec<String>,
    /// Exported to every job in the workspace; `$SERVICE_DIR` expands.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// What the image provides. An image release is a new one of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base {
    pub version: String,
    pub nixpkgs: String,
    /// What the pinned Nixpkgs hashes to. With it, a first preparation locks
    /// the input without asking GitHub and takes the source from a cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nixpkgs_lock: Option<Lock>,
    #[serde(default)]
    pub manifest: Manifest,
    #[serde(default)]
    pub platforms: BTreeMap<String, Platform>,
    #[serde(default)]
    pub services: BTreeMap<String, ServiceDefinition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Lock {
    pub nar_hash: String,
    pub last_modified: u64,
}

/// The flake.lock a generated flake on this pin starts from, when the base knows its hash.
pub fn lock(base: &Base, nixpkgs: &str) -> Option<String> {
    let lock = base
        .nixpkgs_lock
        .as_ref()
        .filter(|_| base.nixpkgs == nixpkgs)?;
    let source = json!({"owner": "NixOS", "repo": "nixpkgs", "rev": nixpkgs, "type": "github"});
    let mut locked = source.clone();
    locked["narHash"] = json!(lock.nar_hash);
    locked["lastModified"] = json!(lock.last_modified);
    serde_json::to_string_pretty(&json!({
        "nodes": {
            "nixpkgs": {"locked": locked, "original": source},
            "root": {"inputs": {"nixpkgs": "nixpkgs"}}
        },
        "root": "root",
        "version": 7
    }))
    .ok()
}

/// A workspace's environment definition: the base it was composed against,
/// the repository layer as read, and the merged result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composed {
    pub base: Base,
    pub repository: Manifest,
    pub manifest: Manifest,
}

/// The image's base: `/etc/hotline-computer/base.json`, or the copy compiled in.
pub fn base() -> Result<Base, String> {
    let path = std::env::var_os("HOTLINE_COMPUTER_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/hotline-computer/base.json"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => EMBEDDED_BASE.to_owned(),
    };
    serde_json::from_str(&text).map_err(|e| format!("image base {}: {e}", path.display()))
}

pub fn read(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse(text: &str) -> Result<Manifest, String> {
    serde_json::from_str(text).map_err(|e| format!("{e}. {SHAPE}"))
}

/// Said with every shape error, because the error is the documentation.
pub const SHAPE: &str = "A manifest is a JSON object with any of: packages (Nixpkgs attribute names), flake (a local flake instead of packages), platform (gl, gtk, gtk4, qt, native, webkit, prebuilt), env (NAME: \"value\", or NAME: [\"path\", ...] to extend a search path), services (NAME: {\"enable\": true}), hooks ({\"create\": {NAME: [argv]}, \"start\": {NAME: [argv]}}), runs (NAME: {\"command\": [argv], \"cwd\": \"subdir\", \"kind\": \"task\" | \"desktop\" | \"web\", \"env\": {}, \"label\": \"...\"}). state manifest shows an example and what this image offers.";

fn concat(mut first: Vec<String>, second: Vec<String>) -> Vec<String> {
    for item in second {
        if !first.contains(&item) {
            first.push(item);
        }
    }
    first
}

/// Layers `upper` over `lower`: lists concatenate, maps merge, scalars are last-wins.
pub fn merge(lower: Manifest, upper: Manifest) -> Manifest {
    let mut env = lower.env;
    for (name, value) in upper.env {
        let merged = match (env.remove(&name), value) {
            (Some(EnvValue::List(a)), EnvValue::List(b)) => EnvValue::List(concat(a, b)),
            (_, value) => value,
        };
        env.insert(name, merged);
    }
    let mut services = lower.services;
    services.extend(upper.services);
    let mut hooks = lower.hooks;
    hooks.create.extend(upper.hooks.create);
    hooks.start.extend(upper.hooks.start);
    let mut runs = lower.runs;
    runs.extend(upper.runs);
    Manifest {
        packages: concat(lower.packages, upper.packages),
        flake: upper.flake.or(lower.flake),
        platform: concat(lower.platform, upper.platform),
        env,
        services,
        hooks,
        runs,
    }
}

fn name(value: &str) -> bool {
    (1..=40).contains(&value.len())
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

fn variable(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn offered<T>(map: &BTreeMap<String, T>) -> String {
    let names: Vec<&str> = map.keys().map(String::as_str).collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

fn relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.contains('\0')
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn argv(what: &str, command: &[String]) -> Result<(), String> {
    if command.first().is_none_or(|c| c.is_empty()) {
        return Err(format!(
            "{what} needs a command as an argv array, such as [\"cargo\", \"build\"]"
        ));
    }
    if command.len() > 256 || command.iter().any(|a| a.contains('\0')) {
        return Err(format!("{what} has an unusable command"));
    }
    Ok(())
}

/// Composes the base with a repository layer and checks the result is usable.
pub fn compose(base: Base, repository: Manifest) -> Result<Composed, String> {
    let manifest = merge(base.manifest.clone(), repository.clone());
    let composed = Composed {
        base,
        repository,
        manifest,
    };
    validate(&composed)?;
    Ok(composed)
}

fn validate(composed: &Composed) -> Result<(), String> {
    let base = &composed.base;
    let m = &composed.manifest;
    if base.nixpkgs.len() != 40 || !base.nixpkgs.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("the image base's nixpkgs pin must be a 40-character Git revision".into());
    }
    for package in &m.packages {
        if !crate::workspace::attribute(package) {
            return Err(format!(
                "package {package:?} is not a Nixpkgs attribute path, such as python312 or nodePackages.typescript"
            ));
        }
    }
    for platform in &m.platform {
        if !base.platforms.contains_key(platform) {
            return Err(format!(
                "unknown platform {platform:?}; this image offers {}",
                offered(&base.platforms)
            ));
        }
    }
    for service in m.services.keys() {
        if !base.services.contains_key(service) {
            return Err(format!(
                "unknown service {service:?}; this image offers {}",
                offered(&base.services)
            ));
        }
    }
    if m.flake.is_some()
        && (!m.packages.is_empty()
            || !m.platform.is_empty()
            || enabled_services(composed).next().is_some())
    {
        return Err("flake replaces packages: with a flake, declare packages, platform and services in the flake itself, or drop flake and list them here".into());
    }
    if packages(composed).len() > 128 {
        return Err(
            "a manifest may bring at most 128 packages, including its platforms and services"
                .into(),
        );
    }
    for (key, value) in &m.env {
        if !variable(key) {
            return Err(format!("env name {key:?} is not a shell variable name"));
        }
        if key == "PATH" && !matches!(value, EnvValue::List(_)) {
            return Err(
                "env PATH must be a list of directories; it extends the path, never replaces it"
                    .into(),
            );
        }
    }
    for (label, hooks) in [("create", &m.hooks.create), ("start", &m.hooks.start)] {
        for (hook, command) in hooks {
            if !name(hook) {
                return Err(format!(
                    "hook name {hook:?} must be 1-40 letters, digits, dots, dashes or underscores"
                ));
            }
            argv(&format!("hooks.{label}.{hook}"), command)?;
        }
    }
    for (run, entry) in &m.runs {
        if !name(run) {
            return Err(format!(
                "run name {run:?} must be 1-40 letters, digits, dots, dashes or underscores"
            ));
        }
        argv(&format!("runs.{run}"), &entry.command)?;
        if let Some(cwd) = &entry.cwd
            && !relative(cwd)
        {
            return Err(format!(
                "runs.{run}.cwd must be a directory inside the workspace, relative to it"
            ));
        }
        if let Some(bad) = entry.env.keys().find(|k| !variable(k)) {
            return Err(format!(
                "runs.{run}.env name {bad:?} is not a shell variable name"
            ));
        }
    }
    Ok(())
}

fn enabled_services(composed: &Composed) -> impl Iterator<Item = (&String, &ServiceDefinition)> {
    composed
        .manifest
        .services
        .iter()
        .filter(|(_, s)| s.enable)
        .filter_map(|(name, _)| composed.base.services.get_key_value(name))
}

/// Platforms in dependency order, each once.
pub fn platforms(composed: &Composed) -> Vec<(&String, &Platform)> {
    fn visit<'a>(
        base: &'a Base,
        name: &str,
        seen: &mut Vec<&'a String>,
        out: &mut Vec<(&'a String, &'a Platform)>,
    ) {
        let Some((key, platform)) = base.platforms.get_key_value(name) else {
            return;
        };
        if seen.contains(&key) {
            return;
        }
        seen.push(key);
        for required in &platform.requires {
            visit(base, required, seen, out);
        }
        out.push((key, platform));
    }
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for name in &composed.manifest.platform {
        visit(&composed.base, name, &mut seen, &mut out);
    }
    out
}

/// Every Nixpkgs attribute the environment brings, sorted.
pub fn packages(composed: &Composed) -> Vec<String> {
    let mut all = composed.manifest.packages.clone();
    for (_, platform) in platforms(composed) {
        all.extend(platform.packages.iter().cloned());
        all.extend(platform.libraries.iter().cloned());
    }
    for (_, service) in enabled_services(composed) {
        all.extend(service.packages.iter().cloned());
    }
    all.sort();
    all.dedup();
    all
}

fn nix_string(value: &str) -> Result<String, String> {
    // ${attribute} names a package's store path; every other character is literal.
    let mut out = String::from("\"");
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let (before, after) = rest.split_at(start);
        out.push_str(&escape(before));
        let Some(end) = after.find('}') else {
            return Err(format!("unterminated ${{...}} in {value:?}"));
        };
        let attribute = &after[2..end];
        if !crate::workspace::attribute(attribute) {
            return Err(format!("${{{attribute}}} is not a Nixpkgs attribute"));
        }
        out.push_str(&format!("${{pkgs.{attribute}}}"));
        rest = &after[end + 1..];
    }
    out.push_str(&escape(rest));
    out.push('"');
    Ok(out)
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

fn list(names: &[String]) -> String {
    names
        .iter()
        .map(|p| format!("pkgs.{p}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The flake that realises a composed manifest. Platforms derive their runtime
/// environment from the packages themselves: GSettings schemas and data from
/// each package's share directory, GIO modules from its lib, and the library
/// path from the packages a platform names as loaded at run time.
pub fn render(composed: &Composed) -> Result<String, String> {
    validate(composed)?;
    let packages = packages(composed);
    if packages.is_empty() {
        return Err(
            "the manifest brings no packages; add packages or a platform, or a flake".into(),
        );
    }
    let platforms = platforms(composed);
    let mut platform_packages: Vec<String> = Vec::new();
    let mut libraries: Vec<String> = Vec::new();
    let mut env: BTreeMap<&str, &str> = BTreeMap::new();
    for (_, platform) in &platforms {
        platform_packages.extend(platform.packages.iter().cloned());
        platform_packages.extend(platform.libraries.iter().cloned());
        libraries.extend(platform.libraries.iter().cloned());
        for (k, v) in &platform.env {
            env.insert(k, v);
        }
    }
    platform_packages.sort();
    platform_packages.dedup();
    libraries.sort();
    libraries.dedup();
    let mut runtime = String::new();
    if !platforms.is_empty() {
        runtime.push_str(
            "        XDG_DATA_DIRS = lib.concatStringsSep \":\" ((map (p: \"${p}/share/gsettings-schemas/${p.name}\") platform) ++ (map (p: \"${p}/share\") platform) ++ [ \"/usr/local/share\" \"/usr/share\" ]);\n        GIO_EXTRA_MODULES = lib.concatMapStringsSep \":\" (p: \"${p}/lib/gio/modules\") platform;\n",
        );
        if !libraries.is_empty() {
            runtime.push_str(&format!(
                "        LD_LIBRARY_PATH = lib.makeLibraryPath [ {} ];\n",
                list(&libraries)
            ));
        }
        for (k, v) in env {
            if !variable(k) {
                return Err(format!(
                    "platform env name {k:?} is not a shell variable name"
                ));
            }
            runtime.push_str(&format!("        {k} = {};\n", nix_string(v)?));
        }
    }
    Ok(format!(
        r#"{{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/{nixpkgs}";
  outputs = {{ self, nixpkgs }}: {{
    devShells = nixpkgs.lib.genAttrs [ "aarch64-linux" "x86_64-linux" ] (system:
      let
        pkgs = import nixpkgs {{ inherit system; }};
        lib = pkgs.lib;
        platform = [ {platform} ];
      in {{ default = pkgs.mkShell {{
        packages = [ {packages} ];
        hardeningDisable = [ "fortify" "fortify3" ];
        NIX_ENFORCE_PURITY = "0";
{runtime}      }}; }});
  }};
}}
"#,
        nixpkgs = composed.base.nixpkgs,
        platform = list(&platform_packages),
        packages = list(&packages),
    ))
}

fn service_directory(workspace: &Path, name: &str) -> PathBuf {
    workspace.join(".hotline/services").join(name)
}

/// Adds what the manifest says to a job's environment: service addresses,
/// then the manifest's own variables, which may use them.
pub fn apply(env: &mut BTreeMap<String, String>, composed: &Composed, workspace: &Path) {
    let root = workspace.to_string_lossy();
    for (name, service) in enabled_services(composed) {
        let directory = service_directory(workspace, name);
        let directory = directory.to_string_lossy();
        for (key, value) in &service.env {
            env.insert(key.clone(), value.replace("$SERVICE_DIR", &directory));
        }
    }
    for (key, value) in &composed.manifest.env {
        match value {
            EnvValue::Text(text) => {
                env.insert(key.clone(), text.replace("$WORKSPACE", &root));
            }
            EnvValue::List(items) => {
                let mut parts: Vec<String> = items
                    .iter()
                    .map(|item| {
                        let item = item.replace("$WORKSPACE", &root);
                        if Path::new(&item).is_absolute() {
                            item
                        } else {
                            workspace.join(item).to_string_lossy().into_owned()
                        }
                    })
                    .collect();
                if let Some(existing) = env.get(key).filter(|v| !v.is_empty()) {
                    parts.push(existing.clone());
                }
                env.insert(key.clone(), parts.join(":"));
            }
        }
    }
}

/// Whether the repository's manifest file still says what was prepared.
pub fn changed(workspace: &Path, composed: &Composed) -> Option<String> {
    let path = workspace.join(FILE);
    if !path.exists() {
        return (composed.repository != Manifest::default())
            .then(|| format!("{} was removed after the last prepare", path.display()));
    }
    match read(&path) {
        Ok(current) if current == composed.repository => None,
        Ok(_) => Some(format!(
            "{} changed after the last prepare; run state prepare for this workspace to take it",
            path.display()
        )),
        Err(error) => Some(error),
    }
}

/// The nearest prepared manifest workspace at or above `cwd`, within home.
pub fn find(home: &Path, cwd: &Path) -> Result<(PathBuf, Composed), String> {
    let home = home.canonicalize().map_err(|e| e.to_string())?;
    let cwd = cwd
        .canonicalize()
        .map_err(|e| format!("{}: {e}", cwd.display()))?;
    for directory in cwd.ancestors().take_while(|d| d.starts_with(&home)) {
        let spec = directory.join(".hotline/environment-spec.json");
        if !spec.exists() {
            continue;
        }
        let definition: crate::workspace::Definition = serde_json::from_slice(
            &std::fs::read(&spec).map_err(|e| format!("{}: {e}", spec.display()))?,
        )
        .map_err(|e| format!("{}: {e}", spec.display()))?;
        return match definition {
            crate::workspace::Definition::Manifest(composed) => {
                Ok((directory.to_path_buf(), *composed))
            }
            _ => Err(format!(
                "{} was prepared from a package list or flake, which names no runs; write {FILE} with runs and state prepare again",
                directory.display()
            )),
        };
    }
    Err(format!(
        "{} is not in a prepared workspace; write {FILE} and run state prepare first",
        cwd.display()
    ))
}

/// A named run as a job to start, with its URL when it serves one.
pub fn job(
    workspace: &Path,
    composed: &Composed,
    run: &str,
    mut extra: Start,
) -> Result<(Start, Option<String>), String> {
    if let Some(problem) = changed(workspace, composed) {
        return Err(problem);
    }
    let Some(entry) = composed.manifest.runs.get(run) else {
        let names = offered(&composed.manifest.runs);
        return Err(format!(
            "no run named {run:?} in {}; it has {names}. Add the command to runs in {FILE} and state prepare, rather than running it by hand",
            workspace.display()
        ));
    };
    let cwd = match &entry.cwd {
        Some(sub) => workspace.join(sub),
        None => workspace.to_path_buf(),
    };
    let mut env = entry.env.clone();
    let mut url = None;
    if entry.kind == Kind::Web {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("no free port: {e}"))?
            .port();
        env.insert("PORT".into(), port.to_string());
        url = Some(format!("http://localhost:{port}"));
    }
    env.append(&mut extra.env);
    let mut args: Vec<String> = entry.command[1..].to_vec();
    args.append(&mut extra.args);
    // Commands are argv, not shell, so the port a web run is given is
    // substituted where the manifest writes $PORT.
    if let Some(port) = env.get("PORT").cloned() {
        for arg in &mut args {
            *arg = arg.replace("$PORT", &port);
        }
    }
    let label = entry.label.clone().unwrap_or_else(|| format!("Run {run}"));
    Ok((
        Start {
            command: entry.command[0].clone(),
            args,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            env,
            label: Some(label),
            timeout: extra.timeout,
            pty: extra.pty,
            request_id: extra.request_id,
            secrets: true,
            ..Start::default()
        },
        url,
    ))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Activation {
    /// Create hooks already run, with the command each ran.
    #[serde(default)]
    created: BTreeMap<String, Vec<String>>,
    /// Latest job for each hook and service: `create:NAME`, `service:NAME`, `start:NAME`.
    #[serde(default)]
    jobs: BTreeMap<String, String>,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    error: Option<String>,
}

fn load(workspace: &Path) -> Activation {
    std::fs::read(workspace.join(ACTIVATION))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(workspace: &Path, activation: &Activation) {
    let path = workspace.join(ACTIVATION);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(activation) {
        let _ = std::fs::write(path, bytes);
    }
}

static ACTIVATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn registry(home: &Path) -> PathBuf {
    home.join(".cache/hotline/manifest-workspaces.json")
}

/// Remembers a manifest workspace, so its services and start hooks come back after a restart.
pub fn remember(home: &Path, workspace: &Path) {
    let path = registry(home);
    let mut known: Vec<PathBuf> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if known.iter().any(|w| w == workspace) {
        return;
    }
    known.push(workspace.to_path_buf());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&known) {
        let _ = std::fs::write(path, bytes);
    }
}

/// At boot: every remembered workspace that is still prepared gets its
/// services and start hooks again. Create hooks already ran and do not repeat.
pub async fn resume(app: App) {
    let Ok(home) = app.config.home.canonicalize() else {
        return;
    };
    let known: Vec<PathBuf> = std::fs::read(registry(&home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for workspace in known {
        let Ok((root, composed)) = find(&home, &workspace) else {
            continue;
        };
        if root != workspace || crate::workspace::environment(&home, &workspace).is_err() {
            continue;
        }
        activate(app.clone(), workspace, composed, "computer".into(), None).await;
    }
}

/// What preparation will do next, said in its answer.
pub fn plan(composed: &Composed) -> Value {
    let m = &composed.manifest;
    json!({
        "runs": m.runs.keys().collect::<Vec<_>>(),
        "create_hooks": m.hooks.create.keys().collect::<Vec<_>>(),
        "services": enabled_services(composed).map(|(n, _)| n).collect::<Vec<_>>(),
        "start_hooks": m.hooks.start.keys().collect::<Vec<_>>(),
    })
}

/// After preparation succeeds: create hooks once, then services and start
/// hooks unless already running. Progress is kept in `.hotline/state`.
pub async fn activate(
    app: App,
    workspace: PathBuf,
    composed: Composed,
    holder: String,
    preparation: Option<String>,
) {
    let _one = ACTIVATING.lock().await;
    let mut state = load(&workspace);
    state.error = None;
    if let Some(id) = preparation {
        state.phase = "preparing".into();
        save(&workspace, &state);
        let record = loop {
            match app.jobs.wait(&id, 60_000).await {
                Ok(record) if record.finished() => break Some(record),
                Ok(_) => continue,
                Err(_) => break None,
            }
        };
        if record.is_none_or(|r| r.exit_code != Some(0)) {
            state.phase = "preparation failed".into();
            state.error = Some(format!(
                "preparation job {id} did not succeed; read it with shell read"
            ));
            save(&workspace, &state);
            return;
        }
    }
    let m = &composed.manifest;
    state.phase = "setting up".into();
    save(&workspace, &state);
    for (name, command) in &m.hooks.create {
        if state.created.get(name) == Some(command) {
            continue;
        }
        let start = Start {
            command: command[0].clone(),
            args: command[1..].to_vec(),
            cwd: Some(workspace.to_string_lossy().into_owned()),
            label: Some(format!("Set up: {name}")),
            secrets: true,
            ..Start::default()
        };
        let record = match start_job(&app, start, &holder).await {
            Ok(record) => record,
            Err(error) => return fail(&workspace, state, format!("create hook {name}: {error}")),
        };
        state
            .jobs
            .insert(format!("create:{name}"), record.id.clone());
        save(&workspace, &state);
        let finished = loop {
            match app.jobs.wait(&record.id, 60_000).await {
                Ok(r) if r.finished() => break r,
                Ok(_) => continue,
                Err(error) => return fail(&workspace, state, error),
            }
        };
        if finished.exit_code != Some(0) {
            return fail(
                &workspace,
                state,
                format!(
                    "create hook {name} failed in job {}; fix it in {FILE} and prepare again",
                    record.id
                ),
            );
        }
        state.created.insert(name.clone(), command.clone());
        save(&workspace, &state);
    }
    state.phase = "starting".into();
    save(&workspace, &state);
    let mut long_running: Vec<(String, Start)> = Vec::new();
    for (name, service) in enabled_services(&composed) {
        let directory = service_directory(&workspace, name);
        if let Err(error) = std::fs::create_dir_all(&directory) {
            return fail(&workspace, state, format!("service {name}: {error}"));
        }
        long_running.push((
            format!("service:{name}"),
            Start {
                command: service.command[0].clone(),
                args: service.command[1..].to_vec(),
                cwd: Some(workspace.to_string_lossy().into_owned()),
                env: BTreeMap::from([(
                    "SERVICE_DIR".into(),
                    directory.to_string_lossy().into_owned(),
                )]),
                label: Some(format!("Service: {name}")),
                ..Start::default()
            },
        ));
    }
    for (name, command) in &m.hooks.start {
        long_running.push((
            format!("start:{name}"),
            Start {
                command: command[0].clone(),
                args: command[1..].to_vec(),
                cwd: Some(workspace.to_string_lossy().into_owned()),
                label: Some(format!("On start: {name}")),
                secrets: true,
                ..Start::default()
            },
        ));
    }
    for (key, start) in long_running {
        if let Some(id) = state.jobs.get(&key)
            && app.jobs.status(id).await.is_ok_and(|r| !r.finished())
        {
            continue;
        }
        match start_job(&app, start, &holder).await {
            Ok(record) => {
                state.jobs.insert(key, record.id);
            }
            Err(error) => return fail(&workspace, state, format!("{key}: {error}")),
        }
    }
    state.phase = "ready".into();
    save(&workspace, &state);
}

async fn start_job(app: &App, start: Start, holder: &str) -> Result<crate::jobs::Record, String> {
    let _guard = app.access.mutate(holder).await?;
    app.jobs.start(start, holder).await
}

fn fail(workspace: &Path, mut state: Activation, error: String) {
    state.phase = "failed".into();
    state.error = Some(error);
    save(workspace, &state);
}

/// An example worth copying, said when a workspace has no manifest yet.
pub fn example() -> Value {
    json!({
        "packages": ["nodejs", "pnpm", "cargo", "rustc"],
        "platform": ["webkit"],
        "env": {"RUST_LOG": "info", "PATH": ["node_modules/.bin"]},
        "services": {"postgres": {"enable": true}},
        "hooks": {"create": {"install": ["pnpm", "install"]}},
        "runs": {
            "build": {"command": ["cargo", "build"]},
            "test": {"command": ["cargo", "test"]},
            "app": {"command": ["cargo", "tauri", "dev"], "kind": "desktop"},
            "web": {"command": ["pnpm", "dev", "--port", "$PORT"], "kind": "web"}
        }
    })
}

/// `state manifest`: what the workspace declares, what is prepared, what runs.
pub async fn describe(app: &App, workspace: &Path) -> Result<Value, String> {
    let current = base()?;
    let offers = json!({
        "version": current.version,
        "nixpkgs": current.nixpkgs,
        "platforms": current.platforms.iter().map(|(k, p)| (k.clone(), json!({"description": p.description, "requires": p.requires}))).collect::<serde_json::Map<_, _>>(),
        "services": current.services.iter().map(|(k, s)| (k.clone(), json!(s.description))).collect::<serde_json::Map<_, _>>(),
    });
    let file = workspace.join(FILE);
    let prepared = match find(&app.config.home, workspace) {
        Ok((root, composed)) if root == workspace => Some(composed),
        _ => None,
    };
    let Some(composed) = prepared else {
        let declared = file.exists().then(|| read(&file)).transpose()?;
        let draft = declared.is_none().then(|| crate::draft::draft(workspace));
        return Ok(json!({
            "workspace": workspace,
            "file": file,
            "declared": declared,
            "prepared": false,
            "draft": draft,
            "next": if declared.is_some() {"state prepare with this workspace"} else {"review draft.manifest against the README (reasons says why each line is there), correct it, then state prepare with it as manifest"},
            "example": example(),
            "shape": SHAPE,
            "image": offers,
        }));
    };
    let state = load(workspace);
    let mut jobs = serde_json::Map::new();
    for (key, id) in &state.jobs {
        let status = app.jobs.status(id).await.ok();
        jobs.insert(
            key.clone(),
            json!({"job_id": id, "state": status.as_ref().map(|r| r.state.clone()), "exit_code": status.and_then(|r| r.exit_code)}),
        );
    }
    let services: serde_json::Map<String, Value> = enabled_services(&composed)
        .map(|(name, _)| {
            let mut env = BTreeMap::new();
            if let Some(s) = composed.base.services.get(name) {
                let dir = service_directory(workspace, name);
                for (k, v) in &s.env {
                    env.insert(k.clone(), v.replace("$SERVICE_DIR", &dir.to_string_lossy()));
                }
            }
            (name.clone(), json!({"env": env}))
        })
        .collect();
    Ok(json!({
        "workspace": workspace,
        "file": file,
        "prepared": true,
        "changed": changed(workspace, &composed),
        "manifest": composed.manifest,
        "runs": composed.manifest.runs,
        "platforms": platforms(&composed).iter().map(|(n, _)| n).collect::<Vec<_>>(),
        "packages": packages(&composed),
        "services": services,
        "activation": {"phase": state.phase, "error": state.error, "created": state.created.keys().collect::<Vec<_>>(), "jobs": jobs},
        "base": {
            "version": composed.base.version,
            "nixpkgs": composed.base.nixpkgs,
            "image": current.version,
            "update_available": (composed.base.version != current.version).then(|| format!("this image carries base {}; state prepare with upgrade true to take it", current.version)),
        },
        "image": offers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> Base {
        serde_json::from_str(EMBEDDED_BASE).unwrap()
    }

    fn manifest(value: Value) -> Manifest {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn layers_concatenate_lists_merge_maps_and_last_wins_for_scalars() {
        let lower = manifest(json!({
            "packages": ["cargo", "rustc"], "platform": ["gtk"],
            "env": {"PATH": ["/opt/a"], "MODE": "base"},
            "runs": {"build": {"command": ["cargo", "build"]}, "test": {"command": ["cargo", "test"]}}
        }));
        let upper = manifest(json!({
            "packages": ["rustc", "bun"], "platform": ["webkit"],
            "env": {"PATH": ["node_modules/.bin"], "MODE": "repo"},
            "runs": {"build": {"command": ["cargo", "build", "--release"]}}
        }));
        let merged = merge(lower, upper);
        assert_eq!(merged.packages, ["cargo", "rustc", "bun"]);
        assert_eq!(merged.platform, ["gtk", "webkit"]);
        assert_eq!(
            merged.env["PATH"],
            EnvValue::List(vec!["/opt/a".into(), "node_modules/.bin".into()])
        );
        assert_eq!(merged.env["MODE"], EnvValue::Text("repo".into()));
        assert_eq!(
            merged.runs["build"].command,
            ["cargo", "build", "--release"]
        );
        assert!(merged.runs.contains_key("test"));
    }

    #[test]
    fn errors_say_what_is_valid() {
        let error = parse(r#"{"run": {}}"#).unwrap_err();
        assert!(
            error.contains("unknown field `run`") && error.contains("runs (NAME:"),
            "{error}"
        );
        let error = compose(embedded(), manifest(json!({"platform": ["cocoa"]}))).unwrap_err();
        assert!(
            error.contains("gl, gtk, gtk4, native, prebuilt, qt, webkit"),
            "{error}"
        );
        let error = compose(embedded(), manifest(json!({"services": {"mysql": {}}}))).unwrap_err();
        assert!(error.contains("postgres, redis"), "{error}");
        let error = compose(embedded(), manifest(json!({"env": {"PATH": "/bin"}}))).unwrap_err();
        assert!(error.contains("must be a list"), "{error}");
        let error = compose(
            embedded(),
            manifest(json!({"runs": {"x": {"command": ["a"], "cwd": "../up"}}})),
        )
        .unwrap_err();
        assert!(error.contains("inside the workspace"), "{error}");
        let error = compose(
            embedded(),
            manifest(json!({"runs": {"x": {"command": []}}})),
        )
        .unwrap_err();
        assert!(error.contains("argv array"), "{error}");
        let error = compose(
            embedded(),
            manifest(json!({"flake": ".", "platform": ["gl"]})),
        )
        .unwrap_err();
        assert!(error.contains("flake replaces packages"), "{error}");
    }

    #[test]
    fn platforms_pull_in_what_they_require_and_render_their_runtime() {
        let composed = compose(
            embedded(),
            manifest(
                json!({"packages": ["cargo"], "platform": ["webkit"], "services": {"redis": {}}}),
            ),
        )
        .unwrap();
        let names: Vec<&String> = platforms(&composed).iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["gl", "gtk", "webkit"]);
        let all = packages(&composed);
        for expected in [
            "cargo",
            "mesa",
            "gtk3",
            "webkitgtk_4_1",
            "libayatana-appindicator",
            "redis",
        ] {
            assert!(all.contains(&expected.to_string()), "{expected}");
        }
        let flake = render(&composed).unwrap();
        assert!(flake.contains("__EGL_VENDOR_LIBRARY_FILENAMES = \"${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json\";"), "{flake}");
        assert!(flake.contains("LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.libayatana-appindicator pkgs.libglvnd pkgs.mesa ];"), "{flake}");
        assert!(flake.contains("GIO_EXTRA_MODULES"));
        assert!(flake.contains("github:NixOS/nixpkgs/ef34387ddd751e1ab8857adf4676492d32eb24ec"));
        let plain = compose(embedded(), manifest(json!({"packages": ["go"]}))).unwrap();
        let flake = render(&plain).unwrap();
        assert!(
            !flake.contains("LD_LIBRARY_PATH") && !flake.contains("XDG_DATA_DIRS"),
            "{flake}"
        );
    }

    #[test]
    fn nix_strings_expand_only_package_references() {
        assert_eq!(nix_string("${mesa}/lib").unwrap(), "\"${pkgs.mesa}/lib\"");
        assert_eq!(nix_string("a\"b$HOME").unwrap(), "\"a\\\"b\\$HOME\"");
        assert!(nix_string("${bad attr}").is_err());
        assert!(nix_string("${open").is_err());
    }

    #[test]
    fn environment_gains_service_addresses_and_extends_paths() {
        let composed = compose(
            embedded(),
            manifest(json!({
                "packages": ["nodejs"],
                "services": {"postgres": {}, "redis": {"enable": false}},
                "env": {"PATH": ["node_modules/.bin", "/opt/x"], "APP_DIR": "$WORKSPACE/app"}
            })),
        )
        .unwrap();
        let workspace = Path::new("/home/agent/src/p");
        let mut env =
            BTreeMap::from([("PATH".to_string(), "/nix/store/x/bin:/usr/bin".to_string())]);
        apply(&mut env, &composed, workspace);
        assert_eq!(
            env["PATH"],
            "/home/agent/src/p/node_modules/.bin:/opt/x:/nix/store/x/bin:/usr/bin"
        );
        assert_eq!(env["APP_DIR"], "/home/agent/src/p/app");
        assert_eq!(
            env["PGHOST"],
            "/home/agent/src/p/.hotline/services/postgres"
        );
        assert!(!env.contains_key("REDIS_URL"));
    }

    #[test]
    fn a_run_is_a_job_and_an_unknown_one_names_the_alternatives() {
        let home = tempfile::tempdir().unwrap();
        let workspace = home.path();
        let repository = manifest(json!({
            "packages": ["go"],
            "runs": {
                "test": {"command": ["go", "test", "./..."], "cwd": "server", "env": {"CGO_ENABLED": "0"}},
                "web": {"command": ["go", "run", ".", "--port", "$PORT"], "kind": "web"}
            }
        }));
        std::fs::create_dir_all(workspace.join(".hotline")).unwrap();
        std::fs::write(
            workspace.join(FILE),
            serde_json::to_vec(&repository).unwrap(),
        )
        .unwrap();
        let composed = compose(embedded(), repository).unwrap();
        let extra = Start {
            args: vec!["-run".into(), "TestX".into()],
            ..Start::default()
        };
        let (start, url) = job(workspace, &composed, "test", extra).unwrap();
        assert_eq!(start.command, "go");
        assert_eq!(start.args, ["test", "./...", "-run", "TestX"]);
        assert_eq!(
            start.cwd.unwrap(),
            workspace.join("server").to_string_lossy()
        );
        assert_eq!(start.label.unwrap(), "Run test");
        assert_eq!(start.env["CGO_ENABLED"], "0");
        assert!(url.is_none());
        let (start, url) = job(workspace, &composed, "web", Start::default()).unwrap();
        assert_eq!(
            url.unwrap(),
            format!("http://localhost:{}", start.env["PORT"])
        );
        assert_eq!(
            start.args,
            ["run", ".", "--port", start.env["PORT"].as_str()]
        );
        let error = job(workspace, &composed, "lint", Start::default()).unwrap_err();
        assert!(
            error.contains("test, web") && error.contains("rather than running it by hand"),
            "{error}"
        );
        std::fs::write(workspace.join(FILE), r#"{"packages":["go","gopls"]}"#).unwrap();
        let error = job(workspace, &composed, "test", Start::default()).unwrap_err();
        assert!(error.contains("changed after the last prepare"), "{error}");
    }
}
