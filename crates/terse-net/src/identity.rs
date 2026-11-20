//! Feed identities (NETWORK 4.1): an Ed25519 keypair whose public key names a
//! feed. The secret key file holds the 32-byte seed as 64 hex digits and a
//! newline; it is created owner-only (0600 on Unix) and never overwritten.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};

use crate::blob::{parse_hex, to_hex};

/// An Ed25519 public key naming a feed.
pub type FeedKey = [u8; 32];

#[derive(Clone)]
pub struct Identity {
    key: SigningKey,
}

/// Shows only the public key.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", to_hex(&self.feed_key()))
    }
}

impl Identity {
    /// A new identity from OS randomness.
    pub fn generate() -> io::Result<Self> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| io::Error::other(format!("no OS randomness: {e}")))?;
        Ok(Self::from_seed(seed))
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        Identity { key: SigningKey::from_bytes(&seed) }
    }

    pub fn feed_key(&self) -> FeedKey {
        self.key.verifying_key().to_bytes()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let seed = parse_hex(text.trim()).ok_or(format!("{}: not a Terse secret key file", path.display()))?;
        Ok(Self::from_seed(seed))
    }

    /// Generates an identity and writes it to `path`, which must not exist.
    /// The file is written in full under a temporary name and then linked
    /// into place, so `path` never holds a partial key and an existing file
    /// is never replaced.
    pub fn create(path: &Path) -> Result<Self, String> {
        let fail = |e: io::Error| format!("{}: {e}", path.display());
        if path.exists() {
            return Err(format!("{}: an identity already exists; not overwriting it", path.display()));
        }
        let identity = Self::generate().map_err(fail)?;
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        private_dir(dir).map_err(fail)?;
        let name = path.file_name().map_or("key".into(), |n| n.to_string_lossy().into_owned());
        let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        let written = (|| {
            let mut f = private_file(&temp)?;
            f.write_all(format!("{}\n", to_hex(&identity.key.to_bytes())).as_bytes())?;
            f.sync_all()?;
            fs::hard_link(&temp, path)
        })();
        let _ = fs::remove_file(&temp);
        match written {
            Ok(()) => Ok(identity),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                Err(format!("{}: an identity already exists; not overwriting it", path.display()))
            }
            Err(e) => Err(fail(e)),
        }
    }
}

/// Whether `key` is a valid feed key (NETWORK 4.1): the canonical encoding
/// of a curve point (RFC 8032 decoding: y below p, no sign bit on x = 0;
/// equivalently, the decoded point encodes back to the same bytes) that is
/// not of small order. A key with a small-order component but of large
/// order itself is valid.
pub fn valid_feed_key(key: &FeedKey) -> bool {
    curve25519_dalek::edwards::CompressedEdwardsY(*key)
        .decompress()
        .is_some_and(|p| p.compress().to_bytes() == *key && !p.is_small_order())
}

#[cfg(unix)]
fn private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// Creates `dir` (owner-only on Unix) if it does not exist.
pub(crate) fn private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_file_is_private_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/identity.key");
        let id = Identity::create(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(Identity::load(&path).unwrap().feed_key(), id.feed_key());
        let before = fs::read(&path).unwrap();
        assert!(Identity::create(&path).unwrap_err().contains("already exists"));
        assert_eq!(fs::read(&path).unwrap(), before);
        // Only the key file is left: no temporary files.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert!(!format!("{id:?}").contains(&to_hex(&id.key.to_bytes())));
    }

    #[test]
    fn feed_keys() {
        let key = Identity::from_seed([7; 32]).feed_key();
        assert!(valid_feed_key(&key));
        assert!(!valid_feed_key(&[0; 32]), "small-order point");
    }
}
