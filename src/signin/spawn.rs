//! Who starts the signed-in browser, and where it keeps its things.
//!
//! In the split computer PID 1 starts it as uid 1002 `signin` when the broker
//! asks (`signin.spawn` on fd 5) and hands back the two ends of its DevTools
//! pipe: `super::split::Pid1`.
//!
//! `Direct` is the other: Chromium started by this process, as this user.
//! It is what the tests run, and what `HOTLINE_COMPUTER_SIGNIN=direct` runs
//! on a single-user computer to try the signed-in browser out. It protects
//! nothing by itself: only the split computer's users do.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::Mutex;

/// A started browser: its process and the broker's ends of its pipe.
pub struct Spawned {
    pub pid: u32,
    /// Where commands go (the browser's fd 3).
    pub write: OwnedFd,
    /// Where answers and events come from (the browser's fd 4).
    pub read: OwnedFd,
}

/// The socket the signed-in browser's native host reaches the broker on, and
/// the one user whose connections are answered there.
#[derive(Clone, Debug)]
pub struct Bridge {
    pub socket: PathBuf,
    pub mode: u32,
    pub peer: u32,
}

pub trait Spawner: Send + Sync {
    fn spawn(&self, arguments: &[String]) -> Result<Spawned, String>;
    fn stop(&self, pid: u32);
    /// The browser's profile directory.
    fn profile(&self) -> PathBuf;
    /// Where the broker keeps the sites revoked since the browser last
    /// closed, beside the profile's lifetime and in the broker's own hands.
    fn revocations(&self) -> PathBuf;
    fn bridge(&self) -> Bridge;
}

/// Chromium as this process's own user, from `PATH`.
pub struct Direct {
    root: PathBuf,
    children: Mutex<HashMap<u32, std::process::Child>>,
}

impl Direct {
    /// Keeps the profile and the bridge socket under `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            children: Mutex::default(),
        }
    }
}

fn pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe writes two new descriptors into the array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(format!("pipe: {}", std::io::Error::last_os_error()));
    }
    // Close-on-exec, so no other child of this process inherits an end; the
    // browser gets its own copies at 3 and 4, which dup2 leaves open.
    for fd in fds {
        // SAFETY: fcntl on a descriptor this function just made.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: both were just made and are owned here alone.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

impl Spawner for Direct {
    fn spawn(&self, arguments: &[String]) -> Result<Spawned, String> {
        std::fs::create_dir_all(self.profile()).map_err(|error| format!("profile: {error}"))?;
        let (commands_read, commands_write) = pipe()?;
        let (events_read, events_write) = pipe()?;
        let (inbound, outbound) = (commands_read.as_raw_fd(), events_write.as_raw_fd());
        let mut command = std::process::Command::new("chromium");
        command
            .args(arguments)
            // The native host Chromium starts for the extension inherits
            // this, and finds the broker by it.
            .env("HOTLINE_COMPUTER_BRIDGE", self.bridge().socket)
            .env_remove("HOTLINE_COMPUTER_TOKEN")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // SAFETY: only dup2 between fork and exec, which is async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                // Either end may itself be 3 or 4: move both clear of them
                // first, so placing one never overwrites the other.
                let inbound = libc::fcntl(inbound, libc::F_DUPFD, 10);
                let outbound = libc::fcntl(outbound, libc::F_DUPFD, 10);
                if inbound < 0
                    || outbound < 0
                    || libc::dup2(inbound, 3) < 0
                    || libc::dup2(outbound, 4) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(inbound);
                libc::close(outbound);
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|error| format!("chromium: {error}"))?;
        drop((commands_read, events_write));
        let pid = child.id();
        self.children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(pid, child);
        Ok(Spawned {
            pid,
            write: commands_write,
            read: events_read,
        })
    }

    fn stop(&self, pid: u32) {
        let child = self
            .children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&pid);
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn profile(&self) -> PathBuf {
        self.root.join("profile")
    }

    fn revocations(&self) -> PathBuf {
        self.root.join("revoked.json")
    }

    fn bridge(&self) -> Bridge {
        Bridge {
            socket: self.root.join("bridge.sock"),
            mode: 0o600,
            // SAFETY: getuid takes nothing and cannot fail.
            peer: unsafe { libc::getuid() },
        }
    }
}
