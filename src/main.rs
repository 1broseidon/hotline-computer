use toad_computer::display::Display;
use toad_computer::{App, Config, boot, serve};

fn usage() -> &'static str {
    "Usage: toad-computer <boot|serve> [--addr ADDRESS] [--token TOKEN] [--home PATH] [--display DISPLAY] [--screen WIDTHxHEIGHT]\n\n  boot   start the display, the session bus and the desktop, then serve; the container entrypoint\n  serve  serve on a display that already exists"
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
    if arguments.first().map(String::as_str) == Some("prepare") {
        if arguments.len() != 4 {
            eprintln!("prepare requires DEFINITION_JSON WORKSPACE HOME (internal managed job)");
            std::process::exit(2);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        if let Err(error) = runtime.block_on(toad_computer::workspace::build(
            std::path::Path::new(&arguments[3]),
            &arguments[1],
            std::path::Path::new(&arguments[2]),
        )) {
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
