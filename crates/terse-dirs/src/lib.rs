//! Where the programs keep the user's files when not told otherwise: the
//! `terse` folder in the user's cache, data or config folder. Every
//! default location of every program comes from [`user_dir`]:
//!
//! | Kind | Linux, macOS, other Unix | Windows |
//! |---|---|---|
//! | [`Kind::Cache`] | `$XDG_CACHE_HOME/terse`, else `~/.cache/terse` | `%LOCALAPPDATA%\terse` |
//! | [`Kind::Data`] | `$XDG_DATA_HOME/terse`, else `~/.local/share/terse` | `%LOCALAPPDATA%\terse` |
//! | [`Kind::Config`] | `$XDG_CONFIG_HOME/terse`, else `~/.config/terse` | `%APPDATA%\terse` |
//!
//! The XDG variable, when set to an absolute path, is used on every
//! system, Windows included: it is how a test or a second profile keeps
//! away from the user's own folders. A relative one is ignored (as the XDG
//! specification says). On Windows, without `%LOCALAPPDATA%` or
//! `%APPDATA%`, the folders they normally name under `%USERPROFILE%`
//! (`AppData\Local`, `AppData\Roaming`). macOS uses the same folders as
//! Linux (not `~/Library`), so that one set of instructions serves both.
//!
//! What goes where: the cache holds the blob store (`blobs`) and the
//! hardware check's result (`hwcheck-<build>.txt`); the data folder the
//! performance record (`perf`), the snapshots, the records, the local
//! relay's store (`relay`); the config folder the identity, the
//! subscriptions, the look (`look`), the sound (`sound`) and the drivers
//! (`drivers`) settings. On Windows the cache and the data share one
//! folder; none of their names are the same.

use std::ffi::OsString;
use std::path::PathBuf;

/// Which of the user's folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cache,
    Data,
    Config,
}

/// The `terse` folder of `kind` for this user (`None`: no home folder can
/// be found).
pub fn user_dir(kind: Kind) -> Option<PathBuf> {
    resolve(kind, cfg!(windows), |v| std::env::var_os(v))
}

/// [`user_dir`] for a system (`windows`) and its environment (`var`):
/// pure, for testing every system on any.
pub fn resolve(kind: Kind, windows: bool, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let (xdg, home_sub, win, win_sub) = match kind {
        Kind::Cache => ("XDG_CACHE_HOME", ".cache", "LOCALAPPDATA", r"AppData\Local"),
        Kind::Data => ("XDG_DATA_HOME", ".local/share", "LOCALAPPDATA", r"AppData\Local"),
        Kind::Config => ("XDG_CONFIG_HOME", ".config", "APPDATA", r"AppData\Roaming"),
    };
    let base = match absolute(xdg) {
        Some(p) => p,
        None if windows => absolute(win).or_else(|| Some(absolute("USERPROFILE")?.join(win_sub)))?,
        None => absolute("HOME")?.join(home_sub),
    };
    Some(base.join("terse"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let m: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |k| m.get(k).cloned()
    }

    /// An absolute path on the system running the test (a Windows path is
    /// absolute only with a drive).
    fn abs(unix: &str) -> String {
        if cfg!(windows) { format!(r"C:{}", unix.replace('/', r"\")) } else { unix.to_string() }
    }

    #[test]
    fn unix_defaults_and_xdg() {
        let home = abs("/home/a");
        let e = env(&[("HOME", &home)]);
        assert_eq!(resolve(Kind::Cache, false, &e), Some(PathBuf::from(&home).join(".cache").join("terse")));
        assert_eq!(resolve(Kind::Data, false, &e), Some(PathBuf::from(&home).join(".local/share").join("terse")));
        assert_eq!(resolve(Kind::Config, false, &e), Some(PathBuf::from(&home).join(".config").join("terse")));
        let x = abs("/x");
        let e = env(&[("HOME", &home), ("XDG_CACHE_HOME", &x), ("XDG_DATA_HOME", "relative"), ("APPDATA", &abs("/w"))]);
        assert_eq!(resolve(Kind::Cache, false, &e), Some(PathBuf::from(&x).join("terse")));
        // A relative XDG variable is ignored; a Windows variable is not
        // looked at elsewhere.
        assert_eq!(resolve(Kind::Data, false, &e), Some(PathBuf::from(&home).join(".local/share").join("terse")));
        assert_eq!(resolve(Kind::Config, false, &e), Some(PathBuf::from(&home).join(".config").join("terse")));
        assert_eq!(resolve(Kind::Config, false, env(&[])), None);
        assert_eq!(resolve(Kind::Config, false, env(&[("HOME", "relative")])), None);
    }

    #[test]
    fn windows_defaults() {
        let (local, roaming, profile) = (abs("/Users/a/AppData/Local"), abs("/Users/a/AppData/Roaming"), abs("/Users/a"));
        let e = env(&[("LOCALAPPDATA", &local), ("APPDATA", &roaming), ("USERPROFILE", &profile), ("HOME", &abs("/h"))]);
        assert_eq!(resolve(Kind::Cache, true, &e), Some(PathBuf::from(&local).join("terse")));
        assert_eq!(resolve(Kind::Data, true, &e), Some(PathBuf::from(&local).join("terse")));
        assert_eq!(resolve(Kind::Config, true, &e), Some(PathBuf::from(&roaming).join("terse")));
        // The XDG variables win on Windows too (tests, a second profile).
        let x = abs("/x");
        let e2 = env(&[("LOCALAPPDATA", &local), ("XDG_DATA_HOME", &x)]);
        assert_eq!(resolve(Kind::Data, true, &e2), Some(PathBuf::from(&x).join("terse")));
        // Without the AppData variables: the folders under the profile.
        let e3 = env(&[("USERPROFILE", &profile)]);
        assert_eq!(resolve(Kind::Cache, true, &e3), Some(PathBuf::from(&profile).join(r"AppData\Local").join("terse")));
        assert_eq!(resolve(Kind::Config, true, &e3), Some(PathBuf::from(&profile).join(r"AppData\Roaming").join("terse")));
        assert_eq!(resolve(Kind::Config, true, env(&[("HOME", &abs("/h"))])), None);
    }

    #[test]
    fn this_system() {
        // Whatever this computer's environment, a folder named terse, or
        // none at all.
        for k in [Kind::Cache, Kind::Data, Kind::Config] {
            if let Some(d) = user_dir(k) {
                assert!(d.is_absolute() && d.ends_with("terse"), "{d:?}");
            }
        }
    }
}
