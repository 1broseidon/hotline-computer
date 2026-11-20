//! A file the signed-in browser downloads, handed to the agent. In the split
//! computer the browser saves it, under DevTools' guid, in a staging folder
//! only it and the broker reach; the broker reads it there, hands it to the
//! runner (`download.write`) for the agent's Downloads, and removes it.
//! A download that began on a page hidden from the agent (mid sign-in, or
//! holding what was typed), or from a frame that is not a known tab, or a
//! file that carries a secret's value in any spelling, is not handed over.

use std::path::PathBuf;

use super::{DOWNLOAD_CAP, Signin};

impl Signin {
    pub(super) async fn hand_over(
        &self,
        guid: String,
        staged: PathBuf,
        name: String,
        frame: String,
        completed: bool,
    ) {
        let mut shown = None;
        for _ in 0..50 {
            shown = self
                .0
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&guid);
            if shown.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let note = if !completed {
            format!("the download of {name} did not finish")
        } else if shown != Some(true) {
            format!(
                "{name} was not handed to you: it began on a page that was hidden from you (signing in, or holding what was typed there)"
            )
        } else {
            match tokio::fs::metadata(&staged).await {
                Ok(metadata) if metadata.len() > DOWNLOAD_CAP => {
                    format!("{name} was not kept: it is larger than 50 MB")
                }
                Ok(_) => match tokio::fs::read(&staged).await {
                    Ok(bytes)
                        if std::str::from_utf8(&bytes)
                            .is_ok_and(|text| self.0.secrets.redact(text) != text) =>
                    {
                        format!("{name} was not handed to you: it carries a secret's value")
                    }
                    Ok(bytes) => match self.0.general.deliver(name.clone(), bytes).await {
                        Ok(path) => format!("downloaded {name} to {path}"),
                        Err(error) => format!("the download of {name} was not kept: {error}"),
                    },
                    Err(error) => format!("the download of {name} could not be read: {error}"),
                },
                Err(error) => format!("the download of {name} could not be read: {error}"),
            }
        };
        let _ = tokio::fs::remove_file(&staged).await;
        let mut tabs = self.lock_tabs();
        let current = tabs.current.clone();
        if let Some(tab) = tabs
            .list
            .iter_mut()
            .find(|tab| tab.target == frame || Some(&tab.target) == current.as_ref())
        {
            tab.notes.push(note);
        }
    }
}
