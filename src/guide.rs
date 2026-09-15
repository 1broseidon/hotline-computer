use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn text() -> String {
    include_str!("../skills/toad-computer/SKILL.md")
        .replace("{{version}}", env!("CARGO_PKG_VERSION"))
        .replace("{{nixpkgs}}", crate::workspace::NIXPKGS)
        .replace("{{channel}}", channel())
        .replace("{{revision}}", revision())
}
pub fn manifest() -> Value {
    let skill = text();
    json!({"version":env!("CARGO_PKG_VERSION"),"build":identity(),"sha256":format!("{:x}",Sha256::digest(skill.as_bytes())),"skill":skill,"catalog":crate::workspace::catalog()})
}

fn channel() -> &'static str {
    option_env!("TOAD_BUILD_CHANNEL").unwrap_or("development")
}
fn revision() -> &'static str {
    option_env!("TOAD_BUILD_REVISION").unwrap_or("unknown")
}
pub fn identity() -> Value {
    json!({"channel":channel(),"revision":revision()})
}
