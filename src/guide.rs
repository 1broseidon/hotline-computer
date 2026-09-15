use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn text() -> String {
    include_str!("../skills/toad-computer/SKILL.md")
        .replace("{{version}}", env!("CARGO_PKG_VERSION"))
        .replace("{{nixpkgs}}", crate::workspace::NIXPKGS)
}
pub fn manifest() -> Value {
    let skill = text();
    json!({"version":env!("CARGO_PKG_VERSION"),"sha256":format!("{:x}",Sha256::digest(skill.as_bytes())),"skill":skill,"catalog":crate::workspace::catalog()})
}
