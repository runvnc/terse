//! Embeds the built manual (docs/manual/build, written by `terse manual
//! build`): one entry per line of its manifest, `<id> <name> <title>`.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../docs/manual/build");
    println!("cargo:rerun-if-changed={}", dir.display());
    let manifest = dir.join("manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).unwrap_or_default();
    let mut out = String::from("/// The built manual, in reading order: (name, title, text).\npub const MANUAL: &[(&str, &str, &[u8])] = &[\n");
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.splitn(3, ' ');
        let (_id, name, title) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap_or(""));
        let file = dir.join(format!("{name}.txt"));
        println!("cargo:rerun-if-changed={}", file.display());
        let _ = writeln!(out, "    ({name:?}, {title:?}, include_bytes!({:?})),", file.display().to_string());
    }
    out.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("manual.rs"), out).unwrap();
}
