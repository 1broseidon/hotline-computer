//! The computer's internal lines: inherited socketpairs carrying frames of a
//! u32 little-endian length and then that many bytes of JSON. Nothing is
//! listening, so no job can open one; a job cannot reach the ends either,
//! since every end is close-on-exec once its process has started.
//!
//! A request is `{"id": N, "op": "...", "args": {...}}`; its answer is
//! `{"re": N, "ok": ...}` or `{"re": N, "err": "..."}`. Either side may ask.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// A tool call carries a file of up to 50 MB in base64, and an answer a
/// screenshot; nothing on these lines is larger.
pub const MAX_FRAME: usize = 96 << 20;

pub fn encode(message: &Value) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if body.len() > MAX_FRAME {
        return Err(format!("a frame of {} bytes is too large", body.len()));
    }
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn decode(body: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(body).map_err(|error| format!("a frame is not JSON: {error}"))
}

/// One frame from a blocking stream; `None` when the other end has gone.
pub fn read_blocking(stream: &mut impl Read) -> Result<Option<Value>, String> {
    let mut length = [0u8; 4];
    match stream.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(format!("a frame of {length} bytes is too large"));
    }
    let mut body = vec![0u8; length];
    stream
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    decode(&body).map(Some)
}

pub fn write_blocking(stream: &mut impl Write, message: &Value) -> Result<(), String> {
    stream
        .write_all(&encode(message)?)
        .map_err(|error| error.to_string())
}

/// Marks a descriptor close-on-exec, so nothing this process starts holds it.
pub fn cloexec(fd: RawFd) -> Result<(), String> {
    // SAFETY: fcntl on a descriptor number with integer arguments.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(format!(
            "descriptor {fd}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Takes ownership of an inherited descriptor, refusing one that is not open.
pub fn inherited(fd: RawFd) -> Result<OwnedFd, String> {
    cloexec(fd)?;
    // SAFETY: the descriptor is open (fcntl just answered for it), and
    // nothing else in this process owns it: it was inherited for this alone.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// A connected pair of stream sockets, both ends close-on-exec.
pub fn pair() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: socketpair writes two descriptors into the array it is given.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(format!("socketpair: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: both descriptors were just made and belong to nobody else.
    let pair = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    cloexec(fds[0])?;
    cloexec(fds[1])?;
    Ok(pair)
}

/// Sends `bytes` with descriptors attached (`SCM_RIGHTS`).
pub fn send_fds(socket: RawFd, bytes: &[u8], fds: &[RawFd]) -> Result<(), String> {
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as u32) } as usize;
    let mut control = vec![0u8; space.max(1)];
    // SAFETY: a zeroed msghdr is valid; every pointer set below outlives the
    // sendmsg call, and the control buffer is CMSG_SPACE bytes for `fds`.
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if !fds.is_empty() {
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = space as _;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as u32) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr(),
                libc::CMSG_DATA(header).cast::<RawFd>(),
                fds.len(),
            );
        }
        loop {
            let sent = libc::sendmsg(socket, &message, SEND_FLAGS);
            if sent >= 0 {
                if sent as usize != bytes.len() {
                    return Err("sendmsg sent part of a message".to_owned());
                }
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("sendmsg: {error}"));
            }
        }
    }
}

/// Receives up to `buffer.len()` bytes and any descriptors that came with
/// them. Zero bytes and no descriptors is the other end gone.
pub fn recv_fds(socket: RawFd, buffer: &mut [u8]) -> Result<(usize, Vec<OwnedFd>), String> {
    const MAX: usize = 8;
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let space = unsafe { libc::CMSG_SPACE((MAX * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: as in `send_fds`; descriptors the kernel hands over are new
    // and owned here alone (MSG_CMSG_CLOEXEC marks them close-on-exec).
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = space as _;
        let received = loop {
            let received = libc::recvmsg(socket, &mut message, RECEIVE_FLAGS);
            if received >= 0 {
                break received as usize;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("recvmsg: {error}"));
            }
        };
        let mut fds = Vec::new();
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                let count = ((*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize)
                    / std::mem::size_of::<RawFd>();
                for index in 0..count {
                    let fd = std::ptr::read_unaligned(data.add(index));
                    fds.push(OwnedFd::from_raw_fd(fd));
                    cloexec(fd)?;
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
        Ok((received, fds))
    }
}

#[cfg(target_os = "linux")]
const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
#[cfg(not(target_os = "linux"))]
const SEND_FLAGS: libc::c_int = 0;
#[cfg(target_os = "linux")]
const RECEIVE_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;
#[cfg(not(target_os = "linux"))]
const RECEIVE_FLAGS: libc::c_int = 0;

type Answer = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
/// What a side does with a request from the other.
pub type Handler = Arc<dyn Fn(String, Value) -> Answer + Send + Sync>;

/// One end of a line that both sides may ask on.
#[derive(Clone)]
pub struct Peer {
    out: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next: Arc<AtomicU64>,
}

impl Peer {
    /// Starts reading and writing `stream`. The returned future ends when
    /// the other side has gone, which for these lines means the computer is
    /// over.
    pub fn start(
        stream: OwnedFd,
        handler: Handler,
    ) -> Result<(Peer, impl Future<Output = ()> + Send + 'static), String> {
        let stream = std::os::unix::net::UnixStream::from(stream);
        stream
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let stream = tokio::net::UnixStream::from_std(stream).map_err(|error| error.to_string())?;
        let (mut reader, mut writer) = stream.into_split();
        let (out, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        let peer = Peer {
            out,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next: Arc::new(AtomicU64::new(1)),
        };
        let answering = peer.clone();
        let pending = Arc::clone(&peer.pending);
        let done = async move {
            let writing = async move {
                while let Some(frame) = outgoing.recv().await {
                    if writer.write_all(&frame).await.is_err() {
                        return;
                    }
                }
            };
            let reading = async move {
                loop {
                    let mut length = [0u8; 4];
                    if reader.read_exact(&mut length).await.is_err() {
                        return;
                    }
                    let length = u32::from_le_bytes(length) as usize;
                    if length > MAX_FRAME {
                        eprintln!("hotline-computer: a frame of {length} bytes is too large");
                        return;
                    }
                    let mut body = vec![0u8; length];
                    if reader.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let Ok(message) = decode(&body) else {
                        eprintln!("hotline-computer: a frame that is not JSON");
                        return;
                    };
                    answering.take(message, &handler);
                }
            };
            tokio::select! {
                () = writing => {}
                () = reading => {}
            }
            // Whoever still waits learns the line is gone.
            peer_gone(&pending);
        };
        Ok((peer, done))
    }

    fn take(&self, message: Value, handler: &Handler) {
        if let Some(id) = message["re"].as_u64() {
            let waiting = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            if let Some(waiting) = waiting {
                let answer = match message.get("err") {
                    Some(error) => Err(error.as_str().unwrap_or("failed").to_owned()),
                    None => Ok(message.get("ok").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiting.send(answer);
            }
            return;
        }
        let (Some(id), Some(op)) = (message["id"].as_u64(), message["op"].as_str()) else {
            return;
        };
        let answer = handler(
            op.to_owned(),
            message.get("args").cloned().unwrap_or(Value::Null),
        );
        let out = self.out.clone();
        tokio::spawn(async move {
            let reply = match answer.await {
                Ok(ok) => json!({"re": id, "ok": ok}),
                Err(error) => json!({"re": id, "err": error}),
            };
            let frame = encode(&reply).unwrap_or_else(|error| {
                encode(&json!({"re": id, "err": error})).expect("a short frame encodes")
            });
            let _ = out.send(frame);
        });
    }

    /// Asks the other side and waits for its answer.
    pub async fn call(&self, op: &str, args: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let frame = encode(&json!({"id": id, "op": op, "args": args}))?;
        let (answer, waiting) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, answer);
        if self.out.send(frame).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            return Err(GONE.to_owned());
        }
        waiting.await.unwrap_or_else(|_| Err(GONE.to_owned()))
    }

    /// Tells the other side something without waiting for an answer.
    pub fn tell(&self, op: &str, args: Value) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        if let Ok(frame) = encode(&json!({"id": id, "op": op, "args": args})) {
            let _ = self.out.send(frame);
        }
    }
}

const GONE: &str = "the computer's other half has gone; it is restarting";

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

fn peer_gone(pending: &Pending) {
    for (_, waiting) in pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain()
    {
        let _ = waiting.send(Err(GONE.to_owned()));
    }
}

/// The raw descriptor of an owned one, for the calls above.
pub fn raw(fd: &OwnedFd) -> RawFd {
    fd.as_raw_fd()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn both_sides_ask_and_answer_over_one_line() {
        let (left, right) = pair().unwrap();
        let echo: Handler = Arc::new(|op, args| {
            Box::pin(async move {
                if op == "fail" {
                    Err("refused".to_owned())
                } else {
                    Ok(json!({"op": op, "args": args}))
                }
            })
        });
        let (a, a_done) = Peer::start(left, Arc::clone(&echo)).unwrap();
        let (b, b_done) = Peer::start(right, echo).unwrap();
        tokio::spawn(a_done);
        let b_task = tokio::spawn(b_done);
        assert_eq!(
            a.call("hello", json!({"n": 1})).await.unwrap(),
            json!({"op": "hello", "args": {"n": 1}})
        );
        assert_eq!(b.call("fail", Value::Null).await.unwrap_err(), "refused");
        let big = "x".repeat(3 << 20);
        assert_eq!(
            b.call("big", json!(big)).await.unwrap()["args"]
                .as_str()
                .unwrap()
                .len(),
            3 << 20
        );
        drop(a);
        // The other side going ends the line for whoever waits on it.
        b_task.abort();
    }

    #[test]
    fn descriptors_cross_a_line() {
        let (left, right) = pair().unwrap();
        let (mine, theirs) = pair().unwrap();
        send_fds(raw(&left), b"x", &[raw(&theirs)]).unwrap();
        drop(theirs);
        let mut buffer = [0u8; 4];
        let (count, mut fds) = recv_fds(raw(&right), &mut buffer).unwrap();
        assert_eq!((count, fds.len()), (1, 1));
        let mut received = std::os::unix::net::UnixStream::from(fds.remove(0));
        let mut mine = std::os::unix::net::UnixStream::from(mine);
        mine.write_all(b"ping").unwrap();
        let mut got = [0u8; 4];
        received.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"ping");
    }

    #[test]
    fn a_frame_is_a_length_and_json() {
        let frame = encode(&json!({"a": 1})).unwrap();
        assert_eq!(&frame[..4], &7u32.to_le_bytes());
        let mut reader = &frame[..];
        assert_eq!(read_blocking(&mut reader).unwrap(), Some(json!({"a": 1})));
        assert_eq!(read_blocking(&mut reader).unwrap(), None);
    }
}
