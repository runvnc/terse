//! The host's built-in programs (SPEC 12.2 `builtin`) and the shell: the
//! images every host binary carries, put into its store at start so that a
//! shell can start them by ID.

use crate::store::BlobStore;
use crate::BlobId;

/// The shell (programs/shell), run when no image is named.
pub const SHELL: &[u8] = include_bytes!("../../../programs/shell/shell.terse");

/// The built-in programs, in the order `builtin` lists them.
pub const PROGRAMS: [&[u8]; 4] = [
    include_bytes!("../../../programs/reader/reader.terse"),
    include_bytes!("../../../programs/sparks/sparks.terse"),
    include_bytes!("../../../programs/fetch/fetch.terse"),
    include_bytes!("../../../programs/hello/hello.terse"),
];

/// Other blobs a built-in program needs: the message `fetch` shows.
const DATA: [&[u8]; 1] = [include_bytes!("../../../programs/fetch/message.txt")];

/// Puts the built-in programs (and their data) and `extra` images into
/// `store`; returns the IDs of the programs, then of `extra`.
pub fn install(store: &BlobStore, extra: &[Vec<u8>]) -> Result<Vec<BlobId>, String> {
    for data in DATA {
        store.put(data).map_err(|e| format!("blob store: {e}"))?;
    }
    let mut ids = Vec::new();
    for image in PROGRAMS.iter().copied().chain(extra.iter().map(Vec::as_slice)) {
        ids.push(store.put(image).map_err(|e| format!("blob store: {e}"))?);
    }
    Ok(ids)
}
