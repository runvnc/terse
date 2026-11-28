//! The host's built-in programs (SPEC §machines.calls `builtin`) and the shell: the
//! images every host binary carries, put into its store at start so that a
//! shell can start them by ID.

use crate::store::BlobStore;
use crate::BlobId;

/// The shell (programs/shell), run when no image is named.
pub const SHELL: &[u8] = include_bytes!("../../../programs/shell/shell.terse");

/// The built-in programs, in the order `builtin` lists them.
pub const PROGRAMS: [&[u8]; 9] = [
    include_bytes!("../../../programs/reader/reader.terse"),
    include_bytes!("../../../programs/sparks/sparks.terse"),
    include_bytes!("../../../programs/fetch/fetch.terse"),
    include_bytes!("../../../programs/hello/hello.terse"),
    include_bytes!("../../../programs/sketch/sketch.terse"),
    include_bytes!("../../../programs/holo/holo.terse"),
    include_bytes!("../../../programs/reports/reports.terse"),
    include_bytes!("../../../programs/reef/reef.terse"),
    include_bytes!("../../../programs/sheet/sheet.terse"),
];

/// Other blobs a built-in program needs: the message `fetch` shows.
pub const DATA: [&[u8]; 1] = [include_bytes!("../../../programs/fetch/message.txt")];

// The manual (docs/manual/build): `MANUAL`, (name, title, text) in
// reading order, the index first.
include!(concat!(env!("OUT_DIR"), "/manual.rs"));

/// Puts the manual's articles into `store` (a host does at start, also
/// when it runs one image on its own, so that its reader finds them).
pub fn install_manual(store: &BlobStore) -> Result<(), String> {
    for m in MANUAL {
        store.put_unsynced(m.2).map_err(|e| format!("blob store: {e}"))?;
    }
    Ok(())
}

/// Puts the blobs every host carries besides the programs into `store`:
/// the built-in programs' data and the manual's articles.
pub fn install_data(store: &BlobStore) -> Result<(), String> {
    for data in DATA.iter().copied().chain(MANUAL.iter().map(|m| m.2)) {
        store.put_unsynced(data).map_err(|e| format!("blob store: {e}"))?;
    }
    Ok(())
}

/// Puts the built-in programs (and their data, and the manual) and `extra`
/// images into `store`; returns the IDs of the programs, then of `extra`.
pub fn install(store: &BlobStore, extra: &[Vec<u8>]) -> Result<Vec<BlobId>, String> {
    install_data(store)?;
    let mut ids = Vec::new();
    for image in PROGRAMS.iter().copied().chain(extra.iter().map(Vec::as_slice)) {
        ids.push(store.put_unsynced(image).map_err(|e| format!("blob store: {e}"))?);
    }
    Ok(ids)
}
