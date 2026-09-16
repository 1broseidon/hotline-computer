//! Booting the machine: the display, the session bus, the desktop, then the
//! agent. `toad-computer boot` is the container's entrypoint.
//!
//! As PID 1 the process forks first. The parent stays a reaper: it collects
//! every child the kernel hands it, forwards SIGTERM to the agent, and exits
//! with the agent's status. The child is the agent, which starts Xvfb and
//! dbus-daemon, becomes the window manager, and serves MCP. A browser helper
//! that outlives its parent, or a program `shell launch` started, is
//! re-parented to PID 1 and collected there instead of lingering as a zombie.
//! A machine whose display or bus has died is a dead machine: the agent exits
//! and the container with it, and Toad starts a fresh one.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::display::Display;
use crate::{App, Config, desktop, serve};

const RUNTIME_DIR: &str = "/tmp/toad-computer";
const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);
const SHUTDOWN: Duration = Duration::from_secs(2);

pub fn run(config: Config) -> Result<(), String> {
    let (width, height) = parse_screen(&config.screen)?;
    if std::process::id() == 1 {
        become_init();
    }
    machine(config, width, height)
}

fn parse_screen(screen: &str) -> Result<(u16, u16), String> {
    let invalid = || format!("--screen must be WIDTHxHEIGHT, not {screen:?}");
    let (width, height) = screen.split_once('x').ok_or_else(invalid)?;
    let width: u16 = width.parse().map_err(|_| invalid())?;
    let height: u16 = height.parse().map_err(|_| invalid())?;
    if width < 640 || height < 480 {
        return Err(format!("--screen must be at least 640x480, not {screen:?}"));
    }
    Ok((width, height))
}

/// Fork. The parent becomes the reaper and never returns; the child returns
/// to become the agent.
fn become_init() {
    // SAFETY: the process is still single-threaded, so fork is sound, and
    // the signal set is a plain C struct this function alone touches.
    unsafe {
        let mut signals: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut signals);
        for signal in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT] {
            libc::sigaddset(&mut signals, signal);
        }
        // Blocked before the fork so an agent that dies at once leaves a
        // pending SIGCHLD rather than a dropped one.
        libc::sigprocmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut());
        let agent = libc::fork();
        if agent < 0 {
            eprintln!("toad-computer: fork failed");
            std::process::exit(1);
        }
        if agent == 0 {
            libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut());
            return;
        }
        loop {
            let mut signal = 0;
            if libc::sigwait(&signals, &mut signal) != 0 {
                continue;
            }
            if signal == libc::SIGCHLD {
                loop {
                    let mut status = 0;
                    let pid = libc::waitpid(-1, &mut status, libc::WNOHANG);
                    if pid <= 0 {
                        break;
                    }
                    if pid == agent {
                        std::process::exit(exit_code(status));
                    }
                }
            } else {
                libc::kill(agent, libc::SIGTERM);
            }
        }
    }
}

fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

fn machine(config: Config, width: u16, height: u16) -> Result<(), String> {
    std::fs::create_dir_all(RUNTIME_DIR)
        .map_err(|error| format!("create {RUNTIME_DIR}: {error}"))?;
    let bus_path = Path::new(RUNTIME_DIR).join("bus");
    let bus_address = format!("unix:path={}", bus_path.display());
    // SAFETY: no other thread exists yet. Chromium and the accessibility
    // client find the display and the bus through the environment, so both
    // are set before anything is spawned.
    unsafe {
        std::env::set_var("DISPLAY", &config.display);
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus_address);
    }

    // A hard container stop leaves sockets and X's PID lock behind. PIDs are
    // reused on restart, so the lock's PID cannot establish that X is alive.
    let display_socket = x_socket(&config.display)?;
    remove_stale_socket(&display_socket)?;
    let number = display_socket.file_name().unwrap().to_string_lossy();
    remove_if_present(&PathBuf::from(format!("/tmp/.{number}-lock")))?;
    remove_stale_socket(&bus_path)?;

    let mut xvfb = spawn(
        "Xvfb",
        &[
            &config.display,
            "-screen",
            "0",
            &format!("{width}x{height}x24"),
            "-nolisten",
            "tcp",
            "-noreset",
            "-dpi",
            "96",
            "-fp",
            "/usr/share/fonts/X11/misc,built-ins",
        ],
    )?;
    wait_for(&x_socket(&config.display)?, "the display", &mut xvfb)?;
    wait_for_x(&config.display, &mut xvfb)?;
    ensure_keymap(&config.display)?;

    let mut dbus = spawn(
        "dbus-daemon",
        &[
            "--session",
            "--nofork",
            "--nopidfile",
            &format!("--address={bus_address}"),
        ],
    )?;
    wait_for(&bus_path, "the session bus", &mut dbus)?;
    let keyring = spawn_keyring()?;
    let keyring_pid = Arc::new(AtomicU32::new(keyring.id()));

    let app = App::new(config.clone()).with_display(Display::open(&config.display)?);
    let (requests, mut incoming) = tokio::sync::mpsc::unbounded_channel();
    let (ready, desktop_ready) = mpsc::channel();
    let display = config.display.clone();
    std::thread::Builder::new()
        .name("desktop".to_owned())
        .spawn(move || {
            if let Err(error) = desktop::run(&display, requests, ready) {
                eprintln!("toad-computer: desktop: {error}");
            }
        })
        .map_err(|error| format!("spawn desktop thread: {error}"))?;
    match desktop_ready.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("desktop startup: {error}")),
        Err(_) => return Err("the desktop did not come up in time".into()),
    }

    let xvfb_pid = xvfb.id();
    let dbus_pid = dbus.id();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let keeper_pid = keyring_pid.clone();
    let result = runtime.block_on(async {
        tokio::time::timeout(START_TIMEOUT, secrets_on_the_bus())
            .await
            .map_err(|_| "the Secret Service did not appear on the bus".to_owned())??;
        tokio::time::timeout(START_TIMEOUT, crate::a11y::connection(&app))
            .await
            .map_err(|_| "accessibility bus did not start".to_owned())??;
        let dock = {
            let app = app.clone();
            async move {
                while let Some(request) = incoming.recv().await {
                    match request {
                        desktop::Request::OpenBrowser => {
                            if let Err(error) = app.browser.open().await {
                                eprintln!("toad-computer: dock: {error}");
                            }
                        }
                        desktop::Request::BrowserClosed => app.browser.forget().await,
                        desktop::Request::OpenTerminal => {
                            if let Err(error) = app.observer.select(None).await {
                                eprintln!("toad-computer: terminal: {error}");
                            }
                        }
                        desktop::Request::OpenShell => {
                            if let Err(error) = app.observer.open_shell().await {
                                eprintln!("toad-computer: shell: {error}");
                            }
                        }
                        desktop::Request::OpenJob(job_id) => {
                            if let Err(error) = app.observer.select(Some(&job_id)).await {
                                eprintln!("toad-computer: terminal: {error}");
                            }
                        }
                    }
                }
            }
        };
        tokio::select! {
            result = serve::run(app) => result,
            status = tokio::task::spawn_blocking(move || xvfb.wait()) => {
                Err(format!("Xvfb exited: {}", describe(status)))
            }
            status = tokio::task::spawn_blocking(move || dbus.wait()) => {
                Err(format!("dbus-daemon exited: {}", describe(status)))
            }
            () = dock => Ok(()),
            () = keep_keyring(keyring, keeper_pid) => Ok(()),
        }
    });
    // Only PIDs captured at spawn are ever signalled.
    for pid in [xvfb_pid, dbus_pid, keyring_pid.load(Ordering::Relaxed)] {
        if pid == 0 {
            continue;
        }
        // SAFETY: kill with a PID this process spawned is a plain syscall.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    runtime.shutdown_timeout(SHUTDOWN);
    result
}

fn describe(
    status: Result<std::io::Result<std::process::ExitStatus>, tokio::task::JoinError>,
) -> String {
    match status {
        Ok(Ok(status)) => status.to_string(),
        Ok(Err(error)) => error.to_string(),
        Err(error) => error.to_string(),
    }
}

/// The Secret Service: gnome-keyring on the session bus, its login
/// collection unlocked, so a native app or `secret-tool` that stores a
/// password gets a keyring and never a prompt on the desktop. The password
/// is empty. The home volume is the boundary around a computer's secrets,
/// as it is for everything else the person and the agent keep there.
fn spawn_keyring() -> Result<Child, String> {
    let mut child = Command::new("gnome-keyring-daemon")
        .args(["--foreground", "--components=secrets", "--unlock"])
        .stdin(Stdio::piped())
        // It prints the control socket's location for a shell to export;
        // clients here find it on the bus.
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| format!("gnome-keyring-daemon: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write as _;
        // A daemon that has already gone is noticed by whoever waits on it.
        let _ = stdin.write_all(b"\n");
    }
    Ok(child)
}

/// A client asking for secrets before the daemon owns the name would have
/// the bus activate a second, locked keyring in its place.
async fn secrets_on_the_bus() -> Result<(), String> {
    use atspi::zbus;
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| format!("session bus: {error}"))?;
    let bus = zbus::fdo::DBusProxy::new(&connection)
        .await
        .map_err(|error| format!("session bus: {error}"))?;
    let name = zbus::names::BusName::try_from("org.freedesktop.secrets")
        .map_err(|error| error.to_string())?;
    loop {
        if bus
            .name_has_owner(name.clone())
            .await
            .map_err(|error| format!("ask the bus for the Secret Service: {error}"))?
        {
            return Ok(());
        }
        tokio::time::sleep(POLL).await;
    }
}

/// A keyring that dies takes nobody's secrets with it; the file is in the
/// home. It comes back unlocked, and apps find it where they left it.
async fn keep_keyring(mut child: Child, pid: Arc<AtomicU32>) {
    loop {
        let status = tokio::task::spawn_blocking(move || child.wait()).await;
        pid.store(0, Ordering::Relaxed);
        eprintln!(
            "toad-computer: gnome-keyring-daemon exited: {}; starting it again",
            describe(status)
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        loop {
            match spawn_keyring() {
                Ok(next) => {
                    pid.store(next.id(), Ordering::Relaxed);
                    child = next;
                    break;
                }
                Err(error) => {
                    eprintln!("toad-computer: {error}; trying again in a minute");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
        }
    }
}

/// Xvfb loads a pc105/us keymap on its own. A desk reported a display with
/// none, which no GTK client can type into; if it ever happens here, the
/// map is loaded explicitly and the log says so.
fn ensure_keymap(display: &str) -> Result<(), String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    let (connection, _) =
        x11rb::connect(Some(display)).map_err(|error| format!("x11 connect: {error}"))?;
    let (min, max) = (
        connection.setup().min_keycode,
        connection.setup().max_keycode,
    );
    let mapping = connection
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| format!("keyboard mapping: {error}"))?;
    let mapped = mapping
        .keysyms
        .chunks(usize::from(mapping.keysyms_per_keycode).max(1))
        .filter(|columns| columns.iter().any(|&keysym| keysym != 0))
        .count();
    if mapped >= 100 {
        return Ok(());
    }
    eprintln!("toad-computer: the display came up with {mapped} mapped keycodes; loading pc105/us");
    let status = Command::new("setxkbmap")
        .args(["-model", "pc105", "-layout", "us"])
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("setxkbmap: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("setxkbmap: {status}"))
    }
}

fn spawn(program: &str, arguments: &[&str]) -> Result<Child, String> {
    Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| format!("{program}: {error}"))
}

fn x_socket(display: &str) -> Result<PathBuf, String> {
    let number = display
        .strip_prefix(':')
        .and_then(|rest| rest.split('.').next())
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| format!("boot needs a local display like :0, not {display:?}"))?;
    Ok(PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove stale {}: {error}", path.display())),
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => Err(format!(
            "{} is already serving; refusing to replace it",
            path.display()
        )),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            remove_if_present(path)
        }
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

fn wait_for(path: &Path, what: &str, child: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + START_TIMEOUT;
    while !path.exists() {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("{what} exited before it was ready: {status}"));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "{what} did not appear at {} within {}s",
                path.display(),
                START_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

/// The socket exists a moment before the server accepts on it.
fn wait_for_x(display: &str, xvfb: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if x11rb::connect(Some(display)).is_ok() {
            return Ok(());
        }
        if let Some(status) = xvfb.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("the display exited before it was ready: {status}"));
        }
        if Instant::now() > deadline {
            return Err("the display did not accept a connection in time".to_owned());
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dead_endpoints_are_removed() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("bus");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(remove_stale_socket(&socket).is_err());
        assert!(socket.exists());
        drop(listener);
        remove_stale_socket(&socket).unwrap();
        assert!(!socket.exists());
        remove_stale_socket(&socket).unwrap();
    }

    #[test]
    fn a_screen_is_width_by_height() {
        assert_eq!(parse_screen("1920x1080").unwrap(), (1920, 1080));
        assert!(parse_screen("1920").is_err());
        assert!(parse_screen("320x200").is_err());
    }

    #[test]
    fn the_display_socket_is_named_by_its_number() {
        assert_eq!(x_socket(":0").unwrap(), PathBuf::from("/tmp/.X11-unix/X0"));
        assert_eq!(
            x_socket(":12.0").unwrap(),
            PathBuf::from("/tmp/.X11-unix/X12")
        );
        assert!(x_socket("localhost:0").is_err());
    }
}
