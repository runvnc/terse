//! Who a host or a device implementation is (`PERFORMANCE.md` R13): its
//! name, its version and a build identifier.
//!
//! The build identifier is a short tag naming a hash, a colon and the
//! first 8 bytes of that hash of the program's own file, in lowercase
//! hexadecimal: `b3:` and BLAKE3 for the reference programs. It is
//! self-reported: a way for honest implementations to be told apart in
//! a record, not a security measure (anyone can write any identifier).

use std::sync::OnceLock;

/// The workspace's version, which every reference program carries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The longest version or build identifier (bytes, printable ASCII
/// without spaces), as `DEVICES.md` 1.1 bounds them.
pub const MAX_FIELD: usize = 32;

/// A host's or a device implementation's identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub version: String,
    pub build: String,
}

impl Identity {
    /// This program's identity under `name`: the workspace version and
    /// the build identifier of its executable.
    pub fn this(name: &str) -> Identity {
        Identity { name: name.to_string(), version: VERSION.to_string(), build: build_id().to_string() }
    }

    /// The one line `--version` prints: name, version, build.
    pub fn line(&self) -> String {
        format!("{} {} {}", self.name, field(&self.version), field(&self.build))
    }
}

fn field(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

/// Whether `s` may be a version or build identifier: at most 32 bytes of
/// printable ASCII without spaces (empty: not given).
pub fn valid_field(s: &str) -> bool {
    s.len() <= MAX_FIELD && s.bytes().all(|b| b.is_ascii_graphic())
}

/// Keeps what a peer reported only if it is a valid field (else empty).
pub fn clean_field(s: &str) -> String {
    if valid_field(s) { s.to_string() } else { String::new() }
}

/// `b3:` and the first 8 bytes of BLAKE3 of `bytes`, in hex.
pub fn build_of(bytes: &[u8]) -> String {
    let h = blake3::hash(bytes);
    let mut s = String::from("b3:");
    for b in &h.as_bytes()[..8] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// This executable's build identifier, computed once (reading and hashing
/// the program file: a few milliseconds, measured in `PERFORMANCE.md`);
/// `unknown` if the file cannot be read.
pub fn build_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| match std::env::current_exe().and_then(std::fs::read) {
        Ok(bytes) => build_of(&bytes),
        Err(_) => "unknown".to_string(),
    })
}

/// Prints `name`'s identity line and returns true if `--version` is among
/// `args` (for the drivers, which take it before anything else).
pub fn print_if_asked(name: &str, args: &[String]) -> bool {
    if args.iter().any(|a| a == "--version") {
        println!("{}", Identity::this(name).line());
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identifiers() {
        assert_eq!(build_of(b""), "b3:af1349b9f5f9a1a6");
        let id = build_id();
        assert!(id.starts_with("b3:") && id.len() == 19, "{id}");
        assert!(valid_field(id) && valid_field("0.1.0") && valid_field(""));
        assert!(!valid_field("a b") && !valid_field(&"x".repeat(33)) && !valid_field("é"));
        let i = Identity { name: "terse-host".into(), version: "0.1.0".into(), build: String::new() };
        assert_eq!(i.line(), "terse-host 0.1.0 -");
    }
}
