//! PID 1 of the split computer: root with `SETUID` and `SETGID` and nothing
//! else. It binds the door (`:8787`) and the proxy (`127.0.0.1:3128`) and
//! keeps them for the container's life, so a job can never take either port
//! even while a process restarts; starts the broker as `hotline` and the
//! runner as `agent`, each with its ends of the internal lines; and then only
//! reaps, starts the signed-in browser as `signin` when the broker asks
//! (fd 5), and stops the container when the broker or the runner exits.
//!
//! It runs nothing a job wrote and reads nothing in the agent's home; the
//! only thing it listens to is the broker's line.

use std::ffi::{CString, OsString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    AGENT_UID, BRIDGE_DIR, BROKER_DIR, BROKER_UID, PROXY_ADDR, PUBLIC_DIR, RUN_DIR, SIGNIN_DIR,
    SIGNIN_PROFILE, SIGNIN_UID, wire,
};
use crate::Config;

const CAP_SETGID: u32 = 6;
const CAP_SETUID: u32 = 7;
const CHROMIUM: &str = "/usr/bin/chromium";
const STOP_PATIENCE: Duration = Duration::from_secs(5);

/// One process PID 1 starts: who it runs as, where, and the descriptors it
/// gets at which numbers.
struct Start {
    uid: u32,
    groups: Vec<libc::gid_t>,
    cwd: CString,
    program: CString,
    argv: Vec<CString>,
    envp: Vec<CString>,
    fds: Vec<(RawFd, RawFd)>,
    quiet: bool,
}

pub fn run(config: Config) -> Result<(), String> {
    keep_only_setid()?;
    prepare_directories()?;
    let door = listen(&config.addr)?;
    // A job must not answer for the door over IPv6 either.
    let _door_v6 = hold_v6(&config.addr);
    let proxy = listen(PROXY_ADDR)?;
    let (broker_line, runner_line) = wire::pair()?;
    let (handoff, handed) = wire::pair()?;
    let (init_line, broker_init) = wire::pair()?;
    let signals = block_signals()?;
    let program = CString::new(
        std::env::current_exe()
            .map_err(|error| format!("this program: {error}"))?
            .as_os_str()
            .as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    let base: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(key, _)| key != "HOTLINE_COMPUTER_TOKEN")
        .collect();
    let mut broker_env = environment(
        &base,
        &config,
        &[
            ("HOME", BROKER_DIR),
            ("USER", "hotline"),
            ("LOGNAME", "hotline"),
        ],
    );
    if let Some(token) = &config.token {
        broker_env.push(cstring(&format!("HOTLINE_COMPUTER_TOKEN={token}"))?);
    }
    let home = config.home.to_string_lossy().into_owned();
    let runner_env = environment(
        &base,
        &config,
        &[("HOME", &home), ("USER", "agent"), ("LOGNAME", "agent")],
    );
    let broker = spawn(&Start {
        uid: BROKER_UID,
        groups: vec![],
        cwd: cstring(BROKER_DIR)?,
        program: program.clone(),
        argv: vec![cstring("hotline-computer")?, cstring("__broker")?],
        envp: broker_env,
        fds: vec![
            (super::fd::LINE, broker_line.as_raw_fd()),
            (super::fd::DOOR, door.as_raw_fd()),
            (super::fd::INIT, broker_init.as_raw_fd()),
            (super::fd::PROXY, proxy.as_raw_fd()),
            (super::fd::HANDOFF, handoff.as_raw_fd()),
        ],
        quiet: false,
    })?;
    let runner = spawn(&Start {
        uid: AGENT_UID,
        groups: vec![],
        cwd: cstring(&home)?,
        program,
        argv: vec![cstring("hotline-computer")?, cstring("__runner")?],
        envp: runner_env,
        fds: vec![
            (super::fd::LINE, runner_line.as_raw_fd()),
            (super::fd::HANDED, handed.as_raw_fd()),
        ],
        quiet: false,
    })?;
    // The children hold their ends now; PID 1 keeps only the listeners and
    // its own end of the broker's line.
    drop((broker_line, runner_line, handoff, handed, broker_init));
    eprintln!("hotline-computer: split boot: broker {broker} as hotline, runner {runner} as agent");
    let status = supervise(&signals, init_line, broker, runner);
    drop((door, proxy));
    std::process::exit(status);
}

fn cstring(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| format!("{text:?} holds a NUL"))
}

fn environment(
    base: &[(OsString, OsString)],
    config: &Config,
    overrides: &[(&str, &str)],
) -> Vec<CString> {
    let screen = config.screen.clone();
    let home = config.home.to_string_lossy().into_owned();
    let fixed: Vec<(&str, &str)> = overrides
        .iter()
        .copied()
        .chain([
            ("HOTLINE_COMPUTER_ADDR", config.addr.as_str()),
            ("HOTLINE_COMPUTER_HOME", home.as_str()),
            ("HOTLINE_COMPUTER_SCREEN", screen.as_str()),
            ("DISPLAY", config.display.as_str()),
        ])
        .collect();
    let mut out = Vec::new();
    for (key, value) in base {
        if fixed
            .iter()
            .any(|(fixed, _)| key.as_bytes() == fixed.as_bytes())
        {
            continue;
        }
        let mut entry = key.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        if let Ok(entry) = CString::new(entry) {
            out.push(entry);
        }
    }
    for (key, value) in fixed {
        if let Ok(entry) = CString::new(format!("{key}={value}")) {
            out.push(entry);
        }
    }
    out
}

// ---- capabilities ---------------------------------------------------------

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: libc::c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

const CAPABILITY_VERSION_3: u32 = 0x2008_0522;

/// Whatever the runtime granted, PID 1 keeps `SETUID` and `SETGID` and
/// nothing else, and refuses to go on without them.
fn keep_only_setid() -> Result<(), String> {
    for cap in 0..64 {
        if cap == CAP_SETUID || cap == CAP_SETGID {
            continue;
        }
        // SAFETY: a prctl with integer arguments. Dropping from the bounding
        // set needs SETPCAP, which a runtime that dropped everything did not
        // give; then those are already out of reach.
        let dropped = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
        if dropped != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
            break;
        }
    }
    // SAFETY: as above.
    unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
            0,
            0,
            0,
        );
    }
    let mut header = CapHeader {
        version: CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [CapData::default(); 2];
    // SAFETY: capget fills two CapData for a version 3 header.
    if unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) } != 0 {
        return Err(format!("capget: {}", std::io::Error::last_os_error()));
    }
    let wanted = (1 << CAP_SETUID) | (1 << CAP_SETGID);
    if data[0].permitted & wanted != wanted {
        return Err("booted as root without SETUID and SETGID: create the computer with --user 0 --cap-add SETUID --cap-add SETGID, or boot it as agent for the single-user computer".to_owned());
    }
    let kept = [
        CapData {
            effective: wanted,
            permitted: wanted,
            inheritable: 0,
        },
        CapData::default(),
    ];
    header.version = CAPABILITY_VERSION_3;
    header.pid = 0;
    // SAFETY: capset reads two CapData for a version 3 header.
    if unsafe { libc::syscall(libc::SYS_capset, &header, kept.as_ptr()) } != 0 {
        return Err(format!("capset: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

// ---- directories ----------------------------------------------------------

/// The image makes these; a runtime that mounts a fresh `/run` loses them,
/// and root without `CHOWN` cannot give one away, so each missing one is
/// made by its owner in a parent that is briefly writable, before any
/// other process is running.
fn prepare_directories() -> Result<(), String> {
    // Root without DAC capabilities sees into none of these, so each is
    // checked from its parent; what is inside is its owner's business.
    let wanted: [(&str, u32, u32, u32); 4] = [
        (BROKER_DIR, BROKER_UID, BROKER_UID, 0o700),
        (BRIDGE_DIR, BROKER_UID, SIGNIN_UID, 0o2750),
        (PUBLIC_DIR, BROKER_UID, BROKER_UID, 0o755),
        (SIGNIN_DIR, SIGNIN_UID, SIGNIN_UID, 0o700),
    ];
    for directory in [RUN_DIR, "/var/lib/hotline-computer"] {
        if !Path::new(directory).is_dir() {
            std::fs::create_dir_all(directory)
                .map_err(|error| format!("create {directory}: {error}"))?;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("{directory}: {error}"))?;
        }
    }
    for (path, uid, gid, mode) in wanted {
        let path = Path::new(path);
        if !path.exists() {
            make_owned(path, uid, gid, mode)?;
        }
        let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if metadata.uid() != uid || metadata.gid() != gid || metadata.mode() & 0o7777 != mode {
            return Err(format!(
                "{} is {}:{} {:o}, not {uid}:{gid} {mode:o}; refusing to start",
                path.display(),
                metadata.uid(),
                metadata.gid(),
                metadata.mode() & 0o7777
            ));
        }
    }
    Ok(())
}

fn make_owned(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<(), String> {
    let parent = path.parent().expect("an absolute path has a parent");
    let before = std::fs::metadata(parent)
        .map_err(|error| format!("{}: {error}", parent.display()))?
        .mode()
        & 0o7777;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o1777))
        .map_err(|error| format!("{}: {error}", parent.display()))?;
    let target = cstring(&path.to_string_lossy())?;
    // The signed-in browser's profile is made with its directory.
    let inner = (path == Path::new(SIGNIN_DIR))
        .then(|| cstring(SIGNIN_PROFILE))
        .transpose()?;
    // SAFETY: fork from this single-threaded process; the child makes the
    // directory as its owner and exits without returning.
    let made = unsafe {
        let child = libc::fork();
        if child == 0 {
            let groups = [gid as libc::gid_t];
            let ok = libc::setgroups(1, groups.as_ptr()) == 0
                && libc::setresgid(gid, gid, gid) == 0
                && libc::setresuid(uid, uid, uid) == 0
                && libc::mkdir(target.as_ptr(), 0o700) == 0
                && libc::chown(target.as_ptr(), u32::MAX, gid) == 0
                && libc::chmod(target.as_ptr(), mode as libc::mode_t) == 0
                && inner
                    .as_ref()
                    .is_none_or(|inner| libc::mkdir(inner.as_ptr(), 0o700) == 0);
            libc::_exit(if ok { 0 } else { 1 });
        }
        let mut status = 0;
        child > 0 && libc::waitpid(child, &mut status, 0) == child && status == 0
    };
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(before))
        .map_err(|error| format!("{}: {error}", parent.display()))?;
    if made {
        Ok(())
    } else {
        Err(format!("could not make {} as {uid}", path.display()))
    }
}

// ---- sockets --------------------------------------------------------------

fn listen(address: &str) -> Result<OwnedFd, String> {
    let listener =
        std::net::TcpListener::bind(address).map_err(|error| format!("bind {address}: {error}"))?;
    Ok(OwnedFd::from(listener))
}

/// `[::]:<port>` for IPv6 alone, held and never answered, where the
/// runtime offers IPv6 at all.
fn hold_v6(address: &str) -> Option<OwnedFd> {
    let port: u16 = address.rsplit_once(':')?.1.parse().ok()?;
    if !address.starts_with("0.0.0.0:") {
        return None;
    }
    // SAFETY: plain socket calls on a descriptor made here; the address is
    // a zeroed sockaddr_in6 with only the family and port set.
    unsafe {
        let socket = libc::socket(libc::AF_INET6, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if socket < 0 {
            return None;
        }
        let socket = OwnedFd::from_raw_fd(socket);
        let on: libc::c_int = 1;
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IPV6,
            libc::IPV6_V6ONLY,
            (&on as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        let mut any: libc::sockaddr_in6 = std::mem::zeroed();
        any.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        any.sin6_port = port.to_be();
        if libc::bind(
            socket.as_raw_fd(),
            (&any as *const libc::sockaddr_in6).cast(),
            std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
        ) != 0
            || libc::listen(socket.as_raw_fd(), 1) != 0
        {
            return None;
        }
        Some(socket)
    }
}

// ---- processes ------------------------------------------------------------

fn block_signals() -> Result<OwnedFd, String> {
    // SAFETY: the signal set is a plain C struct; signalfd returns a new
    // descriptor that is owned here.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT] {
            libc::sigaddset(&mut set, signal);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC);
        if fd < 0 {
            return Err(format!("signalfd: {}", std::io::Error::last_os_error()));
        }
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

/// Forks and execs `start`. In the child only system calls run: the
/// descriptors go to their numbers, everything else closes, the user and
/// groups change, new privileges are refused, and the program replaces it.
fn spawn(start: &Start) -> Result<libc::pid_t, String> {
    let mut argv: Vec<*const libc::c_char> = start.argv.iter().map(|arg| arg.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const libc::c_char> = start.envp.iter().map(|env| env.as_ptr()).collect();
    envp.push(std::ptr::null());
    let highest = start
        .fds
        .iter()
        .map(|(target, _)| *target)
        .max()
        .unwrap_or(2);
    let null = c"/dev/null";
    let groups = start.groups.clone();
    if start.fds.len() > 16 {
        return Err("too many descriptors for one process".to_owned());
    }
    // SAFETY: fork from a single-threaded process; the child calls only
    // async-signal-safe functions on memory prepared before the fork, and
    // never returns.
    unsafe {
        let child = libc::fork();
        if child < 0 {
            return Err(format!("fork: {}", std::io::Error::last_os_error()));
        }
        if child > 0 {
            return Ok(child);
        }
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
        // Out of the way first, so no source is overwritten by a target.
        let mut moved = [0 as RawFd; 16];
        for (index, (_, source)) in start.fds.iter().enumerate() {
            moved[index] = libc::fcntl(*source, libc::F_DUPFD, 64);
            if moved[index] < 0 {
                libc::_exit(126);
            }
        }
        for (index, (target, _)) in start.fds.iter().enumerate() {
            if libc::dup2(moved[index], *target) < 0 {
                libc::_exit(126);
            }
        }
        if start.quiet {
            let devnull = libc::open(null.as_ptr(), libc::O_RDWR);
            for target in 0..3 {
                libc::dup2(devnull, target);
            }
        }
        if libc::syscall(libc::SYS_close_range, (highest + 1) as u32, u32::MAX, 0) != 0 {
            for fd in (highest + 1)..4096 {
                libc::close(fd);
            }
        }
        if libc::setgroups(groups.len(), groups.as_ptr()) != 0
            || libc::setresgid(start.uid, start.uid, start.uid) != 0
            || libc::setresuid(start.uid, start.uid, start.uid) != 0
            || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            || libc::chdir(start.cwd.as_ptr()) != 0
        {
            libc::_exit(126);
        }
        libc::execve(start.program.as_ptr(), argv.as_ptr(), envp.as_ptr());
        libc::_exit(127);
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

/// Reaps, answers the broker, and decides when the computer is over: when
/// the broker or the runner exits, or the container is stopped. Answers the
/// status to exit with.
fn supervise(signals: &OwnedFd, line: OwnedFd, broker: libc::pid_t, runner: libc::pid_t) -> i32 {
    let mut line = Some(std::os::unix::net::UnixStream::from(line));
    let mut signins: Vec<libc::pid_t> = Vec::new();
    let mut alive = vec![broker, runner];
    let status = loop {
        let mut polled = [
            libc::pollfd {
                fd: signals.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: line.as_ref().map_or(-1, |line| line.as_raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: poll over the two entries above.
        if unsafe { libc::poll(polled.as_mut_ptr(), 2, -1) } < 0 {
            continue;
        }
        if polled[0].revents & libc::POLLIN != 0 {
            match next_signal(signals) {
                Some(libc::SIGCHLD) => {
                    if let Some(status) = reap(&mut alive, &mut signins) {
                        eprintln!("hotline-computer: a half of the computer exited; stopping");
                        break status;
                    }
                }
                Some(libc::SIGTERM | libc::SIGINT) => break 0,
                _ => {}
            }
        }
        if polled[1].revents != 0
            && let Some(stream) = line.as_mut()
        {
            match wire::read_blocking(stream) {
                Ok(Some(request)) => {
                    if let Err(error) = answer(stream, &request, &mut signins) {
                        eprintln!("hotline-computer: the broker's line: {error}");
                    }
                }
                // The broker is gone; its exit follows as a SIGCHLD.
                Ok(None) | Err(_) => line = None,
            }
        }
    };
    stop(signals, &mut alive, &mut signins);
    status
}

fn next_signal(signals: &OwnedFd) -> Option<i32> {
    // SAFETY: reads one signalfd_siginfo into a zeroed struct of that size.
    unsafe {
        let mut info: libc::signalfd_siginfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::signalfd_siginfo>();
        let read = libc::read(
            signals.as_raw_fd(),
            (&mut info as *mut libc::signalfd_siginfo).cast(),
            size,
        );
        (read == size as isize).then_some(info.ssi_signo as i32)
    }
}

/// Collects every child that has exited. A broker or a runner among them
/// ends the computer, with its status.
fn reap(alive: &mut Vec<libc::pid_t>, signins: &mut Vec<libc::pid_t>) -> Option<i32> {
    let mut over = None;
    loop {
        let mut status = 0;
        // SAFETY: waitpid on any child, without blocking.
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid <= 0 {
            return over;
        }
        signins.retain(|signin| *signin != pid);
        if alive.contains(&pid) {
            alive.retain(|child| *child != pid);
            over.get_or_insert(match exit_code(status) {
                0 => 1,
                code => code,
            });
        }
    }
}

/// SIGTERM to everyone still running, a few seconds to go, then SIGKILL.
fn stop(signals: &OwnedFd, alive: &mut Vec<libc::pid_t>, signins: &mut Vec<libc::pid_t>) {
    for pid in alive.iter().chain(signins.iter()) {
        // SAFETY: kill with a PID this process started.
        unsafe { libc::kill(*pid, libc::SIGTERM) };
    }
    let deadline = Instant::now() + STOP_PATIENCE;
    while !(alive.is_empty() && signins.is_empty()) && Instant::now() < deadline {
        let mut polled = [libc::pollfd {
            fd: signals.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: poll over one entry.
        unsafe { libc::poll(polled.as_mut_ptr(), 1, 100) };
        let _ = next_signal(signals);
        reap(alive, signins);
    }
    for pid in alive.iter().chain(signins.iter()) {
        // SAFETY: as above.
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
}

/// The broker's requests: start a signed-in browser, or stop one.
fn answer(
    line: &mut std::os::unix::net::UnixStream,
    request: &Value,
    signins: &mut Vec<libc::pid_t>,
) -> Result<(), String> {
    match request["op"].as_str() {
        Some("signin.spawn") => match spawn_signin(&request["args"]) {
            Ok((pid, write, read)) => {
                signins.push(pid);
                let frame = wire::encode(&json!({"ok": {"pid": pid}}))?;
                wire::send_fds(
                    line.as_raw_fd(),
                    &frame,
                    &[write.as_raw_fd(), read.as_raw_fd()],
                )
            }
            Err(error) => wire::write_blocking(line, &json!({"err": error})),
        },
        Some("signin.stop") => {
            let pid = request["args"]["pid"].as_i64().unwrap_or_default() as libc::pid_t;
            if signins.contains(&pid) {
                // SAFETY: kill with a PID this process started for the broker.
                unsafe { libc::kill(pid, libc::SIGTERM) };
                wire::write_blocking(line, &json!({"ok": null}))
            } else {
                wire::write_blocking(
                    line,
                    &json!({"err": format!("{pid} is not a signed-in browser")}),
                )
            }
        }
        other => wire::write_blocking(
            line,
            &json!({"err": format!("PID 1 does not know {other:?}")}),
        ),
    }
}

/// Chromium as `signin`, with DevTools on a pipe (its fds 3 and 4) and no
/// port. Answers the process and the broker's two ends: where to write
/// commands, and where to read what comes back.
fn spawn_signin(arguments: &Value) -> Result<(libc::pid_t, OwnedFd, OwnedFd), String> {
    let arguments: Vec<String> = serde_json::from_value(arguments.clone())
        .map_err(|_| "signin.spawn takes a list of arguments".to_owned())?;
    let mut argv = vec![cstring(CHROMIUM)?];
    for argument in &arguments {
        argv.push(cstring(argument)?);
    }
    let pipe = || -> Result<(OwnedFd, OwnedFd), String> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: pipe2 writes two new descriptors into the array.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(format!("pipe: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: both were just made and are owned here alone.
        Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
    };
    let (commands_read, commands_write) = pipe()?;
    let (events_read, events_write) = pipe()?;
    let envp = vec![
        cstring("PATH=/usr/local/bin:/usr/bin:/bin")?,
        cstring(&format!("HOME={SIGNIN_DIR}"))?,
        cstring("LANG=C.UTF-8")?,
        cstring(&format!(
            "TZ={}",
            std::env::var("TZ").unwrap_or_else(|_| "UTC".to_owned())
        ))?,
    ];
    let pid = spawn(&Start {
        uid: SIGNIN_UID,
        groups: vec![],
        cwd: cstring(SIGNIN_DIR)?,
        program: cstring(CHROMIUM)?,
        argv,
        envp,
        fds: vec![
            (3, commands_read.as_raw_fd()),
            (4, events_write.as_raw_fd()),
        ],
        quiet: true,
    })?;
    drop((commands_read, events_write));
    Ok((pid, commands_write, events_read))
}
