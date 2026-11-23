use hotline_computer::display::Display;
use hotline_computer::{App, Config, boot, serve};

fn usage() -> &'static str {
    "Usage: hotline-computer <boot|serve> [--addr ADDRESS] [--token TOKEN] [--home PATH] [--display DISPLAY] [--screen WIDTHxHEIGHT]\n\n  boot   start the display, the session bus and the desktop, then serve; the container entrypoint\n  serve  serve on a display that already exists\n\nAt the person's terminal:\n  hotline-computer prepare [--workspace DIR] [--packages NAME... | --flake DIR]\n  hotline-computer activity   the recent activity display\n  hotline-computer packages   common Nixpkgs names to choose from\n  hotline-computer open DIR   a fresh terminal in that folder; what the browser's Show in folder does"
}

enum Command {
    Boot,
    Serve,
}

fn parse() -> Result<(Command, Config), String> {
    let mut args = std::env::args().skip(1);
    let command = match args.next().as_deref() {
        Some("boot") => Command::Boot,
        Some("serve") => Command::Serve,
        Some("--help" | "-h") | None => return Err(usage().to_owned()),
        Some(command) => return Err(format!("unknown subcommand {command:?}\n{}", usage())),
    };

    let mut config = Config::from_env();
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            return Err(usage().to_owned());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        match flag.as_str() {
            "--addr" => config.addr = value,
            "--token" => config.token = (!value.is_empty()).then_some(value),
            "--home" => config.home = value.into(),
            "--display" => config.display = value,
            "--screen" => config.screen = value,
            _ => return Err(format!("unknown flag {flag:?}\n{}", usage())),
        }
    }
    Ok((command, config))
}

/// Keeps this process's memory and `/proc` entries from the commands the
/// computer runs. They run as the same user, so a process left dumpable is
/// one a job can read with `/proc/<pid>/mem` or attach a debugger to.
fn hide() {
    #[cfg(target_os = "linux")]
    // SAFETY: a flag on this process alone, taking no pointers.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        eprintln!(
            "hotline-computer: could not hide this process from its jobs: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Keeps the service's bearer, and the secrets it later holds in memory,
/// from the commands it runs. Without this a job reads the token out of
/// `/proc/<pid>/environ`, or the secrets out of `/proc/<pid>/mem`, and
/// answers its own ask-me requests with it.
///
/// The variable leaves the environment so nothing started from here inherits
/// it: the display, the bus, the browser. The process stops being dumpable,
/// which hands its `/proc` entries to root and refuses a same-user ptrace;
/// the reaper `boot` forks from here keeps both. The container's own record
/// still carries the variable, which is where the desk reads it back from.
///
/// Nothing started from here can gain privileges either. The image still
/// carries the base system's setuid programs (`su`, `mount`, `passwd`), and
/// a flaw in one of them would make a job root over this binary and the
/// browser's extension. A runtime that sets `no-new-privileges` already
/// refuses that; this makes it true under the ones that do not.
fn seal() {
    // SAFETY: no thread has been started and nothing has been forked yet, so
    // nothing else is reading the environment.
    unsafe { std::env::remove_var("HOTLINE_COMPUTER_TOKEN") };
    hide();
    #[cfg(target_os = "linux")]
    // SAFETY: a flag on this process and its descendants, taking no pointers.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        eprintln!(
            "hotline-computer: could not refuse new privileges: {}",
            std::io::Error::last_os_error()
        );
    }
}

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    // Chromium starts the extension's native messaging host with the
    // extension's origin as its first argument.
    if arguments
        .first()
        .is_some_and(|first| first.starts_with("chrome-extension://"))
    {
        // The relay carries a login's password from the service to the
        // extension, so its memory is kept from jobs as the service's is.
        hide();
        if let Err(error) = hotline_computer::bridge::relay() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if matches!(arguments.first().map(String::as_str), Some("--version")) {
        println!("hotline-computer {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arguments.is_empty()
        || matches!(arguments.first().map(String::as_str), Some("--help" | "-h"))
    {
        println!("{}", usage());
        return;
    }
    if matches!(
        arguments.first().map(String::as_str),
        Some("observe" | "activity")
    ) {
        // These windows read the jobs' output and the activity record, and
        // run as the jobs' own user: sealed as the service is, so no job
        // reads their memory or `/proc` entries.
        seal();
        let home = arguments
            .get(1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Config::from_env().home);
        let run = if arguments[0] == "activity" {
            hotline_computer::activity::run
        } else {
            hotline_computer::observer::run
        };
        if let Err(error) = run(&home) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.first().map(String::as_str) == Some("shell") {
        let home = arguments
            .get(1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Config::from_env().home);
        let at = arguments.get(2).map(std::path::PathBuf::from);
        if let Err(error) = hotline_computer::observer::shell(&home, at.as_deref()) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.first().map(String::as_str) == Some("open") {
        let Some(target) = arguments.get(1) else {
            eprintln!("open takes a folder or a web address");
            std::process::exit(2);
        };
        let opened = if hotline_computer::links::is_link(target) {
            hotline_computer::links::send(target)
        } else {
            // A folder opens in the file manager; with no service to answer,
            // the person's terminal in that folder still does.
            let target = std::path::Path::new(target);
            hotline_computer::files_ui::open_folder(target)
                .or_else(|_| hotline_computer::observer::open(&Config::from_env().home, target))
        };
        if let Err(error) = opened {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.first().map(String::as_str) == Some("artifact") {
        use std::os::unix::process::CommandExt;
        let Some(spec) = arguments.get(1) else {
            eprintln!("artifact requires JSON input");
            std::process::exit(2);
        };
        let error = std::process::Command::new("python3")
            .args(["-u", "-c", include_str!("../assets/artifact.py"), spec])
            .exec();
        eprintln!("artifact: {error}");
        std::process::exit(1);
    }
    if arguments.first().map(String::as_str) == Some("packages") {
        print!("{}", hotline_computer::workspace::catalog_text());
        return;
    }
    if arguments.first().map(String::as_str) == Some("prepare") {
        use hotline_computer::workspace::{self, Operator};
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        // The managed job spells it @request (or DEFINITION_JSON) WORKSPACE
        // HOME; a person at the terminal spells it with flags.
        let internal = arguments.len() == 4
            && (arguments[1].starts_with('{') || arguments[1] == workspace::REQUEST);
        let result = if internal {
            runtime.block_on(workspace::build(
                std::path::Path::new(&arguments[3]),
                &arguments[1],
                std::path::Path::new(&arguments[2]),
            ))
        } else {
            let cwd = std::env::current_dir().unwrap_or_else(|_| Config::from_env().home);
            match workspace::operator(&arguments[1..], &cwd) {
                Ok(Operator::Help) => {
                    println!("{}", workspace::OPERATOR_USAGE);
                    return;
                }
                Ok(Operator::Catalog) => {
                    print!("{}", workspace::catalog_text());
                    return;
                }
                Ok(Operator::Prepare {
                    workspace: directory,
                    packages,
                    flake,
                }) => runtime.block_on(workspace::prepare_here(
                    &Config::from_env().home,
                    &directory,
                    packages,
                    flake,
                )),
                Err(message) => {
                    eprintln!("{message}");
                    std::process::exit(2);
                }
            }
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    // The split computer's two halves, which PID 1 starts as their users
    // with their lines at fixed descriptors (see `split`).
    if let Some(half @ ("__broker" | "__runner")) = arguments.first().map(String::as_str) {
        let config = Config::from_env();
        let expected = if half == "__broker" {
            hotline_computer::split::BROKER_UID
        } else {
            hotline_computer::split::AGENT_UID
        };
        // SAFETY: geteuid takes nothing and cannot fail.
        if unsafe { libc::geteuid() } != expected {
            eprintln!("hotline-computer: {half} is started by the computer's PID 1");
            std::process::exit(2);
        }
        if half == "__broker"
            && let Err(message) = config.door()
        {
            eprintln!("hotline-computer: {message}");
            std::process::exit(2);
        }
        seal();
        let result = if half == "__broker" {
            hotline_computer::split::broker::run(config)
        } else {
            hotline_computer::split::runner::run(config)
        };
        if let Err(error) = result {
            eprintln!("hotline-computer: {half}: {error}");
            std::process::exit(1);
        }
        return;
    }
    let (command, config) = match parse() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if let Err(message) = config.door() {
        eprintln!("hotline-computer: {message}");
        std::process::exit(2);
    }
    seal();
    let result = match command {
        // Started as root, the computer splits into its users; started as
        // anyone else, it is the single-user computer it always was.
        #[cfg(target_os = "linux")]
        Command::Boot if hotline_computer::split::booted_as_root() => {
            hotline_computer::split::init::run(config)
        }
        Command::Boot => boot::run(config),
        Command::Serve => Display::open(&config.display).and_then(|display| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime.block_on(serve::run(App::new(config).with_display(display)))
                })
        }),
    };
    if let Err(error) = result {
        eprintln!("hotline-computer: {error}");
        std::process::exit(1);
    }
}
