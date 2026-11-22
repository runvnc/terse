//! The local blob store (NETWORK 2): a directory with one file per blob, named
//! by its hex ID. Writes go to a temporary file that is renamed into place, so
//! a reader never sees a partial blob.

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

    pub fn used_bytes(&self) -> u64 {
        *self.used.lock().unwrap()
    }

    pub fn path(&self, id: &BlobId) -> PathBuf {
        self.dir.join(to_hex(id))
    }

    /// The stored bytes for `id`, unverified (callers that do not trust the
    /// disk check the hash themselves).
    pub fn read(&self, id: &BlobId) -> io::Result<Option<Vec<u8>>> {
        match fs::read(self.path(id)) {
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
            fs::rename(&temp, &path)
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
        let len = fs::metadata(&path)?.len();
        fs::remove_file(path)?;
        *used = used.saturating_sub(len);
        Ok(())
    }
}

/// `$<xdg>/terse`, else `~/<home_sub>/terse` (`%<windows>%\terse` on
/// Windows). The XDG variable, when set to an absolute path, is used on
/// every system, Windows included: it is how a test or a second profile
/// keeps away from the user's own folders.
fn user_dir(windows: &str, xdg: &str, home_sub: &str) -> Option<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let base = match std::env::var_os(xdg).and_then(absolute) {
        Some(p) => p,
        None if cfg!(windows) => std::env::var_os(windows).and_then(absolute)?,
        None => std::env::var_os("HOME").and_then(absolute)?.join(home_sub),
    };
    Some(base.join("terse"))
}

/// The default store: `$XDG_CACHE_HOME/terse/blobs`, else
/// `~/.cache/terse/blobs` (`%LOCALAPPDATA%\terse\blobs` on Windows).
pub fn default_store_dir() -> Option<PathBuf> {
    Some(user_dir("LOCALAPPDATA", "XDG_CACHE_HOME", ".cache")?.join("blobs"))
}

/// The default directory for data a host keeps for itself (the relay
/// `terse-host` starts keeps its store in `relay` there):
/// `$XDG_DATA_HOME/terse`, else `~/.local/share/terse` (`%LOCALAPPDATA%\terse`
/// on Windows).
pub fn default_data_dir() -> Option<PathBuf> {
    user_dir("LOCALAPPDATA", "XDG_DATA_HOME", ".local/share")
}

/// The default directory for the identity and subscriptions:
/// `$XDG_CONFIG_HOME/terse`, else `~/.config/terse` (`%APPDATA%\terse` on
/// Windows).
pub fn default_config_dir() -> Option<PathBuf> {
    user_dir("APPDATA", "XDG_CONFIG_HOME", ".config")
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
}
