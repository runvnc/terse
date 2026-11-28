//! The content of the website's "try it in your browser" page (`try.html`,
//! in the manual's `site-files/`, with `try.js` and `terse-web.wasm`, built
//! by `scripts/build-web` from crates/terse-web): `try-content.bin`, the
//! built-in shell and programs this `terse` carries, Low Bell, the
//! programs' data and the manual's articles, in terse-web's bundle format,
//! compressed with zlib (the page reads it through the browser's
//! `DecompressionStream("deflate")`). Written by `terse manual site`
//! whenever the manual's site files have `try.html`.

use crate::builtin;
use crate::manual::Built;

/// The page's content file.
pub const CONTENT: &str = "try-content.bin";

/// Low Bell, which the page offers besides the built-in programs (as
/// `terse-host --builtin` would).
const LOWBELL: &[u8] = include_bytes!("../../../programs/lowbell/lowbell.terse");

/// The bundle of `built` (the manual as the site shows it), uncompressed.
pub fn bundle(built: &[Built]) -> Vec<u8> {
    use terse_web::bundle::{BLOB, MANUAL_INDEX, PROGRAM, SHELL, encode};
    let mut entries: Vec<(u8, &[u8])> = vec![(SHELL, builtin::SHELL)];
    entries.extend(builtin::PROGRAMS.iter().map(|p| (PROGRAM, *p)));
    entries.push((PROGRAM, LOWBELL));
    entries.extend(builtin::DATA.iter().map(|d| (BLOB, *d)));
    for b in built {
        entries.push((if b.name == "index" { MANUAL_INDEX } else { BLOB }, b.text.as_bytes()));
    }
    encode(&entries)
}

/// `try-content.bin`: the bundle, compressed (zlib).
pub fn content(built: &[Built]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(&bundle(built), 9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_holds_the_programs_and_the_manual() {
        let built = crate::manual::build(&[("index".into(), "# Index\n\nHello.\n".into())]).unwrap();
        let b = bundle(&built);
        let entries = terse_web::bundle::decode(&b).unwrap();
        assert_eq!(entries[0], (terse_web::bundle::SHELL, builtin::SHELL.to_vec()));
        let programs = entries.iter().filter(|e| e.0 == terse_web::bundle::PROGRAM).count();
        assert_eq!(programs, builtin::PROGRAMS.len() + 1);
        assert!(entries.contains(&(terse_web::bundle::MANUAL_INDEX, built[0].text.as_bytes().to_vec())));
        let z = content(&built);
        assert_eq!(miniz_oxide::inflate::decompress_to_vec_zlib(&z).unwrap(), b);
        assert_eq!(content(&built), z, "the same bytes every time");
    }
}
