//! Protected keys in the split computer, the broker's half. The broker runs
//! the key proxy (`crate::keyproxy`) on the socket PID 1 holds for it and
//! publishes the proxy's CA for every user to trust. A protected key's value
//! goes to the proxy alone: never to the runner, a job's environment or a
//! log. Each job the runner starts asks here for its environment: a stand-in
//! per protected key, and the variables that send its programs through the
//! proxy and trust its CA. When the job ends the runner says so, and its
//! stand-ins die.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::keyproxy::{Binding, KeyProxy};

/// What the system trusts, which the public bundle starts with so that a job
/// sent through the proxy still reaches every other host as before.
const SYSTEM_ROOTS: &str = "/etc/ssl/certs/ca-certificates.crt";

/// The variables a job with stand-ins is given besides them. A protected
/// key may not take one of these names.
pub const JOB_VARIABLES: [&str; 11] = [
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
    "NODE_USE_ENV_PROXY",
    "SSL_CERT_FILE",
    "NIX_SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
];

/// The proxy, where jobs reach it, and the bundle that trusts it.
#[derive(Clone)]
pub struct Keys {
    proxy: KeyProxy,
    url: String,
    bundle: PathBuf,
}

impl Keys {
    /// A fresh proxy with this boot's CA, serving on `listener`, with the
    /// bundle written at `bundle`. Must run inside the broker's runtime.
    pub fn start(listener: std::net::TcpListener, bundle: &Path) -> Result<Keys, String> {
        let proxy = KeyProxy::new()?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let roots = match std::fs::read_to_string(SYSTEM_ROOTS) {
            Ok(roots) => roots,
            Err(error) => {
                eprintln!(
                    "hotline-computer: key proxy: no system roots at {SYSTEM_ROOTS} ({error}); jobs with stand-ins trust this boot's CA alone"
                );
                String::new()
            }
        };
        write_bundle(bundle, &roots, proxy.ca_pem())?;
        let serving = proxy.clone();
        tokio::spawn(async move {
            if let Err(error) = serving.serve(listener).await {
                eprintln!("hotline-computer: key proxy stopped: {error}");
            }
        });
        Ok(Keys::with(proxy, address, bundle))
    }

    pub(crate) fn with(proxy: KeyProxy, address: std::net::SocketAddr, bundle: &Path) -> Keys {
        Keys {
            proxy,
            url: format!("http://{address}"),
            bundle: bundle.to_owned(),
        }
    }

    /// The desk's protected keys at `generation`, all or none. A delivery
    /// with no generation (a v1 body, from a desk that knows no protected
    /// key) unbinds every key at the generation already held.
    pub fn deliver(&self, generation: Option<u64>, bindings: Vec<Binding>) -> Result<(), String> {
        let generation = match generation {
            Some(generation) => generation,
            None => self.proxy.generation().unwrap_or(0),
        };
        self.proxy.deliver(generation, bindings)
    }

    /// `job`'s environment: nothing when no key is bound, or else a stand-in
    /// per key and the proxy and CA variables.
    pub fn begin(&self, job: &str) -> BTreeMap<String, String> {
        let mut environment: BTreeMap<String, String> = self.proxy.begin(job).into_iter().collect();
        if environment.is_empty() {
            return environment;
        }
        let bundle = self.bundle.to_string_lossy().into_owned();
        for (name, value) in [
            ("HTTPS_PROXY", self.url.as_str()),
            ("https_proxy", self.url.as_str()),
            ("NO_PROXY", "localhost,127.0.0.1,::1"),
            ("no_proxy", "localhost,127.0.0.1,::1"),
            ("NODE_USE_ENV_PROXY", "1"),
        ] {
            environment.insert(name.to_owned(), value.to_owned());
        }
        for name in [
            "SSL_CERT_FILE",
            "NIX_SSL_CERT_FILE",
            "CURL_CA_BUNDLE",
            "GIT_SSL_CAINFO",
            "REQUESTS_CA_BUNDLE",
            "NODE_EXTRA_CA_CERTS",
        ] {
            environment.insert(name.to_owned(), bundle.clone());
        }
        environment
    }

    pub fn end(&self, job: &str) {
        self.proxy.end(job);
    }
}

/// The system roots and this boot's CA, 0644, replaced whole.
fn write_bundle(path: &Path, roots: &str, ca: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let temporary = path.with_extension("pem.tmp");
    let _ = std::fs::remove_file(&temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    let mut text = roots.trim_end().to_owned();
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(ca);
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// The runner's requests about a job's keys, or `None` for another op.
pub fn answer(keys: Option<&Keys>, op: &str, args: &Value) -> Option<Result<Value, String>> {
    let job = || {
        args["job"]
            .as_str()
            .filter(|job| !job.is_empty() && job.len() <= 128)
            .ok_or_else(|| "a job is named by its id".to_owned())
    };
    match op {
        "keys.begin" => Some(
            job().map(|job| json!({"env": keys.map(|keys| keys.begin(job)).unwrap_or_default()})),
        ),
        "keys.end" => Some(job().map(|job| {
            if let Some(keys) = keys {
                keys.end(job);
            }
            Value::Null
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
