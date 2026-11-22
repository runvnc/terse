//! Where the programs keep the user's files when not told otherwise: the
//! `terse` folder in the user's cache, data or config folder. Every
//! default location of every program comes from [`user_dir`]:
//!
//! | Kind | Linux, other Unix | macOS | Windows |
//! |---|---|---|---|
//! | [`Kind::Cache`] | `$XDG_CACHE_HOME/terse`, else `~/.cache/terse` | `~/Library/Caches/terse` | `%LOCALAPPDATA%\terse` |
//! | [`Kind::Data`] | `$XDG_DATA_HOME/terse`, else `~/.local/share/terse` | `~/Library/Application Support/terse/data` | `%LOCALAPPDATA%\terse` |
//! | [`Kind::Config`] | `$XDG_CONFIG_HOME/terse`, else `~/.config/terse` | `~/Library/Application Support/terse` | `%APPDATA%\terse` |
//!
//! The XDG variable, when set to an absolute path, is used on every
//! system, macOS and Windows included (`$XDG_DATA_HOME/terse`, with no
//! `data` below it): it is how a test or a second profile keeps away from
//! the user's own folders. A relative one is ignored (as the XDG
//! specification says). On Windows, without `%LOCALAPPDATA%` or
//! `%APPDATA%`, the folders they normally name under `%USERPROFILE%`
//! (`AppData\Local`, `AppData\Roaming`).
//!
//! What goes where: the cache holds the blob store (`blobs`) and the
//! hardware check's result (`hwcheck-<build>.txt`); the data folder the
//! performance record (`perf`), the snapshots (`snapshots`), the records
//! (`records`), the local relay's store (`relay`); the config folder the
//! identity (`identity.key`), the subscriptions (`subscriptions`), the
//! relays (`relays`, or an older `relay` file), the nicknames, the fonts
//! (`fonts`), the look (`look`), the sound (`sound`) and the drivers
//! (`drivers`) settings. On Windows the cache and the data share one
//! folder; none of their names are the same. On macOS the configuration
//! and the data share `Application Support/terse`, and the config's older
//! `relay` file and the data's `relay` store would collide: the data is in
//! its subfolder `data`.

use std::ffi::OsString;
use std::path::PathBuf;

/// Which of the user's folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cache,
    Data,
    Config,
}

/// Which system's folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    /// Linux and every other Unix but macOS: the XDG folders.
    Unix,
    MacOs,
    Windows,
}

impl System {
    /// The system this program was built for.
    pub fn this() -> System {
        if cfg!(windows) {
            System::Windows
        } else if cfg!(target_os = "macos") {
            System::MacOs
        } else {
            System::Unix
        }
    }
}

/// The `terse` folder of `kind` for this user (`None`: no home folder can
/// be found).
pub fn user_dir(kind: Kind) -> Option<PathBuf> {
    resolve(kind, System::this(), |v| std::env::var_os(v))
}

/// [`user_dir`] for a system and its environment (`var`): pure, for
/// testing every system on any.
pub fn resolve(kind: Kind, system: System, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let (xdg, home_sub, win, win_sub) = match kind {
        Kind::Cache => ("XDG_CACHE_HOME", ".cache", "LOCALAPPDATA", r"AppData\Local"),
        Kind::Data => ("XDG_DATA_HOME", ".local/share", "LOCALAPPDATA", r"AppData\Local"),
        Kind::Config => ("XDG_CONFIG_HOME", ".config", "APPDATA", r"AppData\Roaming"),
    };
    if let Some(p) = absolute(xdg) {
        return Some(p.join("terse"));
    }
    match system {
        System::Windows => Some(absolute(win).or_else(|| Some(absolute("USERPROFILE")?.join(win_sub)))?.join("terse")),
        System::Unix => Some(absolute("HOME")?.join(home_sub).join("terse")),
        System::MacOs => {
            let library = absolute("HOME")?.join("Library");
            Some(match kind {
                Kind::Cache => library.join("Caches").join("terse"),
                Kind::Config => library.join("Application Support").join("terse"),
                // Beside the configuration, in a folder of its own: the
                // config's older `relay` file and the data's `relay`
                // store share a name.
                Kind::Data => library.join("Application Support").join("terse").join("data"),
            })
        }
    }
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
        assert_eq!(resolve(Kind::Cache, System::Unix, &e), Some(PathBuf::from(&home).join(".cache").join("terse")));
        assert_eq!(resolve(Kind::Data, System::Unix, &e), Some(PathBuf::from(&home).join(".local/share").join("terse")));
        assert_eq!(resolve(Kind::Config, System::Unix, &e), Some(PathBuf::from(&home).join(".config").join("terse")));
        let x = abs("/x");
        let e = env(&[("HOME", &home), ("XDG_CACHE_HOME", &x), ("XDG_DATA_HOME", "relative"), ("APPDATA", &abs("/w"))]);
        assert_eq!(resolve(Kind::Cache, System::Unix, &e), Some(PathBuf::from(&x).join("terse")));
        // A relative XDG variable is ignored; a Windows variable is not
        // looked at elsewhere.
        assert_eq!(resolve(Kind::Data, System::Unix, &e), Some(PathBuf::from(&home).join(".local/share").join("terse")));
        assert_eq!(resolve(Kind::Config, System::Unix, &e), Some(PathBuf::from(&home).join(".config").join("terse")));
        assert_eq!(resolve(Kind::Config, System::Unix, env(&[])), None);
        assert_eq!(resolve(Kind::Config, System::Unix, env(&[("HOME", "relative")])), None);
    }

    #[test]
    fn windows_defaults() {
        let (local, roaming, profile) = (abs("/Users/a/AppData/Local"), abs("/Users/a/AppData/Roaming"), abs("/Users/a"));
        let e = env(&[("LOCALAPPDATA", &local), ("APPDATA", &roaming), ("USERPROFILE", &profile), ("HOME", &abs("/h"))]);
        assert_eq!(resolve(Kind::Cache, System::Windows, &e), Some(PathBuf::from(&local).join("terse")));
        assert_eq!(resolve(Kind::Data, System::Windows, &e), Some(PathBuf::from(&local).join("terse")));
        assert_eq!(resolve(Kind::Config, System::Windows, &e), Some(PathBuf::from(&roaming).join("terse")));
        // The XDG variables win on Windows too (tests, a second profile).
        let x = abs("/x");
        let e2 = env(&[("LOCALAPPDATA", &local), ("XDG_DATA_HOME", &x)]);
        assert_eq!(resolve(Kind::Data, System::Windows, &e2), Some(PathBuf::from(&x).join("terse")));
        // Without the AppData variables: the folders under the profile.
        let e3 = env(&[("USERPROFILE", &profile)]);
        assert_eq!(resolve(Kind::Cache, System::Windows, &e3), Some(PathBuf::from(&profile).join(r"AppData\Local").join("terse")));
        assert_eq!(resolve(Kind::Config, System::Windows, &e3), Some(PathBuf::from(&profile).join(r"AppData\Roaming").join("terse")));
        assert_eq!(resolve(Kind::Config, System::Windows, env(&[("HOME", &abs("/h"))])), None);
    }

    #[test]
    fn macos_defaults() {
        let home = abs("/Users/a");
        let e = env(&[("HOME", &home), ("APPDATA", &abs("/w"))]);
        let support = PathBuf::from(&home).join("Library").join("Application Support").join("terse");
        assert_eq!(resolve(Kind::Cache, System::MacOs, &e), Some(PathBuf::from(&home).join("Library").join("Caches").join("terse")));
        assert_eq!(resolve(Kind::Config, System::MacOs, &e), Some(support.clone()));
        assert_eq!(resolve(Kind::Data, System::MacOs, &e), Some(support.join("data")));
        // An absolute XDG variable wins here too, with nothing below it; a
        // relative one is ignored.
        let x = abs("/x");
        let e = env(&[("HOME", &home), ("XDG_DATA_HOME", &x), ("XDG_CONFIG_HOME", "relative")]);
        assert_eq!(resolve(Kind::Data, System::MacOs, &e), Some(PathBuf::from(&x).join("terse")));
        assert_eq!(resolve(Kind::Config, System::MacOs, &e), Some(support));
        assert_eq!(resolve(Kind::Config, System::MacOs, env(&[])), None);
    }

    /// The names the programs keep in the configuration folder and in the
    /// data folder: on macOS the data is below the configuration, and no
    /// name of the configuration is `data`.
    #[test]
    fn config_and_data_names_do_not_collide_on_macos() {
        let config = ["identity.key", "subscriptions", "relays", "relay", "nicknames", "fonts", "look", "sound", "drivers"];
        let data = ["perf", "records", "snapshots", "relay"];
        assert!(!config.contains(&"data"));
        // Were the data beside the configuration, `relay` would be both.
        assert!(data.iter().any(|d| config.contains(d)));
    }

    #[test]
    fn this_system() {
        // Whatever this computer's environment, a folder named terse (on
        // macOS the data in its folder data), or none at all.
        for k in [Kind::Cache, Kind::Data, Kind::Config] {
            if let Some(d) = user_dir(k) {
                assert!(d.is_absolute() && (d.ends_with("terse") || d.ends_with("terse/data")), "{d:?}");
            }
        }
    }
}
