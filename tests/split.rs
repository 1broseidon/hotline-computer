//! The split computer, from where a job stands: run against a container with
//! `HOTLINE_COMPUTER_URL`, `HOTLINE_COMPUTER_TOKEN` and
//! `HOTLINE_COMPUTER_MODE` set to `split` (created with `--user 0 --cap-add
//! SETUID --cap-add SETGID`) or `legacy` (created as today). Each test
//! skips without them, or when the container is the other mode.

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientInfo, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde_json::{Value, json};

type Client = RunningService<rmcp::RoleClient, ClientInfo>;

struct Computer {
    base: String,
    token: String,
    client: Client,
}

async fn computer(mode: &str) -> Option<Computer> {
    let base = std::env::var("HOTLINE_COMPUTER_URL").ok()?;
    if std::env::var("HOTLINE_COMPUTER_MODE").ok()?.as_str() != mode {
        eprintln!("skipped: this container is not booted {mode}");
        return None;
    }
    let token = std::env::var("HOTLINE_COMPUTER_TOKEN").unwrap_or_default();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("x-computer-holder"),
        HeaderValue::from_static("split-test"),
    );
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(format!("{base}/mcp")),
    );
    let client = ClientInfo::new(
        Default::default(),
        Implementation::new("hotline-computer-split", "1"),
    )
    .serve(transport)
    .await
    .expect("MCP handshake");
    Some(Computer {
        base,
        token,
        client,
    })
}

impl Computer {
    /// A Python program run as a job; its stdout, parsed as JSON. All of it:
    /// with the signed-in browser up, the process list is past exec's 8 KiB.
    async fn job(&self, program: &str) -> Value {
        let answer = self
            .client
            .peer()
            .call_tool(
                CallToolRequestParams::new("shell").with_arguments(
                    json!({"action": "exec", "command": "python3", "args": ["-c", program], "timeout": 40, "max_output": 1048576})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .expect("shell exec");
        let text: String = answer
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|content| content.text.clone()))
            .collect();
        let exec: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
        let stdout = exec["stdout"].as_str().unwrap_or_default();
        serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("{exec}"))
    }

    async fn capabilities(&self, nonce: &str) -> Value {
        reqwest::Client::new()
            .get(format!("{}/capabilities?nonce={nonce}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .expect("capabilities")
            .json()
            .await
            .expect("capabilities JSON")
    }

    /// This boot's attest key, as the desk takes it: once per boot, so it
    /// is kept for the rest of this run.
    async fn attest_key(&self) -> Vec<u8> {
        use base64::Engine;
        static KEY: tokio::sync::OnceCell<Vec<u8>> = tokio::sync::OnceCell::const_new();
        KEY.get_or_init(|| async {
            let answer: Value = reqwest::Client::new()
                .post(format!("{}/capabilities/key", self.base))
                .bearer_auth(&self.token)
                .send()
                .await
                .expect("the attest key")
                .json()
                .await
                .expect("the attest key JSON");
            base64::engine::general_purpose::STANDARD
                .decode(answer["key"].as_str().unwrap_or_else(|| panic!("{answer}")))
                .unwrap()
        })
        .await
        .clone()
    }

    /// The attest the desk expects: under this boot's key, of the mode,
    /// the boot, the release and the nonce.
    async fn attest(&self, boot: &str, mode: &str, version: &str, nonce: &str) -> String {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        let key = self.attest_key().await;
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).unwrap();
        mac.update(format!("hotline-broker-v2|{boot}|{mode}|{version}|{nonce}").as_bytes());
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    }
}

/// Every process the job can see: pid, uids, capabilities, no-new-privs and
/// command line, read from `/proc/<pid>/status` and `cmdline` (both open to
/// every user).
const PROCESSES: &str = r#"
import json, os
out = []
for pid in sorted(int(p) for p in os.listdir('/proc') if p.isdigit()):
    try:
        status = dict(line.split(':', 1) for line in open(f'/proc/{pid}/status').read().splitlines() if ':' in line)
        cmdline = open(f'/proc/{pid}/cmdline', 'rb').read().split(b'\0')
    except Exception:
        continue
    out.append({
        'pid': pid,
        'uid': [int(x) for x in status['Uid'].split()],
        'eff': int(status['CapEff'], 16), 'prm': int(status['CapPrm'], 16),
        'inh': int(status['CapInh'], 16), 'amb': int(status.get('CapAmb', '0'), 16),
        'nnp': int(status.get('NoNewPrivs', '0')),
        'cmd': [c.decode(errors='replace') for c in cmdline if c],
        'self': pid == os.getpid(),
    })
sockets = []
for fd in os.listdir('/proc/self/fd'):
    try: sockets.append(os.readlink(f'/proc/self/fd/{fd}'))
    except OSError: pass
print(json.dumps({'processes': out, 'fds': sockets}))
"#;

#[tokio::test(flavor = "multi_thread")]
async fn boot_splits_and_keeps_no_caps() {
    let Some(computer) = computer("split").await else {
        return;
    };
    let seen = computer.job(PROCESSES).await;
    let processes = seen["processes"].as_array().unwrap();
    let find = |argument: &str| {
        processes
            .iter()
            .find(|process| process["cmd"] == json!(["hotline-computer", argument]))
            .unwrap_or_else(|| panic!("no {argument} process: {seen}"))
            .clone()
    };
    let init = processes
        .iter()
        .find(|process| process["pid"] == 1)
        .unwrap();
    assert_eq!(init["uid"], json!([0, 0, 0, 0]), "PID 1 is root: {init}");
    let setid = (1 << 6) | (1 << 7);
    assert_eq!(
        init["eff"], setid,
        "PID 1 keeps SETUID and SETGID alone: {init}"
    );
    assert_eq!(init["prm"], setid, "{init}");
    assert_eq!(
        (init["inh"].as_u64(), init["amb"].as_u64()),
        (Some(0), Some(0)),
        "{init}"
    );
    let broker = find("__broker");
    let runner = find("__runner");
    assert_eq!(broker["uid"], json!([1001, 1001, 1001, 1001]), "{broker}");
    assert_eq!(runner["uid"], json!([1000, 1000, 1000, 1000]), "{runner}");
    for process in processes {
        if process["pid"] == 1 {
            continue;
        }
        assert_eq!(
            (
                process["eff"].as_u64(),
                process["prm"].as_u64(),
                process["amb"].as_u64()
            ),
            (Some(0), Some(0), Some(0)),
            "no process but PID 1 holds a capability: {process}"
        );
        assert_eq!(process["nnp"], 1, "nothing gains a privilege: {process}");
        assert_ne!(process["uid"][0], 0, "nothing else is root: {process}");
    }
    let job = processes
        .iter()
        .find(|process| process["self"] == true)
        .unwrap();
    assert_eq!(
        job["uid"],
        json!([1000, 1000, 1000, 1000]),
        "a job is agent: {job}"
    );
    // A job inherits none of the internal lines.
    for fd in seen["fds"].as_array().unwrap() {
        assert!(
            !fd.as_str().unwrap().starts_with("socket:"),
            "a job holds a socket it did not open: {seen}"
        );
    }
    let answer = computer.capabilities("split-n1").await;
    assert_eq!(answer["protected"]["v"], 2, "{answer}");
    assert_eq!(answer["protected"]["mode"], "split", "{answer}");
    let boot = answer["boot"].as_str().unwrap();
    let version = answer["version"].as_str().unwrap();
    assert_eq!(
        answer["attest"],
        computer.attest(boot, "split", version, "split-n1").await,
        "{answer}"
    );
    let again = reqwest::Client::new()
        .post(format!("{}/capabilities/key", computer.base))
        .bearer_auth(&computer.token)
        .send()
        .await
        .expect("a second claim");
    assert_eq!(again.status(), 409, "the key is handed out once a boot");
}

const BIND: &str = r#"
import errno, json, socket
out = {}
for family, host, port in [(socket.AF_INET, '0.0.0.0', 8787), (socket.AF_INET, '127.0.0.1', 8787),
                           (socket.AF_INET, '127.0.0.1', 3128), (socket.AF_INET, '0.0.0.0', 3128),
                           (socket.AF_INET6, '::', 8787)]:
    s = socket.socket(family, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if hasattr(socket, 'SO_REUSEPORT'):
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    if family == socket.AF_INET6:
        s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
    try:
        s.bind((host, port)); s.listen(1); out[f'{host}:{port}'] = 'BOUND'
    except OSError as e:
        out[f'{host}:{port}'] = errno.errorcode.get(e.errno, str(e.errno))
    s.close()
print(json.dumps(out))
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_job_cannot_bind_the_door_or_proxy_port() {
    let Some(computer) = computer("split").await else {
        return;
    };
    let bound = computer.job(BIND).await;
    for (address, outcome) in bound.as_object().unwrap() {
        // A runtime with no IPv6 has no `::` to bind at all.
        let fine =
            outcome == "EADDRINUSE" || (address.starts_with("::") && outcome == "EADDRNOTAVAIL");
        assert!(fine, "a job took {address}: {bound}");
    }
}

const REACH: &str = r#"
import ctypes, json, os, signal, socket
def attempt(f):
    try:
        f(); return 'OPEN'
    except OSError as e:
        return type(e).__name__ + ':' + os.strerror(e.errno)
procs = {}
for p in os.listdir('/proc'):
    if not p.isdigit(): continue
    try: cmd = open(f'/proc/{p}/cmdline','rb').read().split(b'\0')
    except Exception: continue
    if cmd[:2] == [b'hotline-computer', b'__broker']: procs['broker'] = int(p)
    if cmd[:2] == [b'hotline-computer', b'__runner']: procs['runner'] = int(p)
b, r = procs['broker'], procs['runner']
libc = ctypes.CDLL(None, use_errno=True)
def ptrace(pid):
    if libc.ptrace(16, pid, 0, 0) == 0:
        libc.ptrace(17, pid, 0, 0); return 'OPEN'
    return os.strerror(ctypes.get_errno())
def connect(path):
    s = socket.socket(socket.AF_UNIX); s.connect(path); s.close()
out = {
  'X1 broker dir': attempt(lambda: os.listdir('/run/hotline-computer/broker')),
  'X2 broker environ': attempt(lambda: open(f'/proc/{b}/environ','rb').read()),
  'X2 broker mem': attempt(lambda: open(f'/proc/{b}/mem','rb').read(1)),
  'X2 runner mem': attempt(lambda: open(f'/proc/{r}/mem','rb').read(1)),
  'X2 runner fds': attempt(lambda: os.listdir(f'/proc/{r}/fd')),
  'B6 pid 1 environ': attempt(lambda: open('/proc/1/environ','rb').read()),
  'X4 ptrace broker': ptrace(b),
  'X4 ptrace runner': ptrace(r),
  'X5 bridge socket': attempt(lambda: connect('/run/hotline-computer/bridge/bridge.sock')),
  'X7 kill broker': attempt(lambda: os.kill(b, 0)),
  'X7 kill pid 1': attempt(lambda: os.kill(1, 0)),
  'signin profile': attempt(lambda: os.listdir('/var/lib/hotline-computer/signin')),
}
print(json.dumps(out))
"#;

/// X1-X7 of the design's job probes, against the real computer: nothing of
/// the broker's is within a job's reach.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_reaches_nothing_of_the_brokers() {
    let Some(computer) = computer("split").await else {
        return;
    };
    let reached = computer.job(REACH).await;
    for (probe, outcome) in reached.as_object().unwrap() {
        let outcome = outcome.as_str().unwrap();
        assert!(
            outcome.contains("Permission denied") || outcome.contains("Operation not permitted"),
            "{probe} reached it: {outcome} ({reached})"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_boot_is_todays_single_user() {
    let Some(computer) = computer("legacy").await else {
        return;
    };
    let seen = computer.job(PROCESSES).await;
    for process in seen["processes"].as_array().unwrap() {
        assert_eq!(
            process["uid"],
            json!([1000, 1000, 1000, 1000]),
            "every process is agent: {process}"
        );
        assert_eq!(process["eff"], 0, "{process}");
    }
    let init = seen["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|process| process["pid"] == 1)
        .unwrap();
    assert_eq!(
        init["cmd"],
        json!(["/usr/bin/hotline-computer", "boot"]),
        "PID 1 is the single-user computer's boot: {init}"
    );
    let answer = computer.capabilities("legacy-n1").await;
    assert_eq!(answer["protected"]["v"], 2, "{answer}");
    assert_eq!(answer["protected"]["mode"], "legacy", "{answer}");
    let boot = answer["boot"].as_str().unwrap();
    let version = answer["version"].as_str().unwrap();
    assert_eq!(
        answer["attest"],
        computer.attest(boot, "legacy", version, "legacy-n1").await,
        "{answer}"
    );
    let again = reqwest::Client::new()
        .post(format!("{}/capabilities/key", computer.base))
        .bearer_auth(&computer.token)
        .send()
        .await
        .expect("a second claim");
    assert_eq!(again.status(), 409, "the key is handed out once a boot");
}
