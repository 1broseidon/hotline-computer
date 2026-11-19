//! A file the signed-in browser downloads, handed to the agent. In the split
//! computer the browser saves it, under DevTools' guid, in a staging folder
//! only it and the broker reach; the broker reads it there, hands it to the
//! runner (`download.write`) for the agent's Downloads, and removes it.

use std::path::PathBuf;

use super::{DOWNLOAD_CAP, Signin};

impl Signin {
    pub(super) async fn hand_over(
        &self,
        staged: PathBuf,
        name: String,
        frame: String,
        completed: bool,
    ) {
        let note = if !completed {
            format!("the download of {name} did not finish")
        } else {
            match tokio::fs::metadata(&staged).await {
                Ok(metadata) if metadata.len() > DOWNLOAD_CAP => {
                    format!("{name} was not kept: it is larger than 50 MB")
                }
                Ok(_) => match tokio::fs::read(&staged).await {
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
