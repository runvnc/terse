//! The local blob store (NETWORK 2): a directory with one file per blob, named
//! by its hex ID. Writes go to a temporary file that is renamed into place, so
//! a reader never sees a partial blob.
//!
//! On Windows a file another program has open without sharing it (a virus
//! scanner or the search indexer looking at a file just written, a backup),
//! or one being deleted while something still has it open, cannot be opened,
//! renamed over or removed for a moment. Every operation on a blob's file is
//! tried again briefly while it fails that way ([`retry_in_use`]), and an
//! error left after that is an error, never an absent blob.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::blob::{BlobId, blob_id, parse_hex, to_hex, valid_size};

#[derive(Debug)]
pub enum PutError {
    Empty,
    TooLarge,
    /// The store's size limit would be exceeded.
    Full,
    Io(io::Error),
}

impl std::fmt::Display for PutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "blob is empty"),
            Self::TooLarge => write!(f, "blob is larger than {} bytes", crate::blob::MAX_BLOB),
            Self::Full => write!(f, "blob store is full"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PutError {}

pub struct BlobStore {
    dir: PathBuf,
    /// Total bytes of blobs allowed, if limited.
    limit: Option<u64>,
    /// Bytes of blobs stored; also serialises writers.
    used: Mutex<u64>,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The waits between tries of [`retry_in_use`], in milliseconds: about a
/// quarter of a second in all.
const IN_USE_WAITS_MS: [u64; 8] = [1, 2, 4, 8, 16, 32, 64, 128];

/// Whether `e` is how Windows fails an operation on a file that is in use:
/// open in another program without sharing (ERROR_SHARING_VIOLATION,
/// ERROR_LOCK_VIOLATION), or deleted while still open (ERROR_DELETE_PENDING,
/// or ERROR_ACCESS_DENIED, which opening or renaming over such a file
/// gives). The same operation may succeed a moment later. Never true on
/// other systems, where a file in use can be read, renamed over and removed.
pub fn in_use(e: &io::Error) -> bool {
    cfg!(windows) && e.raw_os_error().is_some_and(windows_in_use)
}

/// The Windows error codes of [`in_use`].
fn windows_in_use(code: i32) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION,
    // ERROR_DELETE_PENDING.
    matches!(code, 5 | 32 | 33 | 303)
}

/// Runs a file operation, and again (for about a quarter of a second at
/// most) while it fails because the file is in use ([`in_use`]); returns
/// the last outcome.
pub fn retry_in_use<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_while(op, in_use)
}

fn retry_while<T>(mut op: impl FnMut() -> io::Result<T>, transient: impl Fn(&io::Error) -> bool) -> io::Result<T> {
    for ms in IN_USE_WAITS_MS {
        match op() {
            Err(e) if transient(&e) => std::thread::sleep(std::time::Duration::from_millis(ms)),
            r => return r,
        }
    }
    op()
}

impl BlobStore {
    /// Opens (creating if needed) the store in `dir`.
    pub fn open(dir: impl Into<PathBuf>, limit: Option<u64>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut used = 0;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_name().to_str().and_then(parse_hex).is_some() {
                used += entry.metadata()?.len();
            }
        }
        Ok(BlobStore { dir, limit, used: Mutex::new(used) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The total bytes of blobs allowed, if limited.
    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    pub fn used_bytes(&self) -> u64 {
        *self.used.lock().unwrap()
    }

    pub fn path(&self, id: &BlobId) -> PathBuf {
        self.dir.join(to_hex(id))
    }

    /// The stored bytes for `id`, unverified (callers that do not trust the
    /// disk check the hash themselves). `None` only if there is no such
    /// file: one that is there but cannot be read (after [`retry_in_use`])
    /// is an error.
    pub fn read(&self, id: &BlobId) -> io::Result<Option<Vec<u8>>> {
        let path = self.path(id);
        match retry_in_use(|| fs::read(&path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Stores a blob and returns its ID. Storing a blob already present is a no-op.
    pub fn put(&self, bytes: &[u8]) -> Result<BlobId, PutError> {
        self.put_new(bytes).map(|(id, _)| id)
    }

    /// Stores a blob; returns its ID and whether it was not stored before.
    pub fn put_new(&self, bytes: &[u8]) -> Result<(BlobId, bool), PutError> {
        self.put_with(bytes, true)
    }

    /// Stores a blob without forcing it to disk first: for blobs the host
    /// can always produce again (its built-in programs and manual). A sync
    /// per blob can take seconds on a busy disk, and these are written
    /// before the window opens. Reads verify a blob against its ID, so a
    /// blob lost to a crash is noticed and stored again.
    pub fn put_unsynced(&self, bytes: &[u8]) -> Result<BlobId, PutError> {
        self.put_with(bytes, false).map(|(id, _)| id)
    }

    fn put_with(&self, bytes: &[u8], sync: bool) -> Result<(BlobId, bool), PutError> {
        if bytes.is_empty() {
            return Err(PutError::Empty);
        }
        if !valid_size(bytes.len()) {
            return Err(PutError::TooLarge);
        }
        let id = blob_id(bytes);
        let path = self.path(&id);
        let mut used = self.used.lock().unwrap();
        if path.exists() {
            return Ok((id, false));
        }
        if self.limit.is_some_and(|limit| *used + bytes.len() as u64 > limit) {
            return Err(PutError::Full);
        }
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = self.dir.join(format!(".{}.{}.{n}.tmp", to_hex(&id), std::process::id()));
        let written = (|| {
            let mut f = fs::File::create(&temp)?;
            f.write_all(bytes)?;
            if sync {
                f.sync_all()?;
            }
            drop(f);
            retry_in_use(|| fs::rename(&temp, &path))
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(PutError::Io(e));
        }
        *used += bytes.len() as u64;
        Ok((id, true))
    }

    /// Deletes a blob (e.g. one whose bytes no longer match its ID).
    pub fn remove(&self, id: &BlobId) -> io::Result<()> {
        let mut used = self.used.lock().unwrap();
        let path = self.path(id);
        let len = retry_in_use(|| fs::metadata(&path))?.len();
        retry_in_use(|| fs::remove_file(&path))?;
        *used = used.saturating_sub(len);
        Ok(())
    }
}

/// The default store: `$XDG_CACHE_HOME/terse/blobs`, else
/// `~/.cache/terse/blobs` (`~/Library/Caches/terse/blobs` on macOS,
/// `%LOCALAPPDATA%\terse\blobs` on Windows;
/// `terse_dirs`).
pub fn default_store_dir() -> Option<PathBuf> {
    Some(terse_dirs::user_dir(terse_dirs::Kind::Cache)?.join("blobs"))
}

/// The default directory for data a host keeps for itself (the relay
/// `terse-host` starts keeps its store in `relay` there):
/// `$XDG_DATA_HOME/terse`, else `~/.local/share/terse` (`~/Library/Application
/// Support/terse/data` on macOS, `%LOCALAPPDATA%\terse`
/// on Windows; `terse_dirs`).
pub fn default_data_dir() -> Option<PathBuf> {
    terse_dirs::user_dir(terse_dirs::Kind::Data)
}

/// The default directory for the identity and subscriptions:
/// `$XDG_CONFIG_HOME/terse`, else `~/.config/terse` (`~/Library/Application
/// Support/terse` on macOS, `%APPDATA%\terse` on
/// Windows; `terse_dirs`).
pub fn default_config_dir() -> Option<PathBuf> {
    terse_dirs::user_dir(terse_dirs::Kind::Config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::MAX_BLOB;

    #[test]
    fn put_read_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), Some(10)).unwrap();
        let id = store.put(b"hello").unwrap();
        assert_eq!(id, blob_id(b"hello"));
        assert_eq!(store.read(&id).unwrap().as_deref(), Some(&b"hello"[..]));
        assert_eq!(store.put(b"hello").unwrap(), id, "idempotent");
        assert_eq!(store.used_bytes(), 5);
        assert!(matches!(store.put(b"too much"), Err(PutError::Full)));
        assert!(matches!(store.put(b""), Err(PutError::Empty)));
        assert!(matches!(store.put(&vec![0; MAX_BLOB + 1]), Err(PutError::TooLarge)));
        assert_eq!(store.read(&blob_id(b"other")).unwrap(), None);
        // No temporary files left behind; the size is recounted on open.
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from(to_hex(&id))]);
        assert_eq!(BlobStore::open(dir.path(), None).unwrap().used_bytes(), 5);
        store.remove(&id).unwrap();
        assert_eq!(store.read(&id).unwrap(), None);
        assert_eq!(store.used_bytes(), 0);
    }

    /// A file operation failing because the file is in use (fault-injected
    /// here with the Windows codes) is tried again until it succeeds; any
    /// other error is returned at once, and one that persists is returned
    /// after about a quarter of a second.
    #[test]
    fn in_use_is_tried_again() {
        let transient = |e: &io::Error| e.raw_os_error().is_some_and(windows_in_use);
        for code in [5, 32, 33, 303] {
            let mut calls = 0;
            let r = retry_while(
                || {
                    calls += 1;
                    if calls < 4 { Err(io::Error::from_raw_os_error(code)) } else { Ok(calls) }
                },
                transient,
            );
            assert_eq!(r.unwrap(), 4, "code {code}");
        }
        let mut calls = 0;
        let r: io::Result<()> = retry_while(
            || {
                calls += 1;
                Err(io::Error::from_raw_os_error(32))
            },
            transient,
        );
        assert_eq!((r.unwrap_err().raw_os_error(), calls), (Some(32), IN_USE_WAITS_MS.len() + 1));
        let mut calls = 0;
        let r: io::Result<()> = retry_while(
            || {
                calls += 1;
                Err(io::ErrorKind::NotFound.into())
            },
            transient,
        );
        assert_eq!((r.unwrap_err().kind(), calls), (io::ErrorKind::NotFound, 1));
        // Only Windows reports a file in use this way.
        assert_eq!(in_use(&io::Error::from_raw_os_error(32)), cfg!(windows));
        assert!(!windows_in_use(2), "ERROR_FILE_NOT_FOUND is an absent file");
    }

    /// A blob file that is there but cannot be read is an error, not an
    /// absent blob (a directory in its place stands for a file in use).
    #[test]
    fn unreadable_is_not_absent() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path(), None).unwrap();
        let id = blob_id(b"held");
        fs::create_dir(store.path(&id)).unwrap();
        assert!(store.read(&id).is_err());
    }
}
