//! The key proxy: how a job uses a protected key without ever holding it.
//!
//! A variable the desk delivers with a `use` entry (the `PUT /secrets` v2
//! body) is not put in a job's environment. The job
//! gets a *stand-in* instead, made for that job alone, and its programs reach
//! the network through this proxy (`HTTPS_PROXY`). The proxy:
//!
//! - answers `CONNECT` only, and tunnels every host byte for byte unless the
//!   target is one of a binding's `use.origins`;
//! - for such an origin, terminates TLS with a leaf from a CA made at boot,
//!   whose key lives in this process alone;
//! - puts the real value in place of a live stand-in only in the binding's
//!   own header and only under one of its schemes (`Basic` is decoded,
//!   swapped and re-encoded), never in a body, a path, a query or another
//!   header;
//! - opens the upstream itself and adds the value only after the upstream
//!   proves the origin's name against the roots compiled into this binary;
//! - never follows a redirect: the job gets the 3xx and decides;
//! - writes the stand-in back over the literal value in every response it
//!   carries from a terminated origin, headers and body;
//! - refuses, before anything is sent, a request carrying a value to one of
//!   a short list of key-minting endpoints (`policy`).
//!
//! The stand-in table is keyed by job: `begin` makes a job's stand-ins and
//! `end` kills them. A delivery carries the desk's generation; an older one
//! is refused, and every stand-in of a variable that is no longer bound dies
//! with the delivery that unbinds it, so revoking stops a running job too.
//!
//! The proxy is given a bound listener (`serve`), so it runs on whatever
//! socket boot holds for it (`127.0.0.1:3128` in split mode).

mod ca;
mod conn;
mod policy;
mod scrub;
mod table;
#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use rustls::{ClientConfig, RootCertStore};
use serde::{Deserialize, Serialize};

pub use table::Binding;

/// How a protected variable is used, as the desk sends it inside `use`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Use {
    /// `https://host[:port]`, the only places the value is sent.
    pub origins: Vec<String>,
    /// The one request header the value goes in.
    #[serde(default = "authorization")]
    pub header: String,
    /// The schemes the header may carry it under (`Bearer`, `token`,
    /// `Basic`). None means the header's whole value is the key.
    #[serde(default)]
    pub schemes: Vec<String>,
    /// The stand-in's shape, so a CLI that checks a token's prefix still runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stand_in: Option<StandIn>,
    /// Key-minting routes on these origins the value never goes to, from
    /// the desk's `services.json`. Absent means the built-in list for the
    /// services Hotline names; empty means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuse: Option<Vec<String>>,
}

fn authorization() -> String {
    "Authorization".to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct StandIn {
    pub prefix: String,
    /// The whole stand-in's length, prefix included (`ghp_` + 36 is 40).
    pub length: usize,
}

/// The proxy, its CA and its table. Cloning shares them.
#[derive(Clone)]
pub struct KeyProxy {
    inner: Arc<Inner>,
}

struct Inner {
    ca: ca::Ca,
    table: Mutex<table::Table>,
    upstream: Upstream,
}

/// Where the proxy goes on a job's behalf, and what it trusts there.
struct Upstream {
    /// Verifies the upstream's name against `roots` and nothing else.
    tls: Arc<ClientConfig>,
    /// Tests send every host to a local fake upstream. Release builds have
    /// no such field: a bound origin is always the real name on the network.
    #[cfg(test)]
    route: Option<SocketAddr>,
}

impl Upstream {
    fn address(&self, host: &str, port: u16) -> Address {
        #[cfg(test)]
        if let Some(route) = self.route {
            return Address::Fixed(route);
        }
        Address::Name(host.to_owned(), port)
    }
}

enum Address {
    Name(String, u16),
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(SocketAddr),
}

impl KeyProxy {
    /// A proxy with a fresh CA, trusting only the compiled-in web roots
    /// upstream.
    pub fn new() -> Result<Self, String> {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        Self::with_roots(roots)
    }

    fn with_roots(roots: RootCertStore) -> Result<Self, String> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut tls = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            inner: Arc::new(Inner {
                ca: ca::Ca::new(provider)?,
                table: Mutex::new(table::Table::default()),
                upstream: Upstream {
                    tls: Arc::new(tls),
                    #[cfg(test)]
                    route: None,
                },
            }),
        })
    }

    #[cfg(test)]
    fn for_test(roots: RootCertStore, route: SocketAddr) -> Self {
        let mut proxy = Self::with_roots(roots).expect("proxy");
        Arc::get_mut(&mut proxy.inner)
            .expect("unshared")
            .upstream
            .route = Some(route);
        proxy
    }

    /// This boot's CA certificate, for the public bundle jobs trust.
    pub fn ca_pem(&self) -> &str {
        self.inner.ca.pem()
    }

    /// Replaces the protected variables with the desk's set at `generation`.
    /// All or nothing; an older generation than the last one is refused.
    /// Stand-ins of a name that is no longer bound die here.
    pub fn deliver(&self, generation: u64, bindings: Vec<Binding>) -> Result<(), String> {
        self.table().deliver(generation, bindings)
    }

    /// Makes `job`'s stand-ins, one per bound variable: `(NAME, stand-in)`
    /// for its environment. Calling it again for the same job adds none.
    pub fn begin(&self, job: &str) -> Vec<(String, String)> {
        self.table().begin(job)
    }

    /// Kills `job`'s stand-ins.
    pub fn end(&self, job: &str) {
        self.table().end(job);
    }

    /// Serves jobs on `listener` until the process ends.
    pub async fn serve(self, listener: std::net::TcpListener) -> std::io::Result<()> {
        listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        conn::serve(self.inner, listener).await
    }

    fn table(&self) -> std::sync::MutexGuard<'_, table::Table> {
        self.inner
            .table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
