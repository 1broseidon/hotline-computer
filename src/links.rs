//! Links an app opens. `xdg-open https://…` runs `hotline-computer open`,
//! which hands the address to the running agent here; the agent opens it as a
//! tab of the managed browser, where the person's sign-ins live. The system's
//! own Chromium handler would start a second, unsandboxable browser that dies.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

use crate::App;

pub const SOCKET: &str = "/tmp/hotline-computer/links.sock";
const LONGEST: u64 = 64 * 1024;

pub fn is_link(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

/// Ask the running agent to open `url`; its error, if any, comes back.
pub fn send(url: &str) -> Result<(), String> {
    let mut stream = std::os::unix::net::UnixStream::connect(SOCKET)
        .map_err(|error| format!("the computer is not taking links: {error}"))?;
    stream
        .write_all(format!("{url}\n").as_bytes())
        .map_err(|error| error.to_string())?;
    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    match answer.trim_end() {
        "ok" => Ok(()),
        error => Err(error.to_owned()),
    }
}

pub async fn serve(app: App) -> Result<(), String> {
    let socket = Path::new(SOCKET);
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove stale link socket: {error}")),
    }
    let listener = tokio::net::UnixListener::bind(socket)
        .map_err(|error| format!("bind link socket: {error}"))?;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|error| format!("link socket: {error}"))?;
        let app = app.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut url = String::new();
            let answer = match tokio::io::BufReader::new(reader.take(LONGEST))
                .read_line(&mut url)
                .await
            {
                Ok(_) => open(&app, url.trim()).await,
                Err(error) => Err(error.to_string()),
            };
            let line = match answer {
                Ok(()) => "ok\n".to_owned(),
                Err(error) => format!("{}\n", error.replace('\n', " ")),
            };
            let _ = writer.write_all(line.as_bytes()).await;
        });
    }
}

async fn open(app: &App, url: &str) -> Result<(), String> {
    if !is_link(url) {
        return Err(format!("not a web address: {url}"));
    }
    let opened = app.browser.tab_new(url).await;
    let display = app.config.display.clone();
    let _ = tokio::task::spawn_blocking(move || raise_browser(&display)).await;
    opened.map(|_| ())
}

fn raise_browser(display: &str) -> Result<(), String> {
    let window = crate::x11::windows(display)?
        .into_iter()
        .find(|window| window.class.contains(".hotline/browser"))
        .ok_or("the browser has no window")?;
    crate::x11::activate(display, &window.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_addresses_are_links() {
        assert!(is_link("https://accounts.x.ai/oauth2/device?user_code=A"));
        assert!(is_link("http://localhost:5173/"));
        assert!(!is_link("file:///etc/passwd"));
        assert!(!is_link("/home/agent/src"));
        assert!(!is_link("javascript:alert(1)"));
    }
}
