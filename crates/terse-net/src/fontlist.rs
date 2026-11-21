//! The user's font settings: `<config>/fonts` (README "Fonts"), which
//! `terse fonts` writes and `terse-host` reads once, when it starts. A
//! host draws all text with three built-in fonts (SPEC §terminal.text);
//! this file may replace each of them with a font file, and adds extra
//! fonts for characters the built-in ones lack, in the order listed.
//!
//! The format, one setting per line (blank lines and lines starting with
//! `#` are ignored):
//!
//! ```text
//! mono <file>    the terminal's and the page's monospace font
//! sans <file>    the page's text
//! bold <file>    the page's bold text
//! extra <file>   one more font, after the built-in ones (repeatable)
//! ```

use std::path::{Path, PathBuf};

/// The settings file in the config directory.
pub const FONTS_FILE: &str = "fonts";

/// What a font file is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Mono,
    Sans,
    Bold,
    Extra,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Mono => "mono",
            Role::Sans => "sans",
            Role::Bold => "bold",
            Role::Extra => "extra",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        [Role::Mono, Role::Sans, Role::Bold, Role::Extra].into_iter().find(|r| r.name() == s)
    }

    /// The built-in font a replacement stands in for.
    pub fn builtin(self) -> &'static str {
        match self {
            Role::Mono => "DejaVu Sans Mono",
            Role::Sans => "DejaVu Sans",
            Role::Bold => "DejaVu Sans Bold",
            Role::Extra => "",
        }
    }
}

/// The settings: replacements of the built-in fonts and the extra fonts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontSettings {
    pub mono: Option<PathBuf>,
    pub sans: Option<PathBuf>,
    pub bold: Option<PathBuf>,
    pub extra: Vec<PathBuf>,
}

impl FontSettings {
    /// Every file named, with its role: the replacements, then the extras
    /// in order.
    pub fn files(&self) -> Vec<(Role, &Path)> {
        let mut out = Vec::new();
        for (role, p) in [(Role::Mono, &self.mono), (Role::Sans, &self.sans), (Role::Bold, &self.bold)] {
            if let Some(p) = p {
                out.push((role, p.as_path()));
            }
        }
        out.extend(self.extra.iter().map(|p| (Role::Extra, p.as_path())));
        out
    }

    pub fn replacement_mut(&mut self, role: Role) -> Option<&mut Option<PathBuf>> {
        match role {
            Role::Mono => Some(&mut self.mono),
            Role::Sans => Some(&mut self.sans),
            Role::Bold => Some(&mut self.bold),
            Role::Extra => None,
        }
    }
}

/// Reads `<config>/fonts`; no file: no settings.
pub fn read(config: &Path) -> Result<FontSettings, String> {
    let path = config.join(FONTS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FontSettings::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    parse(&text).map_err(|(line, e)| format!("{}:{line}: {e}", path.display()))
}

/// Parses the settings; an error names its line (from 1).
pub fn parse(text: &str) -> Result<FontSettings, (usize, String)> {
    let mut s = FontSettings::default();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (word, file) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let file = file.trim();
        let role = Role::parse(word).ok_or((i + 1, format!("not mono, sans, bold or extra: {word}")))?;
        if file.is_empty() {
            return Err((i + 1, format!("{word} needs a font file")));
        }
        let file = PathBuf::from(file);
        if let Some(slot) = s.replacement_mut(role) {
            *slot = Some(file);
        } else if !s.extra.contains(&file) {
            s.extra.push(file);
        }
    }
    Ok(s)
}

/// The settings as the file holds them.
pub fn format(s: &FontSettings) -> String {
    let mut out = String::from("# Fonts of terse-host, read when it starts (README \"Fonts\"); `terse fonts` changes this file.\n");
    for (role, p) in s.files() {
        out.push_str(&format!("{} {}\n", role.name(), p.display()));
    }
    out
}

/// Writes `<config>/fonts`.
pub fn write(config: &Path, s: &FontSettings) -> Result<PathBuf, String> {
    std::fs::create_dir_all(config).map_err(|e| format!("{}: {e}", config.display()))?;
    let path = config.join(FONTS_FILE);
    std::fs::write(&path, format(s)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// What a font file holds, as far as the host cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// The family name of its first face.
    pub family: String,
    pub faces: u32,
    /// Characters its first face maps to glyphs.
    pub chars: u32,
}

/// Checks font file bytes for `role`: they must be a font this host reads
/// (TrueType or OpenType, or a collection, whose first face is used in the
/// terminal); a replacement must be a text font, with every character of
/// printable ASCII; the terminal's (mono) must be monospaced, every one of
/// those characters as wide as the others, so that it fits the cell grid.
pub fn check(role: Role, bytes: &[u8]) -> Result<FontInfo, String> {
    let faces = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("not a font file this host reads ({e})"))?;
    let family = face
        .names()
        .into_iter()
        .filter(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
        .find_map(|n| n.to_string())
        .unwrap_or_else(|| "(no name)".into());
    let mut chars = 0u32;
    if let Some(cmap) = face.tables().cmap {
        let mut seen = std::collections::HashSet::new();
        for sub in cmap.subtables.into_iter().filter(|s| s.is_unicode()) {
            sub.codepoints(|c| {
                if sub.glyph_index(c).is_some_and(|g| g.0 != 0) {
                    seen.insert(c);
                }
            });
        }
        chars = seen.len() as u32;
    }
    if chars == 0 {
        return Err("the font maps no characters".into());
    }
    if role != Role::Extra {
        let missing: String = (' '..='~').filter(|&c| face.glyph_index(c).is_none()).collect();
        if !missing.is_empty() {
            return Err(format!(
                "not a text font: it lacks {missing:?} of printable ASCII (`terse fonts add` adds a font for other characters)"
            ));
        }
    }
    if role == Role::Mono {
        let advance = |c: char| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g));
        let zero = advance('0');
        if let Some(c) = (' '..='~').find(|&c| advance(c) != zero) {
            return Err(format!("not monospaced: {c:?} is not as wide as '0', so it cannot fill the terminal's cells"));
        }
    }
    Ok(FontInfo { family, faces, chars })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let s = parse("# x\n\nmono /a/m.ttf\nextra /b/cjk.ttc\nsans  /c/s.otf \nextra /d/e.ttf\nextra /b/cjk.ttc\n").unwrap();
        assert_eq!(s.mono.as_deref(), Some(Path::new("/a/m.ttf")));
        assert_eq!(s.sans.as_deref(), Some(Path::new("/c/s.otf")));
        assert_eq!(s.bold, None);
        assert_eq!(s.extra, vec![PathBuf::from("/b/cjk.ttc"), PathBuf::from("/d/e.ttf")]);
        assert_eq!(parse(&format(&s)).unwrap(), s);
        assert_eq!(parse("serif /x").unwrap_err().0, 1);
        assert_eq!(parse("\nmono").unwrap_err().0, 2);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()).unwrap(), FontSettings::default());
        write(dir.path(), &s).unwrap();
        assert_eq!(read(dir.path()).unwrap(), s);
    }

    #[test]
    fn checks() {
        assert!(check(Role::Extra, b"not a font").is_err());
    }
}
