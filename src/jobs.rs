//! Commands belong to the computer, independently of a tool call or observer window.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, unix::AsyncFd};
use tokio::sync::{OnceCell, mpsc, watch};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const MAX_JOBS: usize = 64;
const MAX_RUNNING: usize = 16;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Start {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub label: Option<String>,
    pub timeout: Option<u64>,
    #[serde(default)]
    pub pty: bool,
    pub request_id: Option<String>,
    // Only the artifact tool can request staging; ordinary shell input cannot.
    #[serde(skip)]
    pub artifact_destination: Option<PathBuf>,
    // Preparation must be able to repair a missing or obsolete environment.
    #[serde(skip)]
    pub skip_workspace_environment: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    pub id: String,
    pub pid: Option<u32>,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub holder: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub error: Option<String>,
    pub output_bytes: usize,
    pub truncated: bool,
    pub pty: bool,
    pub request_id: Option<String>,
    // Environment values never enter the persisted metadata or public response.
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_staging: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub label: String,
    pub state: String,
    pub exit_code: Option<i32>,
}

impl Record {
    pub fn finished(&self) -> bool {
        self.state != "running"
    }
}

enum Input {
    Pipe(tokio::process::ChildStdin),
    Pty(Arc<AsyncFd<OwnedFd>>),
}

struct Job {
    record: Mutex<Record>,
    directory: PathBuf,
    input: tokio::sync::Mutex<Option<Input>>,
    cancel: mpsc::Sender<()>,
    changes: watch::Sender<u64>,
}

impl Job {
    fn record(&self) -> Record {
        self.record.lock().unwrap().clone()
    }

    fn save(&self) -> Result<(), String> {
        let record = self.record();
        let temporary = self.directory.join("record.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        )
        .and_then(|()| std::fs::rename(temporary, self.directory.join("record.json")))
        .map_err(|e| format!("save job {}: {e}", record.id))
    }

    fn cleanup_artifact(&self) -> Result<(), String> {
        let record = self.record();
        let Some(staging) = record.artifact_staging else {
            return Ok(());
        };
        let expected = format!(".toad-artifact-{}", record.id);
        let home = self
            .directory
            .ancestors()
            .nth(3)
            .ok_or("job home unavailable")?;
        if staging.file_name().and_then(|s| s.to_str()) != Some(&expected)
            || !staging
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .is_some_and(|p| p.starts_with(home))
        {
            return Err("refusing invalid artifact staging path".into());
        }
        match std::fs::remove_dir_all(&staging) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("clean artifact staging: {error}")),
        }
    }

    fn append(&self, channel: &str, bytes: &[u8]) -> Result<(), String> {
        let mut record = self.record.lock().unwrap();
        let remaining = OUTPUT_LIMIT.saturating_sub(record.output_bytes);
        let count = bytes.len().min(remaining);
        record.truncated |= count < bytes.len();
        if count > 0 {
            for filename in ["output.log".to_owned(), format!("{channel}.log")] {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.directory.join(filename))
                    .and_then(|mut file| file.write_all(&bytes[..count]))
                    .map_err(|e| format!("write job output: {e}"))?;
            }
            record.output_bytes += count;
        }
        self.changes.send_modify(|revision| *revision += 1);
        Ok(())
    }
}

#[derive(Clone)]
pub struct Jobs {
    root: PathBuf,
    home: PathBuf,
    display: String,
    jobs: Arc<Mutex<BTreeMap<String, Arc<Job>>>>,
    initialized: Arc<OnceCell<()>>,
    admission: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Serialize)]
pub struct Output {
    pub job: Record,
    pub output: String,
    pub cursor: u64,
    pub next_cursor: u64,
    pub eof: bool,
}

impl Jobs {
    pub fn new(home: &Path, display: &str) -> Self {
        Self {
            root: home.join(".toad/jobs"),
            home: home.to_owned(),
            display: display.to_owned(),
            jobs: Arc::new(Mutex::new(BTreeMap::new())),
            initialized: Arc::new(OnceCell::new()),
            admission: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn initialize(&self) -> Result<(), String> {
        self.initialized
            .get_or_try_init(|| async {
                std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
                let mut jobs = self.jobs.lock().unwrap();
                for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                        continue;
                    }
                    let path = entry.path();
                    let Ok(bytes) = std::fs::read(path.join("record.json")) else {
                        continue;
                    };
                    let Ok(mut record) = serde_json::from_slice::<Record>(&bytes) else {
                        continue;
                    };
                    if path.file_name().and_then(|n| n.to_str()) != Some(&record.id) {
                        continue;
                    }
                    if !record.finished() {
                        record.state = "interrupted".to_owned();
                        record.finished_at = Some(now());
                        record.error =
                            Some("computer restarted; this job is not resumed".to_owned());
                    }
                    record.output_bytes =
                        std::fs::metadata(path.join("output.log")).map_or(0, |m| m.len() as usize);
                    let (cancel, _) = mpsc::channel(1);
                    let (changes, _) = watch::channel(0);
                    let job = Arc::new(Job {
                        record: Mutex::new(record),
                        directory: path,
                        input: tokio::sync::Mutex::new(None),
                        cancel,
                        changes,
                    });
                    job.cleanup_artifact()?;
                    job.save()?;
                    jobs.insert(job.record().id.clone(), job);
                }
                drop(jobs);
                self.publish();
                Ok::<(), String>(())
            })
            .await
            .map(|_| ())
    }

    pub async fn list(&self) -> Result<Vec<Record>, String> {
        self.initialize().await?;
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .values()
            .rev()
            .map(|job| job.record())
            .collect())
    }

    async fn job(&self, id: &str) -> Result<Arc<Job>, String> {
        self.initialize().await?;
        self.jobs
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("job not found: {id}"))
    }

    pub async fn status(&self, id: &str) -> Result<Record, String> {
        Ok(self.job(id).await?.record())
    }

    pub async fn start(&self, mut start: Start, holder: &str) -> Result<Record, String> {
        self.initialize().await?;
        let _admission = self.admission.lock().await;
        if start.command.is_empty() {
            return Err("command is required".into());
        }
        if start.command.len() + start.args.iter().map(String::len).sum::<usize>() > 131_072 {
            return Err("command and args exceed 128 KiB".into());
        }
        if start
            .env
            .iter()
            .any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0'))
        {
            return Err("invalid environment entry".into());
        }
        let cwd = PathBuf::from(
            start
                .cwd
                .as_deref()
                .unwrap_or(self.home.to_str().unwrap_or("/home/agent")),
        )
        .canonicalize()
        .map_err(|e| format!("working directory: {e}"))?;
        if !cwd.is_dir() {
            return Err("cwd must be a directory".into());
        }
        use sha2::{Digest, Sha256};
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&start).map_err(|e| e.to_string())?)
        );
        {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(request_id) = &start.request_id {
                if request_id.is_empty() || request_id.len() > 128 {
                    return Err("request_id must contain 1–128 bytes".into());
                }
                if let Some(job) = jobs.values().find(|j| {
                    let r = j.record();
                    r.holder == holder && r.request_id.as_ref() == Some(request_id)
                }) {
                    let record = job.record();
                    return if record.fingerprint == fingerprint {
                        Ok(record)
                    } else {
                        Err("request_id already used for a different command".into())
                    };
                }
            }
            if jobs.values().filter(|j| !j.record().finished()).count() >= MAX_RUNNING {
                return Err("16 jobs are already running; wait or cancel one".into());
            }
            while jobs.len() >= MAX_JOBS {
                let id = jobs
                    .iter()
                    .find(|(_, j)| j.record().finished())
                    .map(|(id, _)| id.clone())
                    .ok_or("job history is full")?;
                if let Some(job) = jobs.remove(&id) {
                    std::fs::remove_dir_all(&job.directory).map_err(|e| e.to_string())?;
                }
            }
        }
        let mut environment = if start.skip_workspace_environment {
            BTreeMap::new()
        } else {
            crate::workspace::environment(&self.home, &cwd)?
        };
        environment.append(&mut start.env);
        start.env = environment;
        let id = format!("{:016x}-{:08x}", now(), rand_id()?);
        let directory = self.root.join(&id);
        std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
        let artifact_staging = start
            .artifact_destination
            .as_ref()
            .and_then(|destination| destination.parent())
            .map(|parent| parent.join(format!(".toad-artifact-{id}")));
        if let Some(staging) = &artifact_staging {
            start.env.insert(
                "TOAD_ARTIFACT_DIR".into(),
                staging.to_string_lossy().into_owned(),
            );
        }
        let record = Record {
            id: id.clone(),
            pid: None,
            label: start
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map_or_else(|| describe(&start.command, &start.args), str::to_owned),
            command: start.command.clone(),
            args: start.args.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            holder: holder.to_owned(),
            state: "running".into(),
            exit_code: None,
            signal: None,
            started_at: now(),
            finished_at: None,
            error: None,
            output_bytes: 0,
            truncated: false,
            pty: start.pty,
            request_id: start.request_id.clone(),
            fingerprint,
            artifact_staging,
        };
        let (cancel, receiver) = mpsc::channel(1);
        let (changes, _) = watch::channel(0);
        let job = Arc::new(Job {
            record: Mutex::new(record),
            directory,
            input: tokio::sync::Mutex::new(None),
            cancel,
            changes,
        });
        job.save()?;
        self.jobs.lock().unwrap().insert(id, Arc::clone(&job));
        let spawned = spawn(&start, &cwd, &self.display);
        match spawned {
            Err(error) => {
                let mut record = job.record.lock().unwrap();
                record.state = "failed".into();
                record.error = Some(error);
                record.exit_code = Some(127);
                record.finished_at = Some(now());
                drop(record);
                job.save()?;
            }
            Ok((mut child, input, readers)) => {
                job.record.lock().unwrap().pid = child.id();
                *job.input.lock().await = Some(input);
                if let Err(error) = job.save() {
                    if let Some(pid) = child.id() {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                    let _ = child.wait().await;
                    *job.input.lock().await = None;
                    let mut record = job.record.lock().unwrap();
                    record.state = "failed".into();
                    record.error = Some(error);
                    record.finished_at = Some(now());
                    drop(record);
                    let _ = job.save();
                    return Ok(job.record());
                }
                let running = Arc::clone(&job);
                let manager = self.clone();
                tokio::spawn(async move {
                    supervise(running, child, readers, receiver, start.timeout).await;
                    manager.publish();
                });
            }
        }
        self.publish();
        Ok(job.record())
    }

    fn publish(&self) {
        // Keep snapshots ordered through publication; a finishing job must not
        // overwrite a newer start with an older count on another X11 connection.
        let jobs = self.jobs.lock().unwrap();
        let mut records: Vec<_> = jobs.values().map(|job| job.record()).collect();
        records.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let summaries: Vec<_> = records
            .into_iter()
            .map(|record| Summary {
                id: record.id,
                label: record.label,
                state: record.state,
                exit_code: record.exit_code,
            })
            .collect();
        let _ = crate::x11::publish_jobs(&self.display, &summaries);
        drop(jobs);
    }

    pub async fn wait(&self, id: &str, milliseconds: u64) -> Result<Record, String> {
        let job = self.job(id).await?;
        let mut changes = job.changes.subscribe();
        let future = async {
            loop {
                let record = job.record();
                if record.finished() {
                    return record;
                }
                if changes.changed().await.is_err() {
                    return job.record();
                }
            }
        };
        Ok(
            tokio::time::timeout(Duration::from_millis(milliseconds.min(60_000)), future)
                .await
                .unwrap_or_else(|_| job.record()),
        )
    }

    pub async fn request_cancel(&self, id: &str) -> Result<(), String> {
        let job = self.job(id).await?;
        if !job.record().finished() {
            let _ = job.cancel.try_send(());
        }
        Ok(())
    }

    pub async fn cancel(&self, id: &str) -> Result<Record, String> {
        self.request_cancel(id).await?;
        self.wait(id, 5000).await
    }

    pub async fn write(&self, id: &str, text: &str, eof: bool) -> Result<Record, String> {
        if text.len() > 65_536 {
            return Err("stdin write exceeds 64 KiB".into());
        }
        let job = self.job(id).await?;
        if job.record().finished() {
            return Err("job has already finished".into());
        }
        let mut input = job.input.lock().await;
        let writer = input.as_mut().ok_or("job stdin is closed")?;
        tokio::time::timeout(Duration::from_secs(3), async {
            match writer {
                Input::Pipe(stdin) => {
                    stdin.write_all(text.as_bytes()).await?;
                    if eof {
                        stdin.shutdown().await?;
                    }
                }
                Input::Pty(fd) => {
                    pty_write(fd, text.as_bytes()).await?;
                    if eof {
                        pty_write(fd, &[4]).await?;
                    }
                }
            }
            Ok::<(), std::io::Error>(())
        })
        .await
        .map_err(|_| "stdin write timed out".to_owned())?
        .map_err(|e| e.to_string())?;
        if eof {
            *input = None;
        }
        Ok(job.record())
    }

    pub async fn read(&self, id: &str, cursor: u64, limit: usize) -> Result<Output, String> {
        let job = self.job(id).await?;
        let bytes = read_file(
            &job.directory.join("output.log"),
            cursor,
            limit.clamp(1, 1_048_576),
        )?;
        let next_cursor = cursor + bytes.len() as u64;
        let record = job.record();
        Ok(Output {
            eof: record.finished() && next_cursor >= record.output_bytes as u64,
            job: record,
            output: String::from_utf8_lossy(&bytes).into_owned(),
            cursor,
            next_cursor,
        })
    }

    pub async fn streams(&self, id: &str, limit: usize) -> Result<(String, String), String> {
        let job = self.job(id).await?;
        let limit = limit.clamp(1, 1_048_576);
        Ok((
            String::from_utf8_lossy(&read_file(&job.directory.join("stdout.log"), 0, limit)?)
                .into_owned(),
            String::from_utf8_lossy(&read_file(&job.directory.join("stderr.log"), 0, limit)?)
                .into_owned(),
        ))
    }

    pub async fn shutdown(&self) {
        if let Ok(jobs) = self.list().await {
            for job in &jobs {
                if !job.finished()
                    && let Ok(j) = self.job(&job.id).await
                {
                    let _ = j.cancel.try_send(());
                }
            }
            for job in jobs {
                let _ = self.wait(&job.id, 3000).await;
            }
        }
    }
}

fn read_file(path: &Path, offset: u64, limit: usize) -> Result<Vec<u8>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// The name a job gets when the teammate gives it none: its command line,
/// on one line, cut to a width that fits a menu row.
pub fn describe(command: &str, args: &[String]) -> String {
    const WIDTH: usize = 60;
    let line = std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= WIDTH {
        line
    } else {
        let mut cut: String = line.chars().take(WIDTH - 1).collect();
        cut.push('…');
        cut
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn rand_id() -> Result<u32, String> {
    let mut bytes = [0; 4];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    Ok(u32::from_ne_bytes(bytes))
}

enum Reader {
    Pipe(Box<dyn AsyncRead + Send + Unpin>),
    Pty(Arc<AsyncFd<OwnedFd>>),
}
type Spawned = (tokio::process::Child, Input, Vec<(&'static str, Reader)>);
fn spawn(start: &Start, cwd: &Path, display: &str) -> Result<Spawned, String> {
    let mut command = tokio::process::Command::new(&start.command);
    command
        .args(&start.args)
        .current_dir(cwd)
        .envs(&start.env)
        .env("DISPLAY", display)
        .kill_on_drop(true);
    if start.pty {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: 32,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        for fd in [&master, &slave] {
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        command
            .stdin(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
            .stdout(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(slave));
        // A PTY child owns a new session and controlling terminal; it remains
        // independent of whichever terminal happens to observe the job.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let fd = Arc::new(AsyncFd::new(master).map_err(|e| e.to_string())?);
        let child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", start.command))?;
        Ok((
            child,
            Input::Pty(Arc::clone(&fd)),
            vec![("stdout", Reader::Pty(fd))],
        ))
    } else {
        command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", start.command))?;
        let input = Input::Pipe(child.stdin.take().ok_or("stdin pipe unavailable")?);
        let stdout = child.stdout.take().ok_or("stdout pipe unavailable")?;
        let stderr = child.stderr.take().ok_or("stderr pipe unavailable")?;
        Ok((
            child,
            input,
            vec![
                ("stdout", Reader::Pipe(Box::new(stdout))),
                ("stderr", Reader::Pipe(Box::new(stderr))),
            ],
        ))
    }
}

async fn pty_write(fd: &AsyncFd<OwnedFd>, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let mut ready = fd.writable().await?;
        match ready.try_io(|inner| {
            let n = unsafe { libc::write(inner.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }) {
            Ok(Ok(0)) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(Ok(n)) => bytes = &bytes[n..],
            Ok(Err(e)) => return Err(e),
            Err(_) => {}
        }
    }
    Ok(())
}

async fn drain(job: Arc<Job>, channel: &str, mut reader: Reader) -> Result<(), String> {
    let mut bytes = [0; 8192];
    loop {
        let count = match &mut reader {
            Reader::Pipe(pipe) => pipe.read(&mut bytes).await.map_err(|e| e.to_string())?,
            Reader::Pty(fd) => loop {
                let mut ready = fd.readable().await.map_err(|e| e.to_string())?;
                match ready.try_io(|inner| {
                    let n = unsafe {
                        libc::read(inner.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len())
                    };
                    if n < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(n as usize)
                    }
                }) {
                    Ok(Ok(n)) => break n,
                    Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => break 0,
                    Ok(Err(e)) => return Err(e.to_string()),
                    Err(_) => {}
                }
            },
        };
        if count == 0 {
            return Ok(());
        }
        job.append(channel, &bytes[..count])?;
    }
}

async fn supervise(
    job: Arc<Job>,
    mut child: tokio::process::Child,
    readers: Vec<(&'static str, Reader)>,
    mut cancel: mpsc::Receiver<()>,
    timeout: Option<u64>,
) {
    let (failed_output, mut output_failures) = mpsc::channel(2);
    let readers: Vec<_> = readers
        .into_iter()
        .map(|(channel, reader)| {
            let job = Arc::clone(&job);
            let failed_output = failed_output.clone();
            tokio::spawn(async move {
                let result = drain(job, channel, reader).await;
                if result.is_err() {
                    let _ = failed_output.send(()).await;
                }
                result
            })
        })
        .collect();
    drop(failed_output);
    let pid = child.id().expect("spawned child has a PID");
    let deadline = async {
        match timeout {
            Some(seconds) => tokio::time::sleep(Duration::from_secs(seconds)).await,
            None => std::future::pending::<()>().await,
        }
    };
    let (state, status) = tokio::select! {
        status=child.wait()=>("exited",status),
        Some(())=output_failures.recv()=>{unsafe {libc::kill(-(pid as i32),libc::SIGKILL);}("failed",child.wait().await)},
        _=cancel.recv()=>{unsafe {libc::kill(-(pid as i32),libc::SIGKILL);}("cancelled",child.wait().await)},
        ()=deadline=>{unsafe {libc::kill(-(pid as i32),libc::SIGKILL);}("timed_out",child.wait().await)},
    };
    // A completed shell must not leave background descendants holding its pipes.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    *job.input.lock().await = None;
    let mut output_error = None;
    for mut reader in readers {
        match tokio::time::timeout(Duration::from_secs(2), &mut reader).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => output_error = Some(e),
            Ok(Err(e)) => output_error = Some(e.to_string()),
            Err(_) => {
                reader.abort();
                output_error = Some("output reader did not close".into());
            }
        }
    }
    if let Err(error) = job.cleanup_artifact() {
        output_error = Some(error);
    }
    {
        let mut record = job.record.lock().unwrap();
        record.state = state.into();
        record.finished_at = Some(now());
        if output_error.is_some() {
            record.state = "failed".into();
        }
        record.error = output_error;
        match status {
            Ok(status) => {
                record.exit_code = status.code();
                record.signal = status.signal();
                if state == "exited" && !status.success() {
                    record.state = "failed".into();
                }
            }
            Err(e) => {
                record.state = "failed".into();
                record.error = Some(e.to_string());
            }
        }
    }
    if let Err(error) = job.save() {
        job.record.lock().unwrap().error = Some(error);
    }
    job.changes.send_modify(|revision| *revision += 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlabelled_job_is_named_by_its_command_line_cut_to_a_row() {
        assert_eq!(describe("cargo", &["test".into()]), "cargo test");
        assert_eq!(
            describe("bash", &["-c".into(), "cc  main.c\n  -o fixture".into()]),
            "bash -c cc main.c -o fixture"
        );
        let long = describe("python3", &["-c".into(), "x".repeat(200)]);
        assert_eq!(long.chars().count(), 60);
        assert!(long.ends_with('…'));
    }

    fn command(script: &str) -> Start {
        Start {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            ..Start::default()
        }
    }

    #[tokio::test]
    async fn deadlines_retain_partial_output_and_cancellation_reaps_the_child() {
        let home = tempfile::tempdir().unwrap();
        let jobs = Jobs::new(home.path(), ":0");
        let mut start = command("printf 'before timeout'; printf 'diagnostic' >&2; sleep 30");
        start.timeout = Some(1);
        let job = jobs.start(start, "test").await.unwrap();
        let result = jobs.wait(&job.id, 5000).await.unwrap();
        assert_eq!(result.state, "timed_out");
        let (out, err) = jobs.streams(&job.id, 65536).await.unwrap();
        assert_eq!(out, "before timeout");
        assert_eq!(err, "diagnostic");
        assert_eq!(unsafe { libc::kill(job.pid.unwrap() as i32, 0) }, -1);
        let second = jobs.start(command("sleep 30"), "test").await.unwrap();
        assert_eq!(jobs.cancel(&second.id).await.unwrap().state, "cancelled");
        assert_eq!(unsafe { libc::kill(second.pid.unwrap() as i32, 0) }, -1);
    }

    #[tokio::test]
    async fn retry_ids_are_scoped_and_spawn_failures_are_queryable() {
        let home = tempfile::tempdir().unwrap();
        let jobs = Jobs::new(home.path(), ":0");
        let mut start = command("printf once");
        start.request_id = Some("retry".into());
        let first = jobs.start(start.clone(), "alice").await.unwrap();
        assert_eq!(
            jobs.start(start.clone(), "alice").await.unwrap().id,
            first.id
        );
        assert_ne!(jobs.start(start.clone(), "bob").await.unwrap().id, first.id);
        start.args = vec!["-c".into(), "printf twice".into()];
        assert!(
            jobs.start(start, "alice")
                .await
                .unwrap_err()
                .contains("different command")
        );
        let failure = jobs
            .start(
                Start {
                    command: "/missing/toad-command".into(),
                    ..Start::default()
                },
                "alice",
            )
            .await
            .unwrap();
        assert_eq!(failure.state, "failed");
        assert_eq!(failure.exit_code, Some(127));
        assert!(jobs.status(&failure.id).await.unwrap().error.is_some());
        jobs.shutdown().await;
    }

    #[tokio::test]
    async fn stdin_works_without_a_visible_terminal_for_pipes_and_ptys() {
        let home = tempfile::tempdir().unwrap();
        let jobs = Jobs::new(home.path(), ":0");
        for pty in [false, true] {
            let mut start = command("read answer; printf 'received:%s' \"$answer\"");
            start.pty = pty;
            let job = jobs.start(start, "test").await.unwrap();
            jobs.write(&job.id, "hello world\n", false).await.unwrap();
            assert_eq!(jobs.wait(&job.id, 3000).await.unwrap().exit_code, Some(0));
            assert!(
                jobs.read(&job.id, 0, 65536)
                    .await
                    .unwrap()
                    .output
                    .contains("received:hello world")
            );
        }
    }

    #[tokio::test]
    async fn output_storage_failure_stops_and_reaps_the_job() {
        let directory = tempfile::tempdir().unwrap();
        let jobs = Jobs::new(directory.path(), "");
        let record = jobs
            .start(
                command("read line; while :; do printf output; done"),
                "test",
            )
            .await
            .unwrap();
        let job = jobs.job(&record.id).await.unwrap();
        std::fs::create_dir(job.directory.join("output.log")).unwrap();
        jobs.write(&record.id, "go\n", false).await.unwrap();
        let record = jobs.wait(&record.id, 3000).await.unwrap();
        assert_eq!(record.state, "failed");
        assert!(record.error.unwrap().contains("write job output"));
        assert_eq!(unsafe { libc::kill(record.pid.unwrap() as i32, 0) }, -1);
    }

    #[tokio::test]
    async fn output_is_bounded_and_history_survives_a_new_service() {
        let home = tempfile::tempdir().unwrap();
        let jobs = Jobs::new(home.path(), ":0");
        let job = jobs
            .start(command("head -c 5000000 /dev/zero"), "test")
            .await
            .unwrap();
        let result = jobs.wait(&job.id, 10000).await.unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.truncated);
        assert_eq!(result.output_bytes, OUTPUT_LIMIT);
        let restored = Jobs::new(home.path(), ":0");
        assert_eq!(
            restored.status(&job.id).await.unwrap().output_bytes,
            OUTPUT_LIMIT
        );
        let page = restored
            .read(&job.id, OUTPUT_LIMIT as u64 - 8, 16)
            .await
            .unwrap();
        assert_eq!(page.output.len(), 8);
        assert!(page.eof);
    }
}
