//! Drafts a manifest from what a repository already says about itself: its
//! lockfiles, toolchain files and build files. The draft is a proposal the
//! agent reviews and corrects, not a preset: every line says why it is there.
use crate::manifest::{EnvValue, Kind, Manifest, Run};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize)]
pub struct Draft {
    pub manifest: Manifest,
    /// One line per decision: the evidence, then what it added.
    pub reasons: Vec<String>,
}

const SKIP: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".hotline",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    "out",
];

/// Files named `name` within three levels, nearest first, relative to the root.
fn find(root: &Path, name: &str) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        let mut dirs = Vec::new();
        for entry in entries {
            let path = entry.path();
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if path.is_dir() {
                if depth < 3 && !SKIP.contains(&file_name.as_ref()) && !file_name.starts_with('.') {
                    dirs.push(path);
                }
            } else if file_name == name
                && let Ok(relative) = path.strip_prefix(root)
            {
                out.push(relative.to_path_buf());
            }
        }
        for dir in dirs {
            walk(root, &dir, name, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(root, root, name, 0, &mut out);
    out.sort_by_key(|p| p.components().count());
    out
}

fn read(root: &Path, relative: &Path) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap_or_default()
}

fn directory(relative: &Path) -> String {
    let parent = relative.parent().unwrap_or(Path::new(""));
    if parent.as_os_str().is_empty() {
        ".".into()
    } else {
        parent.to_string_lossy().into_owned()
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn run(command: Vec<String>, cwd: &str, kind: Kind) -> Run {
    Run {
        command,
        cwd: (cwd != ".").then(|| cwd.to_owned()),
        kind,
        env: BTreeMap::new(),
        label: None,
    }
}

struct Builder {
    draft: Draft,
}

impl Builder {
    fn packages(&mut self, names: &[&str]) {
        for name in names {
            if !self.draft.manifest.packages.iter().any(|p| p == name) {
                self.draft.manifest.packages.push(name.to_string());
            }
        }
    }

    fn platform(&mut self, name: &str) {
        if !self.draft.manifest.platform.iter().any(|p| p == name) {
            self.draft.manifest.platform.push(name.into());
        }
    }

    fn path(&mut self, entry: String) {
        let path = self
            .draft
            .manifest
            .env
            .entry("PATH".into())
            .or_insert_with(|| EnvValue::List(Vec::new()));
        if let EnvValue::List(list) = path
            && !list.contains(&entry)
        {
            list.push(entry);
        }
    }

    fn run(&mut self, name: &str, entry: Run) {
        self.draft.manifest.runs.entry(name.into()).or_insert(entry);
    }

    fn hook(&mut self, name: &str, command: Vec<String>) {
        self.draft
            .manifest
            .hooks
            .create
            .entry(name.into())
            .or_insert(command);
    }

    fn why(&mut self, reason: impl Into<String>) {
        self.draft.reasons.push(reason.into());
    }
}

/// Drafts a manifest for the repository at `root`.
pub fn draft(root: &Path) -> Draft {
    let mut b = Builder {
        draft: Draft::default(),
    };
    let tauri = find(root, "tauri.conf.json");
    rust(root, &mut b, &tauri);
    node(root, &mut b, !tauri.is_empty());
    python(root, &mut b);
    go(root, &mut b);
    native(root, &mut b);
    java(root, &mut b);
    if root.join("flake.nix").is_file() {
        b.why("flake.nix is present: to use it instead, set flake to \".\" and drop packages, platform and services (a flake declares those itself)");
    }
    if b.draft.manifest == Manifest::default() {
        b.why("no build files recognised; start from the example and the project's README");
    }
    b.draft
}

fn rust(root: &Path, b: &mut Builder, tauri: &[PathBuf]) {
    let Some(cargo) = find(root, "Cargo.toml").into_iter().next() else {
        return;
    };
    let dir = directory(&cargo);
    b.packages(&["cargo", "rustc", "rustfmt", "clippy", "gcc", "pkg-config"]);
    b.why(format!(
        "{} → cargo, rustc, rustfmt, clippy, gcc, pkg-config",
        cargo.display()
    ));
    let lock = read(root, &Path::new(&dir).join("Cargo.lock"));
    // Without a lock yet, the crates the manifests name directly still count.
    let declared: String = find(root, "Cargo.toml")
        .iter()
        .map(|toml| read(root, toml))
        .collect();
    let has = |krate: &str| {
        lock.contains(&format!("name = \"{krate}\"\n"))
            || declared
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{krate} =")))
    };
    for (krate, package) in [
        ("openssl-sys", "openssl"),
        ("libdbus-sys", "dbus"),
        ("alsa-sys", "alsa-lib"),
        ("libudev-sys", "udev"),
    ] {
        if has(krate) {
            b.packages(&[package]);
            b.why(format!("Cargo.lock uses {krate} → {package}"));
        }
    }
    b.run("build", run(argv(&["cargo", "build"]), &dir, Kind::Task));
    b.run("test", run(argv(&["cargo", "test"]), &dir, Kind::Task));
    if let Some(conf) = tauri.first() {
        b.platform("webkit");
        b.packages(&["cargo-tauri"]);
        let app = directory(conf);
        b.run(
            "app",
            run(argv(&["cargo", "tauri", "dev"]), &app, Kind::Desktop),
        );
        b.why(format!(
            "{} → platform webkit, cargo-tauri, and an app run with cargo tauri dev there",
            conf.display()
        ));
        return;
    }
    let gui = if has("gtk4") {
        Some(("gtk4", "gtk4"))
    } else if has("gtk") {
        Some(("gtk", "gtk"))
    } else if has("cxx-qt") || has("qmetaobject") {
        Some(("qt", "cxx-qt or qmetaobject"))
    } else if ["winit", "sdl2", "sdl3", "glfw", "minifb", "softbuffer"]
        .iter()
        .any(|k| has(k))
    {
        Some(("native", "winit, SDL, GLFW or minifb"))
    } else {
        None
    };
    if let Some((platform, evidence)) = gui {
        b.platform(platform);
        if has("sdl2") {
            b.packages(&["SDL2"]);
        }
        b.run("app", run(argv(&["cargo", "run"]), &dir, Kind::Desktop));
        b.why(format!(
            "the crates use {evidence} → platform {platform}, and an app run with cargo run"
        ));
    }
}

fn node(root: &Path, b: &mut Builder, tauri: bool) {
    let manifests = find(root, "package.json");
    if manifests.is_empty() {
        return;
    }
    b.packages(&["nodejs"]);
    for (index, manifest) in manifests.iter().take(4).enumerate() {
        let dir = directory(manifest);
        let exists = |file: &str| root.join(&dir).join(file).is_file() || root.join(file).is_file();
        let (pm, install): (&str, Vec<String>) = if exists("bun.lock") || exists("bun.lockb") {
            ("bun", argv(&["bun", "install", "--frozen-lockfile"]))
        } else if exists("pnpm-lock.yaml") {
            ("pnpm", argv(&["pnpm", "install", "--frozen-lockfile"]))
        } else if exists("yarn.lock") {
            ("yarn", argv(&["yarn", "install"]))
        } else if root.join(&dir).join("package-lock.json").is_file() {
            ("npm", argv(&["npm", "ci"]))
        } else {
            ("npm", argv(&["npm", "install"]))
        };
        if pm != "npm" {
            b.packages(&[pm]);
        }
        let mut install = install;
        if dir != "." {
            let flag = match pm {
                "bun" | "yarn" => "--cwd",
                "pnpm" => "--dir",
                _ => "--prefix",
            };
            install.push(flag.into());
            install.push(dir.clone());
        }
        let hook = if dir == "." {
            "install".to_string()
        } else {
            format!("install-{}", dir.replace('/', "-"))
        };
        b.hook(&hook, install);
        b.path(if dir == "." {
            "node_modules/.bin".into()
        } else {
            format!("{dir}/node_modules/.bin")
        });
        b.why(format!(
            "{} with {pm} → nodejs{}, a create hook that installs dependencies",
            manifest.display(),
            if pm == "npm" {
                String::new()
            } else {
                format!(", {pm}")
            }
        ));
        // A Tauri app drives its web UI itself; scripts come from the root package only.
        if tauri || index > 0 {
            continue;
        }
        let text = read(root, manifest);
        let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let electron = text.contains("\"electron\"");
        let Some(scripts) = package["scripts"].as_object() else {
            continue;
        };
        for name in ["dev", "start", "build", "test", "lint", "typecheck"] {
            let Some(script) = scripts.get(name).and_then(|s| s.as_str()) else {
                continue;
            };
            let mut command = argv(&[pm, "run", name]);
            let kind = if ["dev", "start"].contains(&name) {
                if electron || script.contains("electron") {
                    Kind::Desktop
                } else if [
                    "vite",
                    "next",
                    "astro",
                    "nuxt",
                    "svelte-kit",
                    "remix",
                    "ng serve",
                    "webpack serve",
                ]
                .iter()
                .any(|tool| script.contains(tool))
                {
                    if pm == "npm" {
                        command.push("--".into());
                    }
                    command.extend(argv(&["--port", "$PORT"]));
                    Kind::Web
                } else if script.contains("react-scripts") {
                    Kind::Web
                } else {
                    Kind::Task
                }
            } else {
                Kind::Task
            };
            b.run(name, run(command, &dir, kind));
        }
        let names: Vec<&str> = ["dev", "start", "build", "test", "lint", "typecheck"]
            .into_iter()
            .filter(|n| scripts.contains_key(*n))
            .collect();
        if !names.is_empty() {
            b.why(format!(
                "package.json scripts {} → runs of the same names{}",
                names.join(", "),
                if electron {
                    "; electron → dev and start are desktop runs"
                } else {
                    ""
                }
            ));
        }
    }
}

fn python(root: &Path, b: &mut Builder) {
    let pyproject = root.join("pyproject.toml").is_file();
    let requirements = root.join("requirements.txt").is_file();
    if !pyproject && !requirements && !root.join("setup.py").is_file() {
        return;
    }
    b.packages(&["python312"]);
    let text = read(root, Path::new("pyproject.toml")) + &read(root, Path::new("requirements.txt"));
    if root.join("uv.lock").is_file() {
        b.packages(&["uv"]);
        b.hook("sync", argv(&["uv", "sync"]));
        b.why("uv.lock → python312, uv, a create hook running uv sync");
    } else if root.join("poetry.lock").is_file() {
        b.packages(&["poetry"]);
        b.hook("install", argv(&["poetry", "install"]));
        b.draft.manifest.env.insert(
            "POETRY_VIRTUALENVS_IN_PROJECT".into(),
            EnvValue::Text("true".into()),
        );
        b.why("poetry.lock → python312, poetry, a create hook running poetry install into .venv");
    } else {
        b.packages(&["uv"]);
        b.hook("1-venv", argv(&["uv", "venv"]));
        if requirements {
            b.hook(
                "2-install",
                argv(&["uv", "pip", "install", "-r", "requirements.txt"]),
            );
        } else {
            b.hook("2-install", argv(&["uv", "pip", "install", "-e", "."]));
        }
        b.why("Python project without a lock → python312, uv, create hooks making .venv and installing into it");
    }
    b.path(".venv/bin".into());
    b.draft.manifest.env.insert(
        "VIRTUAL_ENV".into(),
        EnvValue::Text("$WORKSPACE/.venv".into()),
    );
    if text.contains("pytest") || root.join("tests").is_dir() {
        b.run(
            "test",
            run(argv(&["python", "-m", "pytest"]), ".", Kind::Task),
        );
        b.why("pytest or a tests directory → a test run");
    }
}

fn go(root: &Path, b: &mut Builder) {
    let Some(module) = find(root, "go.mod").into_iter().next() else {
        return;
    };
    let dir = directory(&module);
    b.packages(&["go", "gopls"]);
    b.run(
        "build",
        run(argv(&["go", "build", "./..."]), &dir, Kind::Task),
    );
    b.run(
        "test",
        run(argv(&["go", "test", "./..."]), &dir, Kind::Task),
    );
    b.why(format!(
        "{} → go, gopls, build and test runs",
        module.display()
    ));
    let sum = read(root, &Path::new(&dir).join("go.sum"));
    if ["fyne.io/", "gioui.org", "go-gl/glfw", "veandco/go-sdl2"]
        .iter()
        .any(|m| sum.contains(m))
    {
        b.platform("native");
        b.packages(&["gcc"]);
        b.run("app", run(argv(&["go", "run", "."]), &dir, Kind::Desktop));
        b.why("go.sum uses Fyne, Gio, GLFW or SDL → platform native, gcc for cgo, an app run");
    }
}

fn native(root: &Path, b: &mut Builder) {
    let cmake = root.join("CMakeLists.txt");
    let meson = root.join("meson.build");
    let make = root.join("Makefile");
    let text = read(root, Path::new("CMakeLists.txt")) + &read(root, Path::new("meson.build"));
    if cmake.is_file() {
        b.packages(&["cmake", "ninja", "gcc", "pkg-config"]);
        b.hook("configure", argv(&["cmake", "-B", "build", "-G", "Ninja"]));
        b.run(
            "build",
            run(argv(&["cmake", "--build", "build"]), ".", Kind::Task),
        );
        b.run(
            "test",
            run(argv(&["ctest", "--test-dir", "build"]), ".", Kind::Task),
        );
        b.why(
            "CMakeLists.txt → cmake, ninja, gcc, pkg-config; a configure hook, build and test runs",
        );
    } else if meson.is_file() {
        b.packages(&["meson", "ninja", "gcc", "pkg-config"]);
        b.hook("configure", argv(&["meson", "setup", "build"]));
        b.run(
            "build",
            run(argv(&["meson", "compile", "-C", "build"]), ".", Kind::Task),
        );
        b.run(
            "test",
            run(argv(&["meson", "test", "-C", "build"]), ".", Kind::Task),
        );
        b.why("meson.build → meson, ninja, gcc, pkg-config; a configure hook, build and test runs");
    } else if make.is_file() && b.draft.manifest.runs.is_empty() {
        b.packages(&["gcc", "gnumake", "pkg-config"]);
        b.run("build", run(argv(&["make"]), ".", Kind::Task));
        if read(root, Path::new("Makefile")).contains("\ntest:") {
            b.run("test", run(argv(&["make", "test"]), ".", Kind::Task));
        }
        b.why("Makefile → gcc, gnumake, pkg-config and a build run");
    }
    if text.is_empty() {
        return;
    }
    let gui = if text.contains("Qt6") || text.contains("qt6") {
        Some(("qt", "Qt 6"))
    } else if text.contains("gtk4") {
        Some(("gtk4", "GTK 4"))
    } else if text.contains("gtk+-3.0") || text.contains("GTK3") {
        Some(("gtk", "GTK 3"))
    } else if text.contains("SDL2") || text.contains("sdl2") {
        b.packages(&["SDL2"]);
        Some(("native", "SDL2"))
    } else if text.contains("glfw") {
        b.packages(&["glfw"]);
        Some(("native", "GLFW"))
    } else {
        None
    };
    if let Some((platform, toolkit)) = gui {
        b.platform(platform);
        let target = text
            .split("add_executable(")
            .nth(1)
            .and_then(|rest| rest.split(|c: char| c.is_whitespace() || c == ')').next())
            .filter(|name| !name.is_empty() && !name.contains('$'));
        if let Some(target) = target {
            b.run(
                "app",
                run(vec![format!("./build/{target}")], ".", Kind::Desktop),
            );
        }
        b.why(format!(
            "the build uses {toolkit} → platform {platform}{}",
            if target.is_some() {
                ", and an app run for the first executable"
            } else {
                ""
            }
        ));
    }
}

fn java(root: &Path, b: &mut Builder) {
    if root.join("pom.xml").is_file() {
        b.packages(&["jdk21", "maven"]);
        b.run(
            "build",
            run(argv(&["mvn", "-q", "package"]), ".", Kind::Task),
        );
        b.run("test", run(argv(&["mvn", "test"]), ".", Kind::Task));
        b.why("pom.xml → jdk21, maven, build and test runs");
    } else if root.join("build.gradle").is_file() || root.join("build.gradle.kts").is_file() {
        let wrapper = root.join("gradlew").is_file();
        b.packages(if wrapper {
            &["jdk21"]
        } else {
            &["jdk21", "gradle"]
        });
        let gradle = if wrapper { "./gradlew" } else { "gradle" };
        b.run("build", run(argv(&[gradle, "build"]), ".", Kind::Task));
        b.run("test", run(argv(&[gradle, "test"]), ".", Kind::Task));
        b.why("Gradle build → jdk21, build and test runs");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    fn composes(draft: &Draft) {
        let base = crate::manifest::base().unwrap();
        crate::manifest::compose(base, draft.manifest.clone()).expect("a draft must compose");
    }

    #[test]
    fn a_tauri_workspace_gets_webkit_its_ui_install_and_an_app_run() {
        let dir = repo(&[
            ("Cargo.toml", "[workspace]\n"),
            (
                "Cargo.lock",
                "[[package]]\nname = \"tauri\"\n[[package]]\nname = \"openssl-sys\"\n",
            ),
            ("crates/app/tauri.conf.json", "{}"),
            ("ui/package.json", r#"{"scripts":{"dev":"vite"}}"#),
            ("ui/bun.lock", ""),
        ]);
        let draft = draft(dir.path());
        let m = &draft.manifest;
        assert_eq!(m.platform, ["webkit"]);
        for p in ["cargo", "cargo-tauri", "openssl", "bun", "nodejs"] {
            assert!(m.packages.contains(&p.to_string()), "{p}");
        }
        assert_eq!(
            m.hooks.create["install-ui"],
            ["bun", "install", "--frozen-lockfile", "--cwd", "ui"]
        );
        assert_eq!(m.runs["app"].cwd.as_deref(), Some("crates/app"));
        assert_eq!(m.runs["app"].kind, Kind::Desktop);
        assert!(
            !m.runs.contains_key("dev"),
            "the Tauri app drives the UI's dev server"
        );
        composes(&draft);
    }

    #[test]
    fn a_vite_app_gets_a_web_run_on_the_given_port() {
        let dir = repo(&[
            (
                "package.json",
                r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest"}}"#,
            ),
            ("package-lock.json", "{}"),
        ]);
        let draft = draft(dir.path());
        let m = &draft.manifest;
        assert_eq!(m.hooks.create["install"], ["npm", "ci"]);
        assert_eq!(
            m.runs["dev"].command,
            ["npm", "run", "dev", "--", "--port", "$PORT"]
        );
        assert_eq!(m.runs["dev"].kind, Kind::Web);
        assert_eq!(m.runs["build"].kind, Kind::Task);
        assert!(m.runs.contains_key("test"));
        composes(&draft);
    }

    #[test]
    fn native_toolkits_choose_their_platform() {
        let qt = draft(
            repo(&[(
                "CMakeLists.txt",
                "find_package(Qt6 REQUIRED COMPONENTS Widgets)\nadd_executable(viewer main.cpp)\n",
            )])
            .path(),
        );
        assert_eq!(qt.manifest.platform, ["qt"]);
        assert_eq!(qt.manifest.runs["app"].command, ["./build/viewer"]);
        assert_eq!(
            qt.manifest.hooks.create["configure"],
            ["cmake", "-B", "build", "-G", "Ninja"]
        );
        composes(&qt);
        let egui = draft(
            repo(&[
                ("Cargo.toml", "[package]\n"),
                ("Cargo.lock", "[[package]]\nname = \"winit\"\n"),
            ])
            .path(),
        );
        assert_eq!(egui.manifest.platform, ["native"]);
        assert_eq!(egui.manifest.runs["app"].command, ["cargo", "run"]);
        composes(&egui);
        let fyne = draft(
            repo(&[
                ("go.mod", "module x\n"),
                ("go.sum", "fyne.io/fyne/v2 v2.5.0 h1:x\n"),
            ])
            .path(),
        );
        assert_eq!(fyne.manifest.platform, ["native"]);
        composes(&fyne);
    }

    #[test]
    fn python_gets_a_virtualenv_and_every_decision_is_explained() {
        let draft = draft(repo(&[("requirements.txt", "pytest\n")]).path());
        let m = &draft.manifest;
        assert_eq!(
            m.hooks.create["2-install"],
            ["uv", "pip", "install", "-r", "requirements.txt"]
        );
        assert_eq!(m.env["PATH"], EnvValue::List(vec![".venv/bin".into()]));
        assert!(m.runs.contains_key("test"));
        assert!(draft.reasons.len() >= 2);
        composes(&draft);
        let empty = super::draft(repo(&[("README.md", "hi")]).path());
        assert!(empty.reasons[0].contains("no build files"));
    }
}
