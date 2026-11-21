//! The fonts the host draws text with (SPEC §terminal.text, §page.layout):
//! three built into the program (DejaVu Sans Mono, Sans and Sans Bold
//! 2.37, `fonts/README`), each of which the user may replace with a font
//! file, and extra fonts the user lists for characters they lack, from
//! `<config>/fonts` (`terse_net::fontlist`).
//!
//! Every file is opened here, once, when the host starts: it is
//! memory-mapped (so a 20 MB collection costs nothing until its glyphs are
//! drawn) and checked. After that no file is opened and no directory is
//! read for text (SPEC §responsiveness item 5, PERFORMANCE.md R11): the
//! glyph code only ever sees [`FontData`]. There is no system font scan
//! and no fallback search beyond these fonts.
//!
//! The terminal uses the monospace font and the extras ([`TerminalFonts`]),
//! the page all of them ([`PageFonts`]); the two are built separately so
//! that a program that draws only the terminal holds only the monospace
//! font.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use terse_net::fontlist::{self, FontSettings, Role};

static MONO: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");
static SANS: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
static BOLD: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold.ttf");

/// The bytes of one font: built in, or a file mapped when the host started.
#[derive(Clone)]
pub struct FontData {
    /// What to call it: the built-in font's name, or the file's path.
    pub label: String,
    pub bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
}

impl FontData {
    fn builtin(name: &str, bytes: &'static [u8]) -> Self {
        FontData { label: format!("{name} 2.37 (built in)"), bytes: Arc::new(bytes) }
    }

    pub fn data(&self) -> &[u8] {
        (*self.bytes).as_ref()
    }
}

impl std::fmt::Debug for FontData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontData({}, {} bytes)", self.label, self.data().len())
    }
}

struct Mapped(memmap2::Mmap);

impl AsRef<[u8]> for Mapped {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Opens and checks `path` for `role`: mapped, not read.
fn open(role: Role, path: &Path) -> Result<FontData, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: the map is only read; a font file changed while the host runs
    // could show wrong glyphs, never anything worse than a garbled one, since
    // every read of it is bounds-checked by the font parsers.
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;
    fontlist::check(role, &map).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(FontData { label: path.display().to_string(), bytes: Arc::new(Mapped(map)) })
}

/// The settings in `<config>/fonts`, with what went wrong reading them.
fn settings(config: Option<&Path>, notes: &mut Vec<String>) -> FontSettings {
    match config.map(fontlist::read) {
        Some(Ok(s)) => s,
        Some(Err(e)) => {
            notes.push(format!("{e}; using the built-in fonts"));
            FontSettings::default()
        }
        None => FontSettings::default(),
    }
}

/// The replacement for `role` if it is set and good, else nothing (with a
/// note saying why).
fn replacement(role: Role, path: &Option<PathBuf>, notes: &mut Vec<String>) -> Option<FontData> {
    let path = path.as_ref()?;
    open(role, path).map_err(|e| notes.push(format!("{e}; using the built-in {} instead", role.builtin()))).ok()
}

fn extras(s: &FontSettings, notes: &mut Vec<String>) -> Vec<FontData> {
    s.extra.iter().filter_map(|p| open(Role::Extra, p).map_err(|e| notes.push(format!("{e}; not used"))).ok()).collect()
}

/// The terminal's fonts, in the order a character is looked for: the
/// monospace font (the user's replacement, then the built-in one behind
/// it), then the extras in their order.
#[derive(Clone, Debug)]
pub struct TerminalFonts {
    pub mono: Vec<FontData>,
    pub extras: Vec<FontData>,
    /// Settings that could not be used, for the user.
    pub notes: Vec<String>,
}

impl TerminalFonts {
    /// The built-in font only.
    pub fn builtin() -> Self {
        TerminalFonts { mono: vec![FontData::builtin("DejaVu Sans Mono", MONO)], extras: Vec::new(), notes: Vec::new() }
    }

    /// The fonts `<config>/fonts` asks for (`None`: the built-in font).
    pub fn load(config: Option<&Path>) -> Self {
        let mut t = Self::builtin();
        let s = settings(config, &mut t.notes);
        if let Some(f) = replacement(Role::Mono, &s.mono, &mut t.notes) {
            t.mono.insert(0, f);
        }
        t.extras = extras(&s, &mut t.notes);
        t
    }

    pub fn all(&self) -> impl Iterator<Item = &FontData> {
        self.mono.iter().chain(&self.extras)
    }
}

/// The page's fonts: for each of its kinds of text the user's replacement
/// (if any) and the built-in font, then the extras.
#[derive(Clone, Debug)]
pub struct PageFonts {
    pub sans: Vec<FontData>,
    pub bold: Vec<FontData>,
    pub mono: Vec<FontData>,
    pub extras: Vec<FontData>,
    pub notes: Vec<String>,
}

impl Default for PageFonts {
    fn default() -> Self {
        Self::builtin()
    }
}

impl PageFonts {
    /// The built-in fonts only.
    pub fn builtin() -> Self {
        PageFonts {
            sans: vec![FontData::builtin("DejaVu Sans", SANS)],
            bold: vec![FontData::builtin("DejaVu Sans Bold", BOLD)],
            mono: vec![FontData::builtin("DejaVu Sans Mono", MONO)],
            extras: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The fonts `<config>/fonts` asks for (`None`: the built-in fonts).
    pub fn load(config: Option<&Path>) -> Self {
        let mut p = Self::builtin();
        let s = settings(config, &mut p.notes);
        for (role, path) in [(Role::Sans, &s.sans), (Role::Bold, &s.bold), (Role::Mono, &s.mono)] {
            if let Some(f) = replacement(role, path, &mut p.notes) {
                let list = match role {
                    Role::Sans => &mut p.sans,
                    Role::Bold => &mut p.bold,
                    _ => &mut p.mono,
                };
                list.insert(0, f);
            }
        }
        p.extras = extras(&s, &mut p.notes);
        p
    }
}

/// The fonts in use, one line each, for `--version` and `terse fonts list`.
pub fn describe(t: &TerminalFonts, p: &PageFonts) -> String {
    let mut out = String::new();
    let mut line = |what: &str, list: &[FontData]| {
        out.push_str(&format!("{what:<9}{}\n", list.iter().map(|f| f.label.as_str()).collect::<Vec<_>>().join(", then ")));
    };
    line("terminal", &t.mono);
    line("sans", &p.sans);
    line("bold", &p.bold);
    line("mono", &p.mono);
    for f in &t.extras {
        out.push_str(&format!("extra    {}\n", f.label));
    }
    for n in t.notes.iter().chain(&p.notes) {
        out.push_str(&format!("note     {n}\n"));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A CJK font on this system to stand in for a user's extra font, if
    /// there is one.
    pub(crate) fn system_cjk() -> Option<PathBuf> {
        ["/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    }

    #[test]
    fn builtin_fonts_are_good() {
        assert!(fontlist::check(Role::Mono, MONO).is_ok());
        assert!(fontlist::check(Role::Sans, SANS).is_ok());
        assert!(fontlist::check(Role::Bold, BOLD).is_ok());
        assert!(fontlist::check(Role::Mono, SANS).unwrap_err().contains("not monospaced"));
    }

    /// Start-up opens exactly the listed files and nothing after: once
    /// loaded, the files can disappear and the fonts still draw.
    #[test]
    fn settings_are_loaded_once() {
        let config = tempfile::tempdir().unwrap();
        let fonts = tempfile::tempdir().unwrap();
        let mono = fonts.path().join("m.ttf");
        std::fs::write(&mono, MONO).unwrap();
        let sans_as_mono = fonts.path().join("s.ttf");
        std::fs::write(&sans_as_mono, SANS).unwrap();
        let s = FontSettings {
            mono: Some(sans_as_mono.clone()),
            sans: Some(mono.clone()),
            bold: Some(fonts.path().join("missing.ttf")),
            extra: vec![mono.clone()],
        };
        fontlist::write(config.path(), &s).unwrap();
        let t = TerminalFonts::load(Some(config.path()));
        assert_eq!(t.mono.len(), 1, "a proportional terminal font is refused");
        assert!(t.notes[0].contains("not monospaced"), "{:?}", t.notes);
        assert_eq!(t.extras.len(), 1);
        let p = PageFonts::load(Some(config.path()));
        assert_eq!((p.sans.len(), p.bold.len(), p.mono.len(), p.extras.len()), (2, 1, 1, 1));
        assert!(p.notes.iter().any(|n| n.contains("missing.ttf")), "{:?}", p.notes);
        drop(fonts);
        assert!(!mono.exists());
        // Still readable: the maps were made at start.
        assert!(fontlist::check(Role::Extra, t.extras[0].data()).is_ok());
        assert!(fontlist::check(Role::Sans, p.sans[0].data()).is_ok());
        assert_eq!(TerminalFonts::load(None).mono.len(), 1);
    }
}
