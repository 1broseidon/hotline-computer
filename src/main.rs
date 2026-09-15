use toad_computer::display::Display;
use toad_computer::{App, Config, boot, serve};

fn usage() -> &'static str {
    "Usage: toad-computer <boot|serve> [--addr ADDRESS] [--token TOKEN] [--home PATH] [--display DISPLAY] [--screen WIDTHxHEIGHT]\n\n  boot   start the display, the session bus and the desktop, then serve; the container entrypoint\n  serve  serve on a display that already exists\n\nAt the person's terminal:\n  toad-computer prepare [--workspace DIR] [--packages NAME... | --flake DIR]\n  toad-computer packages   common Nixpkgs names to choose from"
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

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if matches!(arguments.first().map(String::as_str), Some("--version")) {
        println!("toad-computer {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if arguments.is_empty()
        || matches!(arguments.first().map(String::as_str), Some("--help" | "-h"))
    {
        println!("{}", usage());
        return;
    }
    if arguments.first().map(String::as_str) == Some("observe") {
        let home = arguments
            .get(1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Config::from_env().home);
        if let Err(error) = toad_computer::observer::run(&home) {
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
        if let Err(error) = toad_computer::observer::shell(&home) {
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
        print!("{}", toad_computer::workspace::catalog_text());
        return;
    }
    if arguments.first().map(String::as_str) == Some("prepare") {
        use toad_computer::workspace::{self, Operator};
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        // The managed job spells it DEFINITION_JSON WORKSPACE HOME; a person
        // at the terminal spells it with flags.
        let internal = arguments.len() == 4 && arguments[1].starts_with('{');
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
    let (command, config) = match parse() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let result = match command {
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
        eprintln!("toad-computer: {error}");
        std::process::exit(1);
    }
}
