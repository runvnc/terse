//! Named records (SPEC 9.6): small data a program keeps on this machine,
//! under its identity (a published program's feed key and name, a built-in
//! program's zero key and title) or, for a machine with no identity, its
//! exact image ID. Records never leave the host.
//!
//! Layout under the records directory (`<data>/records`):
//!
//! - `program/<publisher hex>/<name hex>/<record name hex>`
//! - `image/<image ID hex>/<record name hex>`
//!
//! A record is written to a temporary file in its owner's directory, synced
//! and renamed over the old one, and the directory is synced, so a save is
//! complete only once it is durable.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use terse_core::Machine;
use terse_core::content::{MAX_RECORD, MAX_RECORD_NAME};

use crate::blob::{parse_hex, to_hex};

/// Most records one owner keeps.
pub const MAX_RECORDS: usize = 256;
/// Most bytes of record data one owner keeps.
pub const MAX_TOTAL: usize = 1_048_576;

/// Whose records: a program's identity, or an image with none.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    Program { publisher: [u8; 32], name: Vec<u8> },
    Image([u8; 32]),
}

impl Owner {
    /// The owner of a machine's records: its identity if it has one, else
    /// its image ID (SPEC 9.6).
    pub fn of(machine: &Machine) -> Owner {
        match machine.identity() {
            Some(id) => Owner::Program { publisher: id.publisher, name: id.name.clone() },
            None => Owner::Image(*machine.image_id()),
        }
    }

    /// Parses `program:<key hex>:<name>`, `builtin:<title>` or
    /// `image:<id hex>` (as `terse records` takes them).
    pub fn parse(s: &str) -> Option<Owner> {
        if let Some(rest) = s.strip_prefix("program:") {
            let (key, name) = rest.split_once(':')?;
            let publisher = parse_hex(key)?;
            return (!name.is_empty() && name.len() <= 48)
                .then(|| Owner::Program { publisher, name: name.as_bytes().to_vec() });
        }
        if let Some(title) = s.strip_prefix("builtin:") {
            return (!title.is_empty() && title.len() <= 48)
                .then(|| Owner::Program { publisher: [0; 32], name: title.as_bytes().to_vec() });
        }
        s.strip_prefix("image:").and_then(parse_hex).map(Owner::Image)
    }

    /// The form [`Owner::parse`] reads.
    pub fn describe(&self) -> String {
        match self {
            Owner::Program { publisher, name } if *publisher == [0; 32] => {
                format!("builtin:{}", String::from_utf8_lossy(name))
            }
            Owner::Program { publisher, name } => format!("program:{}:{}", to_hex(publisher), String::from_utf8_lossy(name)),
            Owner::Image(id) => format!("image:{}", to_hex(id)),
        }
    }

    pub(crate) fn dir(&self, root: &Path) -> PathBuf {
        match self {
            Owner::Program { publisher, name } => root.join("program").join(to_hex(publisher)).join(hex(name)),
            Owner::Image(id) => root.join("image").join(to_hex(id)),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || s.is_empty() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// What a save did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    Done,
    /// It would have exceeded 256 records or 1,048,576 bytes; nothing
    /// changed.
    NoRoom,
}

/// The records directory.
#[derive(Clone, Debug)]
pub struct Records {
    root: PathBuf,
}

impl Records {
    /// Records kept under `root` (created when first needed).
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Records { root: root.into() }
    }

    /// `<data>/records`.
    pub fn in_data_dir(data: &Path) -> Self {
        Self::open(data.join("records"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// An owner's records: name and size, in increasing name order (byte
    /// order).
    pub fn list(&self, owner: &Owner) -> io::Result<Vec<(Vec<u8>, u32)>> {
        let dir = owner.dir(&self.root);
        let items = match fs::read_dir(&dir) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for item in items {
            let item = item?;
            let Some(name) = item.file_name().to_str().and_then(unhex) else { continue };
            if name.len() <= MAX_RECORD_NAME && item.file_type()?.is_file() {
                out.push((name, item.metadata()?.len() as u32));
            }
        }
        out.sort();
        Ok(out)
    }

    /// A record's bytes, if it exists.
    pub fn load(&self, owner: &Owner, name: &[u8]) -> io::Result<Option<Vec<u8>>> {
        match fs::read(owner.dir(&self.root).join(hex(name))) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Saves (replaces) a record, durably, unless the owner's limits would
    /// be exceeded.
    pub fn save(&self, owner: &Owner, name: &[u8], data: &[u8]) -> io::Result<Saved> {
        // A machine never gets here with more than MAX_RECORD bytes: the
        // content device rejects such a save at submit with status 04h
        // (SPEC 9.2, 9.6). This check guards the store's other callers,
        // and answers them as an over-limit save is answered (06h).
        if data.len() > MAX_RECORD {
            return Ok(Saved::NoRoom);
        }
        let list = self.list(owner)?;
        let old = list.iter().find(|(n, _)| n == name).map(|(_, size)| *size as usize);
        let count = list.len() + old.is_none() as usize;
        let total = list.iter().map(|(_, s)| *s as usize).sum::<usize>() - old.unwrap_or(0) + data.len();
        if count > MAX_RECORDS || total > MAX_TOTAL {
            return Ok(Saved::NoRoom);
        }
        let dir = owner.dir(&self.root);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".{}.tmp", hex(name)));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, dir.join(hex(name)))?;
        sync_dir(&dir)?;
        Ok(Saved::Done)
    }

    /// Deletes a record (done whether or not it existed).
    pub fn delete(&self, owner: &Owner, name: &[u8]) -> io::Result<()> {
        let dir = owner.dir(&self.root);
        match fs::remove_file(dir.join(hex(name))) {
            Ok(()) => sync_dir(&dir),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Every owner that has records, in order.
    pub fn owners(&self) -> io::Result<Vec<Owner>> {
        let mut out = Vec::new();
        for (key_dir, _) in subdirs(&self.root.join("program"))? {
            let Some(publisher) = parse_hex(&key_dir) else { continue };
            for (name_dir, _) in subdirs(&self.root.join("program").join(&key_dir))? {
                if let Some(name) = unhex(&name_dir) {
                    out.push(Owner::Program { publisher, name });
                }
            }
        }
        for (id, _) in subdirs(&self.root.join("image"))? {
            if let Some(id) = parse_hex(&id) {
                out.push(Owner::Image(id));
            }
        }
        out.sort();
        // An owner whose records were all deleted keeps an empty directory.
        let mut kept = Vec::new();
        for o in out {
            if !self.list(&o)?.is_empty() {
                kept.push(o);
            }
        }
        Ok(kept)
    }
}

fn subdirs(dir: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let items = match fs::read_dir(dir) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for item in items {
        let item = item?;
        if item.file_type()?.is_dir()
            && let Some(name) = item.file_name().to_str()
        {
            out.push((name.to_string(), item.path()));
        }
    }
    out.sort();
    Ok(out)
}

/// Makes a rename in `dir` durable (where the platform allows it).
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// The records request's answer (SPEC 9.6): `(name, size)` records of 36
/// bytes from index `offset`, as many as fit in `length`, and how many
/// there are.
pub fn list_records(list: &[(Vec<u8>, u32)], offset: u32, length: u16) -> (u32, Vec<u8>) {
    let records = list.iter().skip(offset as usize).map(|(name, size)| {
        let mut r = [0u8; 36];
        r[..name.len()].copy_from_slice(name);
        r[32..].copy_from_slice(&size.to_le_bytes());
        r.to_vec()
    });
    (list.len() as u32, crate::timeline::fill(records, length as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sparks() -> Owner {
        Owner::Program { publisher: [3; 32], name: b"sparks".to_vec() }
    }

    #[test]
    fn save_load_replace_delete_list() {
        let dir = tempfile::tempdir().unwrap();
        let r = Records::open(dir.path());
        let o = sparks();
        assert_eq!(r.list(&o).unwrap(), []);
        assert_eq!(r.load(&o, b"high-score").unwrap(), None);
        assert_eq!(r.save(&o, b"high-score", b"0042").unwrap(), Saved::Done);
        assert_eq!(r.save(&o, b"a/b", b"").unwrap(), Saved::Done, "any name, an empty record");
        assert_eq!(r.load(&o, b"high-score").unwrap().as_deref(), Some(&b"0042"[..]));
        assert_eq!(r.save(&o, b"high-score", b"0100!").unwrap(), Saved::Done);
        assert_eq!(r.load(&o, b"high-score").unwrap().as_deref(), Some(&b"0100!"[..]));
        assert_eq!(r.list(&o).unwrap(), [(b"a/b".to_vec(), 0), (b"high-score".to_vec(), 5)]);
        r.delete(&o, b"a/b").unwrap();
        r.delete(&o, b"never").unwrap();
        assert_eq!(r.list(&o).unwrap(), [(b"high-score".to_vec(), 5)]);
        // No temporary files left; the files are where the layout says.
        let owner_dir = dir.path().join("program").join(to_hex(&[3; 32])).join(hex(b"sparks"));
        let names: Vec<String> =
            fs::read_dir(&owner_dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, [hex(b"high-score")]);
        assert_eq!(r.owners().unwrap(), [o.clone()]);
        // Another instance sees the same records.
        assert_eq!(Records::open(dir.path()).load(&o, b"high-score").unwrap().as_deref(), Some(&b"0100!"[..]));
    }

    #[test]
    fn owners_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let r = Records::open(dir.path());
        let a = sparks();
        let b = Owner::Program { publisher: [4; 32], name: b"sparks".to_vec() };
        let c = Owner::Program { publisher: [3; 32], name: b"sparks2".to_vec() };
        let builtin = Owner::Program { publisher: [0; 32], name: b"sparks".to_vec() };
        let image = Owner::Image([3; 32]);
        for (i, o) in [&a, &b, &c, &builtin, &image].into_iter().enumerate() {
            r.save(o, b"k", &[i as u8]).unwrap();
        }
        for (i, o) in [&a, &b, &c, &builtin, &image].into_iter().enumerate() {
            assert_eq!(r.load(o, b"k").unwrap(), Some(vec![i as u8]), "{o:?}");
        }
        assert_eq!(r.owners().unwrap().len(), 5);
        for o in [&a, &b, &c, &builtin, &image] {
            assert_eq!(Owner::parse(&o.describe()).as_ref(), Some(o));
        }
    }

    #[test]
    fn limits() {
        let dir = tempfile::tempdir().unwrap();
        let r = Records::open(dir.path());
        let o = sparks();
        // 256 records; the 257th is refused, replacing one is not.
        for i in 0..MAX_RECORDS {
            assert_eq!(r.save(&o, format!("r{i:03}").as_bytes(), b"x").unwrap(), Saved::Done);
        }
        assert_eq!(r.save(&o, b"one more", b"x").unwrap(), Saved::NoRoom);
        assert_eq!(r.load(&o, b"one more").unwrap(), None);
        assert_eq!(r.save(&o, b"r000", b"yy").unwrap(), Saved::Done);
        // 1,048,576 bytes: 16 records of 65,536 fill it (with 240 of 1 byte
        // deleted first); a byte more is refused, and nothing changes.
        for i in 0..MAX_RECORDS {
            r.delete(&o, format!("r{i:03}").as_bytes()).unwrap();
        }
        let big = vec![7u8; MAX_RECORD];
        for i in 0..16 {
            assert_eq!(r.save(&o, format!("big{i}").as_bytes(), &big).unwrap(), Saved::Done);
        }
        assert_eq!(r.save(&o, b"small", b"x").unwrap(), Saved::NoRoom);
        assert_eq!(r.save(&o, b"big0", &big[..MAX_RECORD - 1]).unwrap(), Saved::Done, "smaller fits");
        assert_eq!(r.save(&o, b"small", b"x").unwrap(), Saved::Done, "the byte freed");
        assert_eq!(r.save(&o, b"small", b"xx").unwrap(), Saved::NoRoom);
        assert_eq!(r.load(&o, b"small").unwrap(), Some(b"x".to_vec()));
        assert_eq!(r.save(&o, b"huge", &vec![0; MAX_RECORD + 1]).unwrap(), Saved::NoRoom);
    }

    #[test]
    fn records_requests_list_names_and_sizes() {
        let list = vec![(b"a".to_vec(), 3), (b"bb".to_vec(), 70_000)];
        let (total, data) = list_records(&list, 0, 100);
        assert_eq!((total, data.len()), (2, 72));
        assert_eq!(&data[..4], b"a\0\0\0");
        assert_eq!(&data[32..36], &3u32.to_le_bytes());
        assert_eq!(&data[36..38], b"bb");
        assert_eq!(&data[68..72], &70_000u32.to_le_bytes());
        assert_eq!(list_records(&list, 1, 35), (2, Vec::new()));
        assert_eq!(list_records(&list, 1, 36).1.len(), 36);
    }
}
