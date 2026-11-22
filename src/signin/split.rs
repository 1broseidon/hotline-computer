//! The signed-in browser in the split computer: PID 1 starts it as uid 1002
//! `signin`, its bridge answers `signin` alone, plain links go to the
//! runner's general browser, and an upload is read from the agent's home by
//! the runner and staged where `signin` may read it.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::json;

use super::spawn::{Bridge, Spawned, Spawner};
use super::{General, Signin, Staged};
use crate::App;
use crate::split::{self, Runner};

/// Where uploads wait for the signed-in browser: in the bridge's directory,
/// `hotline:signin` and setgid, so a file the broker writes there 0640 is
/// the broker's and `signin`'s to read and no job's.
const UPLOADS: &str = "/run/hotline-computer/bridge/uploads";
/// Where the signed-in browser saves downloads for the broker to hand on:
/// the same directory, the browser's group writing.
const DOWNLOADS: &str = "/run/hotline-computer/bridge/downloads";

pub struct Pid1;

impl Spawner for Pid1 {
    fn spawn(&self, arguments: &[String]) -> Result<Spawned, String> {
        let (pid, write, read) = split::broker::signin_spawn(arguments)?;
        Ok(Spawned { pid, write, read })
    }

    fn stop(&self, pid: u32) {
        if let Err(error) = split::broker::signin_stop(pid) {
            eprintln!("signin: the browser did not stop: {error}");
        }
    }

    fn profile(&self) -> PathBuf {
        PathBuf::from(split::SIGNIN_PROFILE)
    }

    fn revocations(&self) -> PathBuf {
        PathBuf::from(split::SIGNIN_REVOKED)
    }

    fn bridge(&self) -> Bridge {
        Bridge {
            socket: PathBuf::from(split::BRIDGE_SOCKET),
            mode: 0o660,
            peer: split::SIGNIN_UID,
        }
    }
}

/// The runner, for what the signed-in browser needs of the agent's side.
pub struct ByRunner(pub Runner);

impl General for ByRunner {
    fn open(&self, url: String) {
        let runner = self.0.clone();
        tokio::spawn(async move {
            if let Err(error) = runner.call("links.open", json!({"url": url})).await {
                eprintln!("signin: the general browser did not open a link: {error}");
            }
        });
    }

    fn stage(&self, path: &Path) -> Staged {
        let runner = self.0.clone();
        let path = path.to_string_lossy().into_owned();
        Box::pin(async move {
            use std::os::unix::fs::PermissionsExt;
            let read = runner.call("upload.read", json!({"path": path})).await?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(read["content"].as_str().unwrap_or_default())
                .map_err(|error| format!("upload: {error}"))?;
            let name = read["name"].as_str().unwrap_or("upload");
            let name = Path::new(name).file_name().map_or_else(
                || "upload".into(),
                |name| name.to_string_lossy().into_owned(),
            );
            let directory = Path::new(UPLOADS).join(super::flow_id());
            tokio::fs::create_dir_all(&directory)
                .await
                .map_err(|error| format!("upload: {error}"))?;
            let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o750));
            let staged = directory.join(name);
            tokio::fs::write(&staged, bytes)
                .await
                .map_err(|error| format!("upload: {error}"))?;
            let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o640));
            Ok(staged)
        })
    }

    fn deliver(&self, name: String, bytes: Vec<u8>) -> super::Delivered {
        let runner = self.0.clone();
        Box::pin(async move {
            let path = format!("{}/Downloads/{name}", split::AGENT_HOME);
            let content = base64::engine::general_purpose::STANDARD.encode(bytes);
            runner
                .call("download.write", json!({"path": path, "content": content}))
                .await?;
            Ok(path)
        })
    }
}

/// The broker's signed-in browser.
pub fn for_broker(app: &App) -> Signin {
    let runner = app
        .runner
        .clone()
        .expect("the broker has its runner before its signed-in browser");
    // Created here by the broker, which owns the bridge's directory: it
    // takes that directory's group, `signin`, whose browser writes into it.
    let downloads = Path::new(DOWNLOADS);
    let made = std::fs::create_dir_all(downloads).and_then(|()| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(downloads, std::fs::Permissions::from_mode(0o770))
    });
    if let Err(error) = &made {
        eprintln!("signin: {DOWNLOADS}: {error}");
    }
    let signin = Signin::new(
        Box::new(Pid1),
        Box::new(ByRunner(runner)),
        app.secrets.clone(),
        made.is_ok()
            .then(|| super::Downloads::Staged(downloads.to_owned())),
    );
    signin.arming_from(app.passkeys.clone());
    signin
}
