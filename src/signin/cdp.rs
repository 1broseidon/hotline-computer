//! DevTools over a pipe: the signed-in browser's only control line.
//!
//! Chromium started with `--remote-debugging-pipe` reads commands on its fd 3
//! and writes answers and events on its fd 4, each a JSON message ended by a
//! NUL. No port is opened, so nothing else in the computer can reach it. One
//! thread reads, one writes; a call waits for the answer with its id, and
//! every event goes to whoever subscribed.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

/// No DevTools call waits longer than this.
const CALL_DEADLINE: Duration = Duration::from_secs(30);
/// The largest message read from the browser; a screenshot of a big page is
/// several megabytes of base64.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// An event the browser sent: its method, its parameters, and the session
/// (the attached tab) it came from, if any.
#[derive(Clone, Debug)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

#[derive(Clone)]
pub struct Cdp {
    outgoing: std::sync::mpsc::Sender<Vec<u8>>,
    pending: Pending,
    events: broadcast::Sender<Event>,
    next: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl Cdp {
    /// Speaks DevTools over `write` (the browser's fd 3) and `read` (its fd 4).
    pub fn over(write: OwnedFd, read: OwnedFd) -> Self {
        let (outgoing, queue) = std::sync::mpsc::channel::<Vec<u8>>();
        let pending: Pending = Arc::default();
        let (events, _) = broadcast::channel(1024);
        let closed = Arc::new(AtomicBool::new(false));
        let mut writer = std::fs::File::from(write);
        std::thread::Builder::new()
            .name("signin-cdp-write".into())
            .spawn(move || {
                for message in queue {
                    if writer.write_all(&message).is_err() {
                        return;
                    }
                }
            })
            .expect("a thread starts");
        let reading = (pending.clone(), events.clone(), closed.clone());
        let mut reader = std::fs::File::from(read);
        std::thread::Builder::new()
            .name("signin-cdp-read".into())
            .spawn(move || {
                let (pending, events, closed) = reading;
                let mut buffer: Vec<u8> = Vec::new();
                let mut chunk = vec![0_u8; 1 << 16];
                loop {
                    let count = match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => count,
                    };
                    buffer.extend_from_slice(&chunk[..count]);
                    while let Some(end) = buffer.iter().position(|byte| *byte == 0) {
                        let message: Vec<u8> = buffer.drain(..=end).collect();
                        dispatch(&message[..end], &pending, &events);
                    }
                    if buffer.len() > MAX_MESSAGE {
                        break;
                    }
                }
                closed.store(true, Ordering::SeqCst);
                let waiting: Vec<_> = pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .drain()
                    .collect();
                for (_, waiter) in waiting {
                    let _ = waiter.send(Err("the signed-in browser exited".into()));
                }
            })
            .expect("a thread starts");
        Self {
            outgoing,
            pending,
            events,
            next: Arc::new(AtomicU64::new(1)),
            closed,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// A browser-wide command.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.send(method, params, None).await
    }

    /// A command to one attached tab.
    pub async fn on(&self, session: &str, method: &str, params: Value) -> Result<Value, String> {
        self.send(method, params, Some(session)).await
    }

    async fn send(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        if self.is_closed() {
            return Err("the signed-in browser exited".into());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (answer, answered) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, answer);
        let mut bytes = message.to_string().into_bytes();
        bytes.push(0);
        if self.outgoing.send(bytes).is_err() {
            self.forget(id);
            return Err("the signed-in browser exited".into());
        }
        match tokio::time::timeout(CALL_DEADLINE, answered).await {
            Ok(Ok(answer)) => answer.map_err(|error| format!("{method}: {error}")),
            Ok(Err(_)) => Err("the signed-in browser exited".into()),
            Err(_) => {
                self.forget(id);
                Err(format!("{method}: the signed-in browser did not answer"))
            }
        }
    }

    fn forget(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
    }
}

fn dispatch(raw: &[u8], pending: &Pending, events: &broadcast::Sender<Event>) {
    let Ok(message) = serde_json::from_slice::<Value>(raw) else {
        return;
    };
    if let Some(id) = message["id"].as_u64() {
        let waiter = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        if let Some(waiter) = waiter {
            let answer = match message.get("error") {
                Some(error) => Err(error["message"]
                    .as_str()
                    .unwrap_or("the browser refused")
                    .to_owned()),
                None => Ok(message["result"].clone()),
            };
            let _ = waiter.send(answer);
        }
        return;
    }
    if let Some(method) = message["method"].as_str() {
        let _ = events.send(Event {
            method: method.to_owned(),
            params: message["params"].clone(),
            session: message["sessionId"].as_str().map(str::to_owned),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    fn pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [0; 2];
        // SAFETY: pipe writes two new descriptors into the array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both were just made and are owned here alone.
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    /// A fake browser on the other end: it answers each call with its own
    /// method name and sends one event first.
    #[tokio::test]
    async fn calls_are_answered_by_id_and_events_are_broadcast() {
        let (commands_read, commands_write) = pipe();
        let (events_read, events_write) = pipe();
        let cdp = Cdp::over(commands_write, events_read);
        let mut events = cdp.events();
        std::thread::spawn(move || {
            let mut input = std::fs::File::from(commands_read);
            let mut output = std::fs::File::from(events_write);
            let mut buffer = Vec::new();
            let mut byte = [0_u8; 1];
            while input.read(&mut byte).unwrap_or(0) == 1 {
                if byte[0] != 0 {
                    buffer.push(byte[0]);
                    continue;
                }
                let call: Value = serde_json::from_slice(&buffer).unwrap();
                buffer.clear();
                let event =
                    json!({"method": "Page.loadEventFired", "params": {}, "sessionId": "s1"});
                let answer = json!({"id": call["id"], "result": {"echo": call["method"]}});
                for message in [event, answer] {
                    output.write_all(message.to_string().as_bytes()).unwrap();
                    output.write_all(&[0]).unwrap();
                }
            }
        });
        let answer = cdp.call("Browser.getVersion", json!({})).await.unwrap();
        assert_eq!(answer["echo"], "Browser.getVersion");
        let event = events.recv().await.unwrap();
        assert_eq!(event.method, "Page.loadEventFired");
        assert_eq!(event.session.as_deref(), Some("s1"));
        drop(cdp);
    }
}
