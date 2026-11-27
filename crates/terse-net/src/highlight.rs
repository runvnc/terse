//! Syntax colouring of a fence's lines for the static websites
//! ([`crate::site_render`]): a small tokenizer, not a parser, good enough
//! for the languages the manual shows. It is a feature of the sites, not
//! of the article format: ARTICLE.md 2.4 ignores a fence's info string,
//! and a page that shows every fence in one colour is just as right.
//!
//! The language is the info string's first word ([`lang`]): `c`, `rust`,
//! `go`, `sh` (command lines), `article` (article markup), `drive` and
//! `events` (the scripts of `terse-host --drive` and `terse run
//! --events`); a word that is a file name is taken by its extension
//! (`c examples/x.c`, or only `examples/x.c`); `text` and an empty info
//! string are not coloured; any other word gets the generic colouring
//! (comments, strings, numbers). The same rules, with the same classes,
//! are in the reader (`programs/reader/reader.c`) and the Go reader
//! (`native/reader-go/internal/render/highlight.go`).
//!
//! Classes ([`Class`]): comment, string, number, keyword (also a shell
//! line's command, a script's command, an article's heading), type, and
//! meta (a C preprocessor word, a Rust attribute or macro, a shell
//! variable). Everything else is plain.

/// A language a fence can be coloured as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    C,
    Rust,
    Go,
    Shell,
    Article,
    Script,
    Generic,
}

/// What a run of a line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Plain,
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Meta,
}

impl Class {
    /// The CSS class of a run (none for plain text).
    pub fn css(self) -> Option<&'static str> {
        Some(match self {
            Class::Plain => return None,
            Class::Comment => "c",
            Class::String => "s",
            Class::Number => "n",
            Class::Keyword => "k",
            Class::Type => "t",
            Class::Meta => "m",
        })
    }
}

fn by_word(w: &str) -> Option<Option<Lang>> {
    Some(match w {
        "c" | "h" => Some(Lang::C),
        "rust" | "rs" => Some(Lang::Rust),
        "go" => Some(Lang::Go),
        "sh" | "shell" | "bash" | "fish" | "zsh" | "console" => Some(Lang::Shell),
        "article" => Some(Lang::Article),
        "drive" | "events" => Some(Lang::Script),
        "text" | "txt" | "plain" | "output" | "path" => None,
        _ => return None,
    })
}

/// The language of a fence by its info string (its text after the
/// backquotes): `None` if it is not coloured.
pub fn lang(info: &str) -> Option<Lang> {
    let mut words = info.split_ascii_whitespace();
    let first = words.next()?;
    if let Some(l) = by_word(&first.to_ascii_lowercase()) {
        return l;
    }
    // A file name: its extension.
    for w in info.split_ascii_whitespace() {
        if let Some((_, ext)) = w.rsplit_once('.').filter(|(b, e)| !b.is_empty() && !b.ends_with('/') && !e.contains('/')) {
            if let Some(l) = by_word(&ext.to_ascii_lowercase()) {
                return l;
            }
        }
    }
    Some(Lang::Generic)
}

const C_KEYWORDS: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "inline",
    "register", "restrict", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while",
    "true", "false", "NULL",
];
const C_TYPES: &[&str] = &["char", "double", "float", "int", "long", "short", "signed", "unsigned", "void", "bool", "_Bool"];
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "static", "struct",
    "super", "trait", "true", "type", "unsafe", "use", "where", "while",
];
const RUST_TYPES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32",
    "f64",
];
const GO_KEYWORDS: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto",
    "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var",
    "true", "false", "nil", "iota",
];
const GO_TYPES: &[&str] = &[
    "bool", "byte", "complex64", "complex128", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64",
    "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any",
];
const SH_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "until", "case", "esac", "function", "end",
    "begin", "set", "export",
];

/// The state a line leaves for the next.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Normal,
    /// Inside a `/* */` comment.
    Block,
    /// Inside a string that goes on to the next line: its closing byte
    /// (Rust "..." and Go `...`).
    Str(u8),
    /// A shell line ended with `\`: the next one continues the command.
    Continued,
    /// Shell: inside a here-document, until a line that is this word.
    Here(String),
    /// Article markup: inside a fence, until its closing line.
    Fence,
}

/// The coloured runs of a fence's lines: for each line, (class, byte
/// range) for every run that is not plain, in order, not overlapping.
pub fn highlight(lang: Lang, lines: &[&str]) -> Vec<Vec<(Class, std::ops::Range<usize>)>> {
    let mut state = State::Normal;
    lines.iter().map(|l| line(lang, l.as_bytes(), &mut state)).collect()
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

struct Out(Vec<(Class, std::ops::Range<usize>)>);

impl Out {
    fn push(&mut self, c: Class, a: usize, b: usize) {
        if c == Class::Plain || a >= b {
            return;
        }
        if let Some((last, r)) = self.0.last_mut()
            && *last == c
            && r.end == a
        {
            r.end = b;
            return;
        }
        self.0.push((c, a..b));
    }
}

/// The end of a string opened by `q` whose text starts at `i` (`\`
/// escapes the next byte unless `raw`): the index after its closing quote,
/// or `None` if the line ends first.
fn string_end(s: &[u8], mut i: usize, q: u8, raw: bool) -> Option<usize> {
    while i < s.len() {
        if s[i] == b'\\' && !raw {
            i += 2;
        } else if s[i] == q {
            return Some(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

fn number_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && (is_ident(s[i]) || (s[i] == b'.' && i + 1 < s.len() && s[i + 1].is_ascii_digit())) {
        i += 1;
    }
    i
}

fn line(lang: Lang, s: &[u8], state: &mut State) -> Vec<(Class, std::ops::Range<usize>)> {
    let mut out = Out(Vec::new());
    match lang {
        Lang::C | Lang::Rust | Lang::Go => code(lang, s, state, &mut out),
        Lang::Shell => shell(s, state, &mut out),
        Lang::Article => article(s, state, &mut out),
        Lang::Script => script(s, &mut out),
        Lang::Generic => generic(s, &mut out),
    }
    out.0
}

/// C, Rust and Go.
fn code(lang: Lang, s: &[u8], state: &mut State, out: &mut Out) {
    let (keywords, types) = match lang {
        Lang::C => (C_KEYWORDS, C_TYPES),
        Lang::Rust => (RUST_KEYWORDS, RUST_TYPES),
        _ => (GO_KEYWORDS, GO_TYPES),
    };
    let mut i = 0;
    match *state {
        State::Block => match find(s, 0, b"*/") {
            Some(e) => {
                out.push(Class::Comment, 0, e + 2);
                i = e + 2;
                *state = State::Normal;
            }
            None => return out.push(Class::Comment, 0, s.len()),
        },
        State::Str(q) => match string_end(s, 0, q, q == b'`') {
            Some(e) => {
                out.push(Class::String, 0, e);
                i = e;
                *state = State::Normal;
            }
            None => return out.push(Class::String, 0, s.len()),
        },
        _ => *state = State::Normal,
    }
    // A C preprocessor line: its directive.
    let lead = s.iter().take_while(|b| b.is_ascii_whitespace()).count();
    if lang == Lang::C && i == 0 && s.get(lead) == Some(&b'#') {
        let mut e = lead + 1;
        while e < s.len() && (s[e] == b' ' || is_ident(s[e])) && !(s[e] == b' ' && e > lead + 1 && is_ident(s[e - 1])) {
            e += 1;
        }
        out.push(Class::Meta, lead, e);
        i = e;
        // #include <file>: the file is a string.
        let rest = &s[e..];
        let skip = rest.iter().take_while(|b| **b == b' ').count();
        if rest.get(skip) == Some(&b'<')
            && let Some(k) = rest.iter().position(|&b| b == b'>')
        {
            out.push(Class::String, e + skip, e + k + 1);
            i = e + k + 1;
        }
    }
    while i < s.len() {
        let b = s[i];
        let next = s.get(i + 1).copied().unwrap_or(0);
        if b == b'/' && next == b'/' {
            return out.push(Class::Comment, i, s.len());
        }
        if b == b'/' && next == b'*' {
            match find(s, i + 2, b"*/") {
                Some(e) => {
                    out.push(Class::Comment, i, e + 2);
                    i = e + 2;
                    continue;
                }
                None => {
                    *state = State::Block;
                    return out.push(Class::Comment, i, s.len());
                }
            }
        }
        // Rust: an attribute, #[...] or #![...], to its closing bracket.
        if lang == Lang::Rust && b == b'#' && (next == b'[' || (next == b'!' && s.get(i + 2) == Some(&b'['))) {
            let e = find(s, i, b"]").map_or(s.len(), |e| e + 1);
            out.push(Class::Meta, i, e);
            i = e;
            continue;
        }
        if b == b'"' || (b == b'`' && lang == Lang::Go) {
            let raw = b == b'`';
            match string_end(s, i + 1, b, raw) {
                Some(e) => {
                    out.push(Class::String, i, e);
                    i = e;
                }
                None => {
                    if lang != Lang::C {
                        *state = State::Str(b);
                    }
                    return out.push(Class::String, i, s.len());
                }
            }
            continue;
        }
        if b == b'\'' {
            // A character literal; in Rust a quote not closed within a
            // character is a lifetime (plain).
            let e = if next == b'\\' { string_end(s, i + 1, b'\'', false) } else { char_end(s, i + 1) };
            match e {
                Some(e) => {
                    out.push(Class::String, i, e);
                    i = e;
                }
                None if lang == Lang::Rust => i += 1,
                None => return out.push(Class::String, i, s.len()),
            }
            continue;
        }
        if b.is_ascii_digit() {
            let e = number_end(s, i);
            out.push(Class::Number, i, e);
            i = e;
            continue;
        }
        if is_ident_start(b) {
            let mut e = i;
            while e < s.len() && is_ident(s[e]) {
                e += 1;
            }
            let w = std::str::from_utf8(&s[i..e]).unwrap_or("");
            // Rust: r"..." and r#"..."# raw strings, b"..." byte strings.
            if lang == Lang::Rust && matches!(w, "r" | "b" | "br") && matches!(s.get(e), Some(b'"' | b'#')) {
                let hashes = s[e..].iter().take_while(|&&c| c == b'#').count();
                if s.get(e + hashes) == Some(&b'"') {
                    let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
                    let raw = w != "b";
                    let end = if raw { find(s, e + hashes + 1, &close).map(|k| k + close.len()) } else { string_end(s, e + 1, b'"', false) };
                    let end = end.unwrap_or(s.len());
                    out.push(Class::String, i, end);
                    i = end;
                    continue;
                }
            }
            let class = if keywords.contains(&w) {
                Class::Keyword
            } else if types.contains(&w)
                || (lang == Lang::C && w.ends_with("_t"))
                || (lang == Lang::Rust && b.is_ascii_uppercase() && s[i..e].iter().any(u8::is_ascii_lowercase))
            {
                Class::Type
            } else if lang == Lang::Rust && s.get(e) == Some(&b'!') && s.get(e + 1) != Some(&b'=') {
                e += 1;
                Class::Meta
            } else {
                Class::Plain
            };
            out.push(class, i, e);
            i = e;
            continue;
        }
        i += 1;
        // Skip the rest of a UTF-8 sequence.
        while i < s.len() && s[i] & 0xC0 == 0x80 {
            i += 1;
        }
    }
}

/// The end of a one-character literal whose character starts at `i`.
fn char_end(s: &[u8], i: usize) -> Option<usize> {
    let c = *s.get(i)?;
    let n = match c {
        0..0x80 => 1,
        0xC0..0xE0 => 2,
        0xE0..0xF0 => 3,
        _ => 4,
    };
    (c != b'\'' && s.get(i + n) == Some(&b'\'')).then_some(i + n + 1)
}

fn find(s: &[u8], from: usize, what: &[u8]) -> Option<usize> {
    if from > s.len() {
        return None;
    }
    s[from..].windows(what.len()).position(|w| w == what).map(|k| k + from)
}

/// Command lines (POSIX shell and fish): the command word of each command
/// (the first of a line not continued, and after `|`, `;`, `&&`, `||`,
/// `(`), keywords, strings, comments, variables and here-documents.
fn shell(s: &[u8], state: &mut State, out: &mut Out) {
    let mut command = true;
    match std::mem::take(state) {
        State::Here(word) => {
            out.push(Class::String, 0, s.len());
            if std::str::from_utf8(s).map(str::trim) != Ok(word.as_str()) {
                *state = State::Here(word);
            }
            return;
        }
        State::Continued => command = false,
        _ => {}
    }
    let mut i = 0;
    let mut word_start = true;
    let mut after_for = 0; // for x in: the words since "for"
    while i < s.len() {
        let b = s[i];
        if b == b' ' || b == b'\t' {
            word_start = true;
            i += 1;
            continue;
        }
        if b == b'#' && word_start {
            return out.push(Class::Comment, i, s.len());
        }
        if b == b'\\' && i + 1 == s.len() {
            *state = State::Continued;
            return;
        }
        if b == b'|' || b == b';' || b == b'&' || b == b'(' {
            command = b != b'&' || s.get(i + 1) == Some(&b'&') || (i > 0 && s[i - 1] == b'&');
            if b == b'&' && i > 0 && s[i - 1] == b'>' {
                command = false; // 2>&1
            }
            word_start = true;
            i += 1;
            continue;
        }
        if b == b'<' && s.get(i + 1) == Some(&b'<') && s.get(i + 2) != Some(&b'<') {
            // A here-document: <<WORD, <<'WORD', <<"WORD", <<-WORD.
            let mut k = i + 2;
            if s.get(k) == Some(&b'-') {
                k += 1;
            }
            let q = s.get(k).copied().filter(|&c| c == b'\'' || c == b'"');
            let from = k + q.is_some() as usize;
            let mut e = from;
            while e < s.len() && is_ident(s[e]) {
                e += 1;
            }
            if e > from {
                let word = String::from_utf8_lossy(&s[from..e]).into_owned();
                let end = if q.is_some() && s.get(e).copied() == q { e + 1 } else { e };
                out.push(Class::Meta, i, end);
                *state = State::Here(word);
                i = end;
                word_start = false;
                continue;
            }
        }
        if b == b'\'' || b == b'"' {
            let e = string_end(s, i + 1, b, b == b'\'').unwrap_or(s.len());
            out.push(Class::String, i, e);
            i = e;
            word_start = false;
            command = false;
            continue;
        }
        if b == b'$' {
            let e = if s.get(i + 1) == Some(&b'{') {
                find(s, i, b"}").map_or(s.len(), |k| k + 1)
            } else if s.get(i + 1) == Some(&b'(') {
                // $(command): the command word follows.
                out.push(Class::Meta, i, i + 2);
                i += 2;
                command = true;
                word_start = true;
                continue;
            } else {
                let mut e = i + 1;
                while e < s.len() && (is_ident(s[e]) || (e == i + 1 && b"?#@*!$".contains(&s[e]))) {
                    e += 1;
                }
                e
            };
            out.push(Class::Meta, i, e);
            i = e;
            word_start = false;
            continue;
        }
        // A word.
        let mut e = i;
        while e < s.len() && !b" \t|;&()<>'\"$".contains(&s[e]) {
            e += 1;
        }
        if e == i {
            i += 1;
            word_start = false;
            continue;
        }
        let w = std::str::from_utf8(&s[i..e]).unwrap_or("");
        if after_for > 0 {
            after_for += 1;
            if after_for == 3 && w == "in" {
                out.push(Class::Keyword, i, e);
            }
        }
        if command && word_start {
            if let Some(eq) = w.find('=').filter(|&k| k > 0 && w[..k].bytes().all(is_ident)) {
                // An assignment before the command.
                out.push(Class::Meta, i, i + eq);
            } else if SH_KEYWORDS.contains(&w) {
                out.push(Class::Keyword, i, e);
                // After these a command follows; after the others (for x
                // in ...) words.
                command = matches!(w, "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "begin");
                after_for = (w == "for") as u8;
            } else {
                out.push(Class::Keyword, i, e);
                command = false;
            }
        }
        i = e;
        word_start = false;
    }
}

/// Article markup (ARTICLE.md): headings, list markers, table bars, code
/// spans and fences.
fn article(s: &[u8], state: &mut State, out: &mut Out) {
    let fence = s.starts_with(b"```");
    if *state == State::Fence {
        if fence && s[3..].iter().all(|b| *b == b' ' || *b == b'\t') {
            *state = State::Normal;
            return out.push(Class::Meta, 0, s.len());
        }
        return out.push(Class::String, 0, s.len());
    }
    if fence {
        *state = State::Fence;
        return out.push(Class::Meta, 0, s.len());
    }
    if s.starts_with(b"# ") || s.starts_with(b"## ") {
        return out.push(Class::Keyword, 0, s.len());
    }
    if s.starts_with(b"    ") || s.starts_with(b"\t") {
        return out.push(Class::String, 0, s.len());
    }
    let mut i = 0;
    if s.starts_with(b"- ") || s.starts_with(b"* ") {
        out.push(Class::Meta, 0, 1);
        i = 2;
    }
    let table = s.starts_with(b"|");
    while i < s.len() {
        if s[i] == b'`'
            && let Some(k) = s[i + 1..].iter().position(|&b| b == b'`').filter(|&k| k > 0)
        {
            out.push(Class::String, i, i + k + 2);
            i += k + 2;
            continue;
        }
        if table && s[i] == b'|' && (i == 0 || s[i - 1] != b'\\') {
            out.push(Class::Meta, i, i + 1);
        }
        i += 1;
    }
}

/// The scripts of `terse run --events` (a frame number, then a command)
/// and `terse-host --drive` (a command): the command, numbers, comments.
fn script(s: &[u8], out: &mut Out) {
    let lead = s.iter().take_while(|b| **b == b' ' || **b == b'\t').count();
    if s.get(lead) == Some(&b'#') {
        return out.push(Class::Comment, lead, s.len());
    }
    let mut first = true;
    let mut i = lead;
    while i < s.len() {
        if s[i] == b' ' || s[i] == b'\t' {
            i += 1;
            continue;
        }
        let mut e = i;
        while e < s.len() && s[e] != b' ' && s[e] != b'\t' {
            e += 1;
        }
        let number = s[i..e].iter().all(|b| b.is_ascii_hexdigit() || b"x-.".contains(b)) && s[i..e].iter().any(u8::is_ascii_digit);
        if number {
            out.push(Class::Number, i, e);
        } else if first {
            out.push(Class::Keyword, i, e);
            first = false;
        }
        i = e;
    }
}

/// Any other language: `#` and `//` comments (at the start of a word),
/// strings, numbers, and a `[section]` line (TOML, INI) as a keyword.
fn generic(s: &[u8], out: &mut Out) {
    let lead = s.iter().take_while(|b| **b == b' ' || **b == b'\t').count();
    if s.get(lead) == Some(&b'[') && s.trim_ascii_end().ends_with(b"]") {
        return out.push(Class::Keyword, lead, s.trim_ascii_end().len());
    }
    let mut i = 0;
    let mut word_start = true;
    while i < s.len() {
        let b = s[i];
        if word_start && (b == b'#' || (b == b'/' && s.get(i + 1) == Some(&b'/'))) {
            return out.push(Class::Comment, i, s.len());
        }
        if word_start && (b == b'"' || b == b'\'') {
            if let Some(e) = string_end(s, i + 1, b, false) {
                out.push(Class::String, i, e);
                i = e;
                word_start = false;
                continue;
            }
        }
        if word_start && b.is_ascii_digit() {
            let e = number_end(s, i);
            out.push(Class::Number, i, e);
            i = e;
            word_start = false;
            continue;
        }
        word_start = !is_ident(b) && b != b'.' && b & 0x80 == 0;
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line as text with each run in brackets: `k[fn] main()`.
    fn show(lang: Lang, src: &str) -> String {
        let lines: Vec<&str> = src.split('\n').collect();
        let runs = highlight(lang, &lines);
        let mut out = Vec::new();
        for (l, rs) in lines.iter().zip(runs) {
            let mut s = String::new();
            let mut at = 0;
            for (c, r) in rs {
                s.push_str(&l[at..r.start]);
                s.push_str(&format!("{}[{}]", c.css().unwrap(), &l[r.clone()]));
                at = r.end;
            }
            s.push_str(&l[at..]);
            out.push(s);
        }
        out.join("\n")
    }

    #[test]
    fn languages_by_word_and_file() {
        assert_eq!(lang("c"), Some(Lang::C));
        assert_eq!(lang("rust from examples/x.rs"), Some(Lang::Rust));
        assert_eq!(lang("go"), Some(Lang::Go));
        assert_eq!(lang("examples/program/c/hello.c"), Some(Lang::C));
        assert_eq!(lang("x/main.go"), Some(Lang::Go));
        assert_eq!(lang("sh"), Some(Lang::Shell));
        assert_eq!(lang("drive examples/program/line.drive"), Some(Lang::Script));
        assert_eq!(lang("text"), None);
        assert_eq!(lang("text examples/x.c"), None);
        assert_eq!(lang(""), None);
        assert_eq!(lang("  "), None);
        assert_eq!(lang("toml"), Some(Lang::Generic));
        assert_eq!(lang("Python"), Some(Lang::Generic));
    }

    #[test]
    fn c() {
        assert_eq!(
            show(Lang::C, "#include \"terse.h\"\n#include <stdint.h>\nstatic uint8_t x = 0x1F; /* a\nb */ int y; // c\nchar c = '\\n';"),
            "m[#include] s[\"terse.h\"]\nm[#include] s[<stdint.h>]\nk[static] t[uint8_t] x = n[0x1F]; c[/* a]\nc[b */] t[int] y; c[// c]\nt[char] c = s['\\n'];"
        );
        assert_eq!(show(Lang::C, "#define U V3_UNIT"), "m[#define] U V3_UNIT");
    }

    #[test]
    fn rust() {
        assert_eq!(
            show(Lang::Rust, "#![no_std]\nfn main() -> Result<(), String> {\n    println!(\"{}\", 'a', x as u8, 1.5);\n    let s = r#\"q\"#; f::<'a>()"),
            "m[#![no_std]]\nk[fn] main() -> t[Result]<(), t[String]> {\n    m[println!](s[\"{}\"], s['a'], x k[as] t[u8], n[1.5]);\n    k[let] s = s[r#\"q\"#]; f::<'a>()"
        );
        assert_eq!(show(Lang::Rust, "let a = \"x\ny\";"), "k[let] a = s[\"x]\ns[y\"];");
        assert_eq!(show(Lang::Rust, "if a != b {}"), "k[if] a != b {}");
    }

    #[test]
    fn go() {
        assert_eq!(
            show(Lang::Go, "package main\nfunc f(b []byte) error { return nil } // x\ns := `a\nb`"),
            "k[package] main\nk[func] f(b []t[byte]) t[error] { k[return] k[nil] } c[// x]\ns := s[`a]\ns[b`]"
        );
    }

    #[test]
    fn shell() {
        assert_eq!(
            show(Lang::Shell, "cargo build --release \\\n    --manifest-path x\nX=1 terse run a | grep \"b\" 2>&1 && ls $HOME # c"),
            "k[cargo] build --release \\\n    --manifest-path x\nm[X]=1 k[terse] run a | k[grep] s[\"b\"] 2>&1 && k[ls] m[$HOME] c[# c]"
        );
        assert_eq!(
            show(Lang::Shell, "xxd -r -p > f <<EOF\n5445\nEOF\nfor f in a; do\n    a=$(bin/x $f)\ndone"),
            "k[xxd] -r -p > f m[<<EOF]\ns[5445]\ns[EOF]\nk[for] f k[in] a; k[do]\n    m[a]=m[$(]k[bin/x] m[$f])\nk[done]"
        );
    }

    #[test]
    fn article_and_scripts() {
        assert_eq!(
            show(Lang::Article, "# Hello\n\nA `code` span.\n- item\n```c\nint x;\n```"),
            "k[# Hello]\n\nA s[`code`] span.\nm[-] item\nm[```c]\ns[int x;]\nm[```]"
        );
        assert_eq!(show(Lang::Script, "# a\n10 press enter\nexpect idle 120"), "c[# a]\nn[10] k[press] enter\nk[expect] idle n[120]");
        assert_eq!(
            show(Lang::Generic, "[build]\nname = \"x\" # y\n_start: li t0, 100000"),
            "k[[build]]\nname = s[\"x\"] c[# y]\n_start: li t0, n[100000]"
        );
    }
}
