//! A reference parser for the article format (ARTICLE.md), written from
//! that document alone, and the article dump of `vectors/README`.
//!
//! [`parse`] turns an article's bytes into [`Block`]s: what a reader must
//! show, with the constructs resolved (paragraph lines joined, links and
//! code spans found, table cells cut and aligned, figure paths reduced to
//! absolute commands in sixteenths of a unit). [`dump`] writes that as the
//! plain text format the test vectors use. The parser is a model of the
//! format, not a renderer: it says what is shown, not where.
//!
//! Where ARTICLE.md leaves a choice open, the choice made here is noted at
//! the place it is made ("ARTICLE.md is silent").

use std::fmt::Write as _;

/// What a link leads to (ARTICLE.md 4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An article, by blob ID.
    Article([u8; 32]),
    /// A feed, by key, and the hint list of its address (NETWORK.md 4.9)
    /// as written, or empty.
    Feed([u8; 32], String),
    /// A program: the publisher's feed key and the decoded name (a display
    /// name, so valid UTF-8).
    Program([u8; 32], String),
}

/// A piece of shown text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    /// Ordinary text, as shown (control characters other than tab removed,
    /// malformed UTF-8 as U+FFFD). Runs of blanks are as written.
    Text(String),
    /// A code span's text, without its backquotes.
    Code(String),
    /// A link: bare (`text` is `None`, shown as written or shortened) or
    /// with text.
    Link { target: Target, text: Option<String> },
}

/// A column's alignment (ARTICLE.md 2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Centre,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// 1 to 8.
    pub columns: usize,
    /// The first row is a header (there was an alignment row).
    pub header: bool,
    /// One per column.
    pub align: Vec<Align>,
    /// Each row has exactly `columns` cells.
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// A colour of a figure (ARTICLE.md 6.1): `fg` or (r, g, b, a). A colour
/// with opacity 0 is kept as `None` by the paths that use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    Fg,
    Rgba([u8; 4]),
}

/// One command of a figure path, in sixteenths of a unit, absolute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    Move(i32, i32),
    Line(i32, i32),
    Quad(i32, i32, i32, i32),
    Cubic(i32, i32, i32, i32, i32, i32),
    Close,
}

/// A path (ARTICLE.md 6.3): its subpaths, filled then stroked. Every
/// subpath starts with a Move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    /// `None`: not filled.
    pub fill: Option<Colour>,
    /// Colour and width in sixteenths; `None`: not stroked.
    pub stroke: Option<(Colour, u32)>,
    pub segs: Vec<Seg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Figure {
    pub width: u32,
    pub height: u32,
    /// The paths that draw something, in order.
    pub paths: Vec<Path>,
}

/// A block as shown (ARTICLE.md 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(Vec<Inline>),
    Subheading(Vec<Inline>),
    Paragraph(Vec<Inline>),
    Item(Vec<Inline>),
    /// A preformatted block or a fence (or a figure shown as its fence):
    /// its lines, as written, links found in them.
    Pre(Vec<Vec<Inline>>),
    Table(Table),
    Figure(Figure),
}

/// What a blob is to a reader (ARTICLE.md 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// Starts with `TERS`: a program image, not shown as text.
    ProgramImage,
    /// Starts with `TDOC`: a document, not shown as text.
    Document,
    Article(Vec<Block>),
}

// --- Bytes -----------------------------------------------------------------

fn is_blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r')
}

fn is_alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn lower_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

fn any_hex(b: u8) -> Option<u8> {
    lower_hex(b.to_ascii_lowercase())
}

/// Shown text (ARTICLE.md 1): malformed UTF-8 as U+FFFD, the control
/// characters but tab and CR not shown. Tabs are kept (a reader shows each
/// as at least one space). A CR here is a lone one (the CR before an LF is
/// gone with the line break, [`lines`]): a blank, shown as a space.
fn shown(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|&c| c == '\t' || c == '\r' || !c.is_control())
        .map(|c| if c == '\r' { ' ' } else { c })
        .collect()
}

/// Shown text of a table cell (`cell`: `\|` stands for `|`, ARTICLE.md
/// 2.5) or of any other block.
fn shown_in(bytes: &[u8], cell: bool) -> String {
    if cell { shown(&unescape(bytes)) } else { shown(bytes) }
}

fn trim_start_blanks(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|&&c| is_blank(c)).count();
    &b[n..]
}

fn trim_end_blanks(b: &[u8]) -> &[u8] {
    let n = b.iter().rev().take_while(|&&c| is_blank(c)).count();
    &b[..b.len() - n]
}

// --- Display names (NETWORK.md 4.6) ------------------------------------------

/// The White_Space characters NETWORK.md 4.6 lists.
fn white_space(c: char) -> bool {
    matches!(c as u32,
        0x09..=0x0D | 0x20 | 0x85 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000)
}

/// Whether a character is in the basic text repertoire (ARTICLE.md 1.1,
/// the terminal's basic repertoire too: `terse_core::text::BASIC`), or is
/// one that ARTICLE.md 1 does not show (control characters) or shows as a
/// blank.
pub fn basic_char(c: char) -> bool {
    c.is_control() || u16::try_from(c as u32).is_ok_and(terse_core::text::is_basic)
}

/// Whether text (an article's bytes, or a title) is in basic text
/// (ARTICLE.md 1.1): every character it shows is in the basic text
/// repertoire. Malformed UTF-8 is shown as U+FFFD, which is; a byte order
/// mark that starts it is not shown.
pub fn basic_text(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).chars().all(basic_char)
}

/// Whether text is in the full repertoire (ARTICLE.md 1.1, SPEC
/// §terminal.text): every character it shows is one that every full-text
/// reader of Terse draws from its built-in fonts; other characters show
/// only where the user added a font that has them.
pub fn full_text(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    String::from_utf8_lossy(bytes)
        .chars()
        .all(|c| c.is_control() || u16::try_from(c as u32).is_ok_and(terse_core::text::is_full))
}

/// A display name: 1 to 48 bytes of UTF-8, no control characters, no
/// whitespace first or last.
pub fn display_name(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?;
    let control = |c: char| matches!(c as u32, 0..=0x1F | 0x7F..=0x9F);
    if !(1..=48).contains(&bytes.len()) || s.chars().any(control) {
        return None;
    }
    if s.starts_with(white_space) || s.ends_with(white_space) {
        return None;
    }
    Some(s.to_string())
}

// --- Links (ARTICLE.md 4) ---------------------------------------------------

/// 32 bytes from 64 lowercase hex digits at the start of `s`.
fn hex64(s: &[u8]) -> Option<[u8; 32]> {
    let d = s.get(..64)?;
    let mut out = [0u8; 32];
    for (i, pair) in d.chunks_exact(2).enumerate() {
        out[i] = lower_hex(pair[0])? << 4 | lower_hex(pair[1])?;
    }
    Some(out)
}

/// A bare link starting at `line[at]`, its length and target. `at` must
/// not be preceded by a letter or digit (the caller checks).
fn bare_link(line: &[u8], at: usize) -> Option<(usize, Target)> {
    let s = line[at..].strip_prefix(b"terse:")?;
    let not_followed = |rest: &[u8]| rest.first().is_none_or(|&b| !is_alnum(b));
    if let Some(id) = hex64(s) {
        return not_followed(&s[64..]).then_some((6 + 64, Target::Article(id)));
    }
    if let Some(r) = s.strip_prefix(b"feed/") {
        let key = hex64(r)?;
        if !not_followed(&r[64..]) {
            return None;
        }
        // ARTICLE.md 4.1: the hints after an `@`, if they are a valid
        // hint list once `.`, `,` and `:` at their end are left out; else
        // the link is the key alone.
        let n = match r[64..].strip_prefix(b"@") {
            Some(after) => terse_core::hints::article_hints(after),
            None => 0,
        };
        let hints = if n == 0 { String::new() } else { String::from_utf8(r[65..65 + n].to_vec()).unwrap() };
        let len = if n == 0 { 11 + 64 } else { 11 + 64 + 1 + n };
        return Some((len, Target::Feed(key, hints)));
    }
    let r = s.strip_prefix(b"program/")?;
    let key = hex64(r)?;
    let r = r[64..].strip_prefix(b"/")?;
    // The run of unreserved bytes and %XX triples.
    let mut k = 0;
    loop {
        match r.get(k) {
            Some(b) if b.is_ascii_alphanumeric() || b"-._~".contains(b) => k += 1,
            Some(b'%') if r.len() >= k + 3 && any_hex(r[k + 1]).is_some() && any_hex(r[k + 2]).is_some() => k += 3,
            _ => break,
        }
    }
    // "less every `.` at its end": all final dots (a run of dots ends a
    // sentence as well as one).
    while k > 0 && r[k - 1] == b'.' {
        k -= 1;
    }
    let mut name = Vec::new();
    let mut i = 0;
    while i < k {
        if r[i] == b'%' {
            name.push(any_hex(r[i + 1])? << 4 | any_hex(r[i + 2])?);
            i += 3;
        } else {
            name.push(r[i]);
            i += 1;
        }
    }
    let name = display_name(&name)?;
    Some((6 + 8 + 64 + 1 + k, Target::Program(key, name)))
}

/// Whether a bare link starts at `line[at]` (with the preceding-byte rule).
fn bare_at(line: &[u8], at: usize) -> Option<(usize, Target)> {
    if at > 0 && is_alnum(line[at - 1]) {
        return None;
    }
    bare_link(line, at)
}

/// A link with text at `line[at]` (a `[`): its length, target and text.
fn text_link(line: &[u8], at: usize) -> Option<(usize, Target, Vec<u8>)> {
    let rest = &line[at + 1..];
    let close = rest.iter().position(|&b| b == b']')?;
    let text = &rest[..close];
    if !(1..=200).contains(&text.len()) || text.iter().any(|&b| b == b'[' || b == b'`') {
        return None;
    }
    if (0..text.len()).any(|i| bare_at(text, i).is_some()) {
        return None;
    }
    let after = rest[close + 1..].strip_prefix(b"(")?;
    let end = after.iter().position(|&b| b == b')')?;
    let inner = &after[..end];
    let (len, target) = bare_link(inner, 0)?;
    if len != inner.len() {
        return None;
    }
    // `[`, text, `]`, `(`, target, `)`.
    Some((1 + text.len() + 2 + inner.len() + 1, target, text.to_vec()))
}

/// A code span at `line[at]` (a backquote): its length.
fn code_span(line: &[u8], at: usize) -> Option<usize> {
    let rest = &line[at + 1..];
    let close = rest.iter().position(|&b| b == b'`')?;
    (1..=200).contains(&close).then_some(close + 2)
}

/// The inline content of one line (ARTICLE.md 3): links, and code spans if
/// `code`.
fn inlines(line: &[u8], code: bool) -> Vec<Inline> {
    inlines_in(line, code, false)
}

/// The inline content of one line, or (`cell`) of a table cell as written:
/// links and code spans are found in the bytes as written, so their limits
/// count `\|` as two bytes (ARTICLE.md 4.2, 5), and `\|` is shown as `|`.
fn inlines_in(line: &[u8], code: bool, cell: bool) -> Vec<Inline> {
    let mut out = Vec::new();
    let (mut at, mut from) = (0, 0);
    let flush = |out: &mut Vec<Inline>, from: usize, to: usize| {
        if to > from {
            push_text(out, &shown_in(&line[from..to], cell));
        }
    };
    while at < line.len() {
        let b = line[at];
        if b == b'[' {
            if let Some((len, target, text)) = text_link(line, at) {
                flush(&mut out, from, at);
                out.push(Inline::Link { target, text: Some(shown_in(&text, cell)) });
                at += len;
                from = at;
                continue;
            }
        } else if b == b'`' && code {
            if let Some(len) = code_span(line, at) {
                flush(&mut out, from, at);
                out.push(Inline::Code(shown_in(&line[at + 1..at + len - 1], cell)));
                at += len;
                from = at;
                continue;
            }
        } else if b == b't'
            && let Some((len, target)) = bare_at(line, at)
        {
            flush(&mut out, from, at);
            out.push(Inline::Link { target, text: None });
            at += len;
            from = at;
            continue;
        }
        at += 1;
    }
    flush(&mut out, from, line.len());
    out
}

fn push_text(out: &mut Vec<Inline>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Inline::Text(t)) = out.last_mut() {
        t.push_str(s);
    } else {
        out.push(Inline::Text(s.to_string()));
    }
}

/// Lines joined (ARTICLE.md 2.1): blanks at the start of each line dropped,
/// each line break a space.
fn joined(lines: &[&[u8]]) -> Vec<Inline> {
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            push_text(&mut out, " ");
        }
        for x in inlines(trim_start_blanks(l), true) {
            match x {
                Inline::Text(t) => push_text(&mut out, &t),
                x => out.push(x),
            }
        }
    }
    out
}

// --- Blocks (ARTICLE.md 2) --------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    H1,
    H2,
    Item,
    Pre,
    Fence,
    Table,
    Ordinary,
}

fn kind(line: &[u8]) -> Kind {
    if line.iter().all(|&b| is_blank(b)) {
        Kind::Blank
    } else if line.starts_with(b"# ") {
        Kind::H1
    } else if line.starts_with(b"## ") {
        Kind::H2
    } else if line.starts_with(b"- ") || line.starts_with(b"* ") {
        Kind::Item
    } else if line.starts_with(b"    ") || line.starts_with(b"\t") {
        Kind::Pre
    } else if line.starts_with(b"```") {
        Kind::Fence
    } else if line.starts_with(b"|") {
        Kind::Table
    } else {
        Kind::Ordinary
    }
}

/// The article's lines: split at LF, a CR before each LF dropped. A final
/// LF ends the last line (it does not start an empty one).
fn lines(text: &[u8]) -> Vec<&[u8]> {
    let mut out: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
    if text.ends_with(b"\n") || text.is_empty() {
        out.pop();
    }
    let n = out.len();
    for (i, l) in out.iter_mut().enumerate() {
        // Only a CR before an LF is ignored; the last line has an LF after
        // it if the text ends with one.
        if (i + 1 < n || text.ends_with(b"\n")) && l.ends_with(b"\r") {
            *l = &l[..l.len() - 1];
        }
    }
    out
}

/// Parses a blob as a reader must show it.
pub fn parse(blob: &[u8]) -> Parsed {
    if blob.starts_with(b"TERS") {
        return Parsed::ProgramImage;
    }
    if blob.starts_with(b"TDOC") {
        return Parsed::Document;
    }
    // A byte order mark at the start is ignored (ARTICLE.md 1).
    let ls = lines(blob.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(blob));
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < ls.len() {
        let line = ls[i];
        let k = kind(line);
        i += 1;
        // The ordinary lines after line i - 1.
        let ordinary = |i: &mut usize| {
            let from = *i;
            while *i < ls.len() && kind(ls[*i]) == Kind::Ordinary {
                *i += 1;
            }
            from..*i
        };
        match k {
            Kind::Blank => {}
            Kind::H1 => blocks.push(Block::Heading(heading(&line[2..]))),
            Kind::H2 => blocks.push(Block::Subheading(heading(&line[3..]))),
            Kind::Item => {
                let more = ordinary(&mut i);
                let mut all = vec![&line[2..]];
                all.extend_from_slice(&ls[more]);
                blocks.push(Block::Item(joined(&all)));
            }
            Kind::Ordinary => {
                let more = ordinary(&mut i);
                let mut all = vec![line];
                all.extend_from_slice(&ls[more]);
                blocks.push(Block::Paragraph(joined(&all)));
            }
            Kind::Pre => {
                let mut all = vec![line];
                while i < ls.len() && kind(ls[i]) == Kind::Pre {
                    all.push(ls[i]);
                    i += 1;
                }
                blocks.push(Block::Pre(all.iter().map(|l| inlines(l, false)).collect()));
            }
            Kind::Table => {
                let mut all = vec![line];
                while i < ls.len() && ls[i].starts_with(b"|") {
                    all.push(ls[i]);
                    i += 1;
                }
                blocks.push(Block::Table(table(&all)));
            }
            Kind::Fence => {
                let from = i;
                let mut closed = false;
                while i < ls.len() {
                    let l = ls[i];
                    i += 1;
                    if l.starts_with(b"```") && l[3..].iter().all(|&b| is_blank(b)) {
                        closed = true;
                        break;
                    }
                }
                let body = &ls[from..if closed { i - 1 } else { i }];
                if closed && let Some(fig) = figure(&line[3..], body) {
                    blocks.push(Block::Figure(fig));
                } else if !body.is_empty() {
                    blocks.push(Block::Pre(body.iter().map(|l| inlines(l, false)).collect()));
                }
            }
        }
    }
    Parsed::Article(blocks)
}

/// A heading's text: the blanks after the marker dropped (and, as for a
/// paragraph's line, the text's links and code spans).
fn heading(rest: &[u8]) -> Vec<Inline> {
    inlines(trim_start_blanks(rest), true)
}

// --- Tables (ARTICLE.md 2.5) ------------------------------------------------

/// A row's cells, as source bytes (escapes not yet replaced).
fn cells(line: &[u8]) -> Vec<&[u8]> {
    let mut r = trim_end_blanks(&line[1..]);
    if r.ends_with(b"|") && !(r.len() >= 2 && r[r.len() - 2] == b'\\') {
        r = &r[..r.len() - 1];
    }
    let mut out = Vec::new();
    let mut from = 0;
    for k in 0..r.len() {
        if r[k] == b'|' && !(k > 0 && r[k - 1] == b'\\') {
            out.push(&r[from..k]);
            from = k + 1;
        }
    }
    out.push(&r[from..]);
    out.into_iter().map(|c| trim_end_blanks(trim_start_blanks(c))).collect()
}

/// `\|` as `|`; any other backslash is itself.
fn unescape(cell: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cell.len());
    let mut k = 0;
    while k < cell.len() {
        if cell[k] == b'\\' && cell.get(k + 1) == Some(&b'|') {
            out.push(b'|');
            k += 2;
        } else {
            out.push(cell[k]);
            k += 1;
        }
    }
    out
}

fn alignment_row(line: &[u8]) -> bool {
    line.iter().all(|&b| matches!(b, b'|' | b'-' | b':') || is_blank(b)) && line.contains(&b'-')
}

fn align_of(cell: &[u8]) -> Align {
    if cell.len() >= 2 && cell.starts_with(b":") && cell.ends_with(b":") {
        Align::Centre
    } else if cell.ends_with(b":") {
        // Ends with `:` and does not start with one, or is only `:`.
        Align::End
    } else {
        Align::Start
    }
}

fn table(lines: &[&[u8]]) -> Table {
    let columns = cells(lines[0]).len().min(8);
    let header = lines.len() >= 2 && alignment_row(lines[1]);
    let mut align = vec![Align::Start; columns];
    if header {
        for (a, c) in align.iter_mut().zip(cells(lines[1])) {
            *a = align_of(c);
        }
    }
    let rows = lines
        .iter()
        .enumerate()
        .filter(|&(i, _)| !(header && i == 1))
        .map(|(_, l)| {
            let mut row: Vec<Vec<Inline>> = cells(l).into_iter().take(columns).map(|c| inlines_in(c, true, true)).collect();
            row.resize(columns, Vec::new());
            row
        })
        .collect();
    Table { columns, header, align, rows }
}

// --- Figures (ARTICLE.md 6) -------------------------------------------------

/// Points lie within ±2047 units.
const LIMIT: i64 = 2047 * 16;

/// The size on a figure's opening line (after the backquotes), if it is a
/// figure: `None` if the info string is not `path`; `Some(None)` if it is
/// but the rest is malformed.
fn figure_size(info: &[u8]) -> Option<Option<(u32, u32)>> {
    let rest = trim_start_blanks(info).strip_prefix(b"path")?;
    if !rest.first().is_none_or(|&b| is_blank(b)) {
        return None;
    }
    // W, blanks, H, then only blanks; each 1 to 4 decimal digits (no sign,
    // leading zeros allowed) for a value from 1 to 1024.
    let words: Vec<&[u8]> = rest.split(|&b| is_blank(b)).filter(|w| !w.is_empty()).collect();
    let num = |w: &[u8]| -> Option<u32> {
        if w.is_empty() || w.len() > 4 || !w.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let v: u64 = std::str::from_utf8(w).ok()?.parse().ok()?;
        (1..=1024).contains(&v).then_some(v as u32)
    };
    Some(match words.as_slice() {
        [w, h] => num(w).zip(num(h)),
        _ => None,
    })
}

fn figure(info: &[u8], body: &[&[u8]]) -> Option<Figure> {
    let (width, height) = figure_size(info)??;
    let path: Vec<u8> = body.join(&b'\n');
    let paths = path_string(&path)?;
    Some(Figure { width, height, paths })
}

fn is_sep(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b',')
}

/// A number at `s[*at]` (ARTICLE.md 6.1) in sixteenths, rounded to the
/// nearest (halves away from zero); `None` if there is none or it is
/// malformed. Values far out of range saturate (they are refused later).
fn number(s: &[u8], at: &mut usize) -> Option<i64> {
    number_exact(s, at).map(|(v, _)| v)
}

/// A number as [`number`] does, and the sign of what rounding took off:
/// 1 if the number as written is greater than the rounded value, -1 if it
/// is less, 0 if it is a whole number of sixteenths. Range checks apply to
/// the number as written (ARTICLE.md 6.1), so a rounded value at a limit
/// is out of range if the number as written is beyond it.
fn number_exact(s: &[u8], at: &mut usize) -> Option<(i64, i64)> {
    let start = *at;
    let mut k = start;
    let neg = match s.get(k) {
        Some(b'-') => {
            k += 1;
            true
        }
        Some(b'+') => {
            k += 1;
            false
        }
        _ => false,
    };
    let int_from = k;
    while s.get(k).is_some_and(u8::is_ascii_digit) {
        k += 1;
    }
    let int = &s[int_from..k];
    let mut frac: &[u8] = &[];
    if s.get(k) == Some(&b'.') {
        k += 1;
        let f = k;
        while s.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        frac = &s[f..k];
    }
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if k - start > 32 {
        return None;
    }
    *at = k;
    // value × 16 = int × 16 + frac × 16 / 10^len, rounded half away.
    let mut i: u128 = 0;
    for &d in int {
        i = i.saturating_mul(10).saturating_add((d - b'0') as u128);
    }
    let (mut f, mut scale): (u128, u128) = (0, 1);
    for &d in frac {
        f = f * 10 + (d - b'0') as u128;
        scale *= 10;
    }
    let (q, r) = (f * 16 / scale, f * 16 % scale);
    let mut v = i.saturating_mul(16).saturating_add(q);
    // The magnitude as written less the rounded one: r / scale, or that
    // less 1 if rounded up.
    let mut excess = (r > 0) as i64;
    if 2 * r >= scale {
        v = v.saturating_add(1);
        excess = -1;
    }
    let v = v.min(i64::MAX as u128 / 2) as i64;
    Some(if neg { (-v, -excess) } else { (v, excess) })
}

/// Whether a coordinate, rounded to `v` sixteenths with `excess` the sign
/// of what rounding took off ([`number_exact`]), lies within ±2047 units
/// as written.
fn in_range(v: i64, excess: i64) -> bool {
    v.abs() < LIMIT || (v == LIMIT && excess <= 0) || (v == -LIMIT && excess >= 0)
}

/// A colour at `s[*at]`: `#rgb`, `#rrggbb`, `#rrggbbaa`, `none` (opacity
/// 0) or `fg`; it must be followed by a separator or the end.
fn colour(s: &[u8], at: &mut usize) -> Option<Colour> {
    let end = s[*at..].iter().position(|&b| is_sep(b)).map_or(s.len(), |p| *at + p);
    let w = &s[*at..end];
    *at = end;
    match w {
        b"none" => Some(Colour::Rgba([0; 4])),
        b"fg" => Some(Colour::Fg),
        _ => {
            let h = w.strip_prefix(b"#")?;
            let d: Vec<u8> = h.iter().map(|&b| any_hex(b)).collect::<Option<_>>()?;
            match d.len() {
                3 => Some(Colour::Rgba([d[0] * 17, d[1] * 17, d[2] * 17, 255])),
                6 | 8 => {
                    let byte = |i: usize| d.get(2 * i).map_or(255, |&hi| hi << 4 | d[2 * i + 1]);
                    Some(Colour::Rgba([byte(0), byte(1), byte(2), byte(3)]))
                }
                _ => None,
            }
        }
    }
}

fn visible(c: Colour) -> Option<Colour> {
    match c {
        Colour::Rgba([_, _, _, 0]) => None,
        c => Some(c),
    }
}

/// The path string of a figure (ARTICLE.md 6.1, 6.3): its paths that draw
/// something, or `None` if it is malformed.
fn path_string(s: &[u8]) -> Option<Vec<Path>> {
    let mut paths = Vec::new();
    let mut fill: Option<Colour> = None;
    let mut stroke: Option<(Colour, u32)> = Some((Colour::Fg, 16));
    let mut segs: Vec<Seg> = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    let mut start: Option<(i64, i64)> = None;
    // The last control point of an immediately preceding C or S (true) or
    // Q or T (false).
    let mut last_ctrl: Option<(bool, i64, i64)> = None;
    let mut cmd: Option<u8> = None;
    let mut first = true;
    let flush = |paths: &mut Vec<Path>, segs: &mut Vec<Seg>, fill: Option<Colour>, stroke: Option<(Colour, u32)>| {
        let draws = segs.iter().any(|s| !matches!(s, Seg::Move(..)));
        if draws && (fill.is_some() || stroke.is_some()) {
            paths.push(Path { fill, stroke, segs: std::mem::take(segs) });
        }
        segs.clear();
    };
    let mut at = 0;
    loop {
        while at < s.len() && is_sep(s[at]) {
            at += 1;
        }
        if at >= s.len() {
            break;
        }
        let b = s[at];
        // Words: at the start of the path string or after a separator (a
        // command letter may follow a number directly; a word may not).
        let after_sep = at == 0 || is_sep(s[at - 1]);
        let word = if !after_sep {
            None
        } else if s[at..].starts_with(b"stroke") {
            Some(true)
        } else if s[at..].starts_with(b"fill") {
            Some(false)
        } else {
            None
        };
        if let Some(is_stroke) = word {
            at += if is_stroke { 6 } else { 4 };
            if at >= s.len() || !is_sep(s[at]) {
                return None;
            }
            while at < s.len() && is_sep(s[at]) {
                at += 1;
            }
            if at >= s.len() {
                return None;
            }
            let c = colour(s, &mut at)?;
            flush(&mut paths, &mut segs, fill, stroke);
            start = None;
            cmd = None;
            if is_stroke {
                let mut k = at;
                while k < s.len() && is_sep(s[k]) {
                    k += 1;
                }
                let mut width = 16;
                if let Some((w, ex)) = number_exact(s, &mut k) {
                    // 0 to 2047 as written.
                    if !(0..=LIMIT).contains(&w) || (w == LIMIT && ex > 0) || (w == 0 && ex < 0) {
                        return None;
                    }
                    width = w as u32;
                    at = k;
                }
                stroke = visible(c).filter(|_| width > 0).map(|c| (c, width));
            } else {
                fill = visible(c);
            }
            continue;
        }
        // Commands.
        let arity = match b.to_ascii_uppercase() {
            b'M' | b'L' | b'T' => Some(2),
            b'H' | b'V' => Some(1),
            b'C' => Some(6),
            b'S' | b'Q' => Some(4),
            b'Z' => Some(0),
            _ => None,
        };
        let op = if arity.is_some() {
            at += 1;
            if first && b != b'M' && b != b'm' {
                return None;
            }
            first = false;
            b
        } else {
            // A number repeats the current command.
            match cmd {
                Some(c) if c.to_ascii_uppercase() != b'Z' => {
                    number(s, &mut at.clone())?;
                    match c {
                        b'M' => b'L',
                        b'm' => b'l',
                        c => c,
                    }
                }
                _ => return None,
            }
        };
        let n = match op.to_ascii_uppercase() {
            b'M' | b'L' | b'T' => 2,
            b'H' | b'V' => 1,
            b'C' => 6,
            b'S' | b'Q' => 4,
            _ => 0,
        };
        let mut v = [0i64; 6];
        let mut ex = [0i64; 6];
        for k in 0..n {
            while at < s.len() && is_sep(s[at]) {
                at += 1;
            }
            (v[k], ex[k]) = number_exact(s, &mut at)?;
        }
        let rel = op.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0, 0) };
        let pt = |x: i64, y: i64| (ox + x, oy + y);
        // A point within ±2047 units: computed from the numbers as written
        // (the current point, which is exact, plus a relative command's
        // numbers), before rounding. `k` is the index of the point's x in
        // `v`, or None for a point that is not written (a reflected one).
        let ok = |p: (i64, i64), k: Option<usize>| {
            let (ex, ey) = k.map_or((0, 0), |k| (ex[k], ex[k + 1]));
            in_range(p.0, ex) && in_range(p.1, ey)
        };
        let up = op.to_ascii_uppercase();
        if up != b'M' && up != b'Z' && start.is_none() {
            segs.push(Seg::Move(cx as i32, cy as i32));
            start = Some((cx, cy));
        }
        let reflect = |cubic: bool| match last_ctrl {
            Some((c, x, y)) if c == cubic => (2 * cx - x, 2 * cy - y),
            _ => (cx, cy),
        };
        let mut ctrl = None;
        match up {
            b'M' => {
                let p = pt(v[0], v[1]);
                if !ok(p, Some(0)) {
                    return None;
                }
                (cx, cy) = p;
                segs.push(Seg::Move(cx as i32, cy as i32));
                start = Some(p);
            }
            b'L' | b'H' | b'V' => {
                let (p, e) = match up {
                    b'L' => (pt(v[0], v[1]), (ex[0], ex[1])),
                    b'H' => ((ox + v[0], cy), (ex[0], 0)),
                    _ => ((cx, oy + v[0]), (0, ex[0])),
                };
                if !(in_range(p.0, e.0) && in_range(p.1, e.1)) {
                    return None;
                }
                (cx, cy) = p;
                segs.push(Seg::Line(cx as i32, cy as i32));
            }
            b'C' | b'S' => {
                let (c1, c2, p, k) = if up == b'C' {
                    (pt(v[0], v[1]), pt(v[2], v[3]), pt(v[4], v[5]), [Some(0), Some(2), Some(4)])
                } else {
                    (reflect(true), pt(v[0], v[1]), pt(v[2], v[3]), [None, Some(0), Some(2)])
                };
                if !ok(c1, k[0]) || !ok(c2, k[1]) || !ok(p, k[2]) {
                    return None;
                }
                segs.push(Seg::Cubic(c1.0 as i32, c1.1 as i32, c2.0 as i32, c2.1 as i32, p.0 as i32, p.1 as i32));
                (cx, cy) = p;
                ctrl = Some((true, c2.0, c2.1));
            }
            b'Q' | b'T' => {
                let (c1, p, k) = if up == b'Q' {
                    (pt(v[0], v[1]), pt(v[2], v[3]), [Some(0), Some(2)])
                } else {
                    (reflect(false), pt(v[0], v[1]), [None, Some(0)])
                };
                if !ok(c1, k[0]) || !ok(p, k[1]) {
                    return None;
                }
                segs.push(Seg::Quad(c1.0 as i32, c1.1 as i32, p.0 as i32, p.1 as i32));
                (cx, cy) = p;
                ctrl = Some((false, c1.0, c1.1));
            }
            _ => {
                // Z. With no subpath open it does nothing and leaves the
                // current point where it is (ARTICLE.md 6.3).
                if let Some(p) = start.take() {
                    segs.push(Seg::Close);
                    (cx, cy) = p;
                }
            }
        }
        last_ctrl = ctrl;
        cmd = Some(op);
    }
    flush(&mut paths, &mut segs, fill, stroke);
    Some(paths)
}

// --- The dump (vectors/README) ---------------------------------------------

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// A string in double quotes: `\` and `"` escaped, tab, LF and CR as
/// `\t`, `\n` and `\r`.
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn target(t: &Target) -> String {
    match t {
        Target::Article(id) => format!("article {}", hex(id)),
        Target::Feed(k, h) if h.is_empty() => format!("feed {}", hex(k)),
        Target::Feed(k, h) => format!("feed {} @{h}", hex(k)),
        Target::Program(k, name) => format!("program {} {}", hex(k), quote(name)),
    }
}

fn dump_inlines(out: &mut String, xs: &[Inline], indent: &str) {
    for x in xs {
        let _ = match x {
            Inline::Text(t) => writeln!(out, "{indent}text {}", quote(t)),
            Inline::Code(t) => writeln!(out, "{indent}code {}", quote(t)),
            Inline::Link { target: t, text: None } => writeln!(out, "{indent}link {}", target(t)),
            Inline::Link { target: t, text: Some(s) } => writeln!(out, "{indent}link {} as {}", target(t), quote(s)),
        };
    }
}

fn colour_text(c: Colour) -> String {
    match c {
        Colour::Fg => "fg".into(),
        Colour::Rgba(c) => format!("#{}", hex(&c)),
    }
}

/// The dump of one block (vectors/README, "Article dumps").
pub fn dump_block(out: &mut String, b: &Block) {
    match b {
        Block::Heading(x) => {
            out.push_str("heading\n");
            dump_inlines(out, x, "  ");
        }
        Block::Subheading(x) => {
            out.push_str("subheading\n");
            dump_inlines(out, x, "  ");
        }
        Block::Paragraph(x) => {
            out.push_str("paragraph\n");
            dump_inlines(out, x, "  ");
        }
        Block::Item(x) => {
            out.push_str("item\n");
            dump_inlines(out, x, "  ");
        }
        Block::Pre(lines) => {
            out.push_str("pre\n");
            for l in lines {
                out.push_str("  line\n");
                dump_inlines(out, l, "    ");
            }
        }
        Block::Table(t) => {
            let _ = writeln!(out, "table {} {}", t.columns, if t.header { "header" } else { "noheader" });
            let a: Vec<&str> = t
                .align
                .iter()
                .map(|a| match a {
                    Align::Start => "start",
                    Align::Centre => "centre",
                    Align::End => "end",
                })
                .collect();
            let _ = writeln!(out, "  align {}", a.join(" "));
            for row in &t.rows {
                out.push_str("  row\n");
                for cell in row {
                    out.push_str("    cell\n");
                    dump_inlines(out, cell, "      ");
                }
            }
        }
        Block::Figure(f) => {
            let _ = writeln!(out, "figure {} {}", f.width, f.height);
            for p in &f.paths {
                let fill = p.fill.map_or("none".into(), colour_text);
                let stroke = p.stroke.map_or("none".into(), |(c, w)| format!("{} {w}", colour_text(c)));
                let _ = writeln!(out, "  path fill {fill} stroke {stroke}");
                for s in &p.segs {
                    let _ = match *s {
                        Seg::Move(x, y) => writeln!(out, "    M {x} {y}"),
                        Seg::Line(x, y) => writeln!(out, "    L {x} {y}"),
                        Seg::Quad(a, b, x, y) => writeln!(out, "    Q {a} {b} {x} {y}"),
                        Seg::Cubic(a, b, c, d, x, y) => writeln!(out, "    C {a} {b} {c} {d} {x} {y}"),
                        Seg::Close => writeln!(out, "    Z"),
                    };
                }
            }
        }
    }
}

/// The dump of a parsed blob (vectors/README, "Article dumps").
pub fn dump(p: &Parsed) -> String {
    let mut out = String::new();
    match p {
        Parsed::ProgramImage => out.push_str("not-an-article program-image\n"),
        Parsed::Document => out.push_str("not-an-article document\n"),
        Parsed::Article(blocks) => {
            for b in blocks {
                dump_block(&mut out, b);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_text_repertoire() {
        assert!(basic_text(b"Plain ASCII, tabs\tand\r\nlines. 3 \xc3\x97 4 \xe2\x88\x92 1 \xe2\x80\x94 \xe2\x80\x9cyes\xe2\x80\x9d\xe2\x80\xa6"));
        assert!(basic_text(b"\xef\xbb\xbfmalformed \xff is U+FFFD"));
        assert!(basic_text("caf\u{e9}, x\u{a0}y, \u{2500}\u{2588}".as_bytes()), "Latin-1, box drawing");
        assert!(!basic_text("a\u{feff}b".as_bytes()), "a BOM not at the start is text");
        assert!(!basic_text("5 \u{20ac}".as_bytes()));
        assert!(!basic_text("\u{1f600}".as_bytes()));
    }

    /// The texts of `vectors/basic-text.txt` are in basic text or not as
    /// it says, and the repertoire (control characters aside) is its ranges.
    #[test]
    fn basic_text_vectors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/basic-text.txt");
        let text = std::fs::read_to_string(path).unwrap();
        let (mut ranges, mut cases, mut hex) = (Vec::new(), 0, None);
        for line in text.lines() {
            let mut w = line.split_whitespace();
            match (w.next(), w.next(), w.next()) {
                (Some("range"), Some(a), Some(b)) => {
                    ranges.push(u32::from_str_radix(a, 16).unwrap()..=u32::from_str_radix(b, 16).unwrap())
                }
                (Some("hex"), Some(h), None) => hex = Some(h.to_string()),
                (Some("basic"), Some(yes), None) => {
                    let h = hex.take().unwrap();
                    let bytes: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
                    assert_eq!(basic_text(&bytes), yes == "yes", "{h}");
                    cases += 1;
                }
                _ => {}
            }
        }
        assert!(cases >= 10);
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            let listed = ranges.iter().any(|r| r.contains(&(c as u32)));
            assert_eq!(basic_char(c) && !c.is_control(), listed, "U+{:04X}", c as u32);
        }
    }

    fn d(s: &str) -> String {
        dump(&parse(s.as_bytes()))
    }

    #[test]
    fn blocks() {
        assert_eq!(d("# A\n## B\n- c\n  d\ne\n"), "heading\n  text \"A\"\nsubheading\n  text \"B\"\nitem\n  text \"c d e\"\n");
        assert_eq!(d("### x\n1. y\n"), "paragraph\n  text \"### x 1. y\"\n");
        assert_eq!(d("```\n```\n"), "");
    }

    #[test]
    fn numbers_round_to_sixteenths() {
        let n = |s: &str| number(s.as_bytes(), &mut 0);
        assert_eq!(n("1"), Some(16));
        assert_eq!(n(".03125"), Some(1), "a half rounds away from zero");
        assert_eq!(n("-.03125"), Some(-1));
        assert_eq!(n(".03124"), Some(0));
        assert_eq!(n("5."), Some(80));
        assert_eq!(n("."), None);
        assert_eq!(n("-"), None);
        assert_eq!(n(&"1".repeat(33)), None, "over 32 characters");
    }

    #[test]
    fn paths() {
        let p = path_string(b"M10-5L.5.5").unwrap();
        assert_eq!(p[0].segs, [Seg::Move(160, -80), Seg::Line(8, 8)]);
        assert!(path_string(b"L1 1").is_none(), "the first command is M");
        assert!(path_string(b"M0 0 L1 1 2").is_none());
        assert_eq!(path_string(b"fill #fff").unwrap(), []);
        assert!(path_string(b"M0 0 L2047 -2047").is_some());
        assert!(path_string(b"M0 0 L2047.04 0").is_none());
        // Ranges apply to the numbers as written, before rounding.
        assert!(path_string(b"M0 0 L2047.01 0").is_none(), "rounds to 2047 but is beyond it");
        assert!(path_string(b"M0 0 L-2047.01 0").is_none());
        assert!(path_string(b"M0 0 L2046.99 0").is_some(), "rounds to 2047, within it");
        assert!(path_string(b"M2047 0 l0.001 0").is_none(), "relative: the point as written");
        assert!(path_string(b"M2047 0 l-0.001 0").is_some());
        assert!(path_string(b"M0 0 H2047.0000000001").is_none());
        assert!(path_string(b"stroke fg 2047.001 M0 0 L1 1").is_none());
        assert!(path_string(b"stroke fg -0.001 M0 0 L1 1").is_none());
        assert!(path_string(b"stroke fg -0 M0 0 L1 1").is_some());
        // A word follows a separator or starts the string; a command letter
        // may follow a number directly.
        assert!(path_string(b"M0 0 L5 5fill #fff").is_none());
        assert!(path_string(b"M0 0 L5 5Zfill #fff").is_none());
        assert!(path_string(b"M0 0 L5 5Z fill #fff").is_some());
        assert!(path_string(b"M0 0L5 5L6 6").is_some());
    }

    #[test]
    fn decided_details() {
        // A byte order mark first is not shown; the first line's marker counts.
        assert_eq!(d("\u{feff}# A\n"), "heading\n  text \"A\"\n");
        // A lone CR is a blank, shown as a space; a CR before an LF is not shown.
        assert_eq!(d("a\rb\r\nc\n"), "paragraph\n  text \"a b c\"\n");
        // Figure sizes: at most 4 digits.
        assert_eq!(figure_size(b"path 0100 1024"), Some(Some((100, 1024))));
        assert_eq!(figure_size(b"path 00100 10"), Some(None));
        // A cell's limits count `\|` as two bytes.
        let span = format!("| `{}\\|` |\n", "x".repeat(199));
        assert!(!d(&span).contains("code"), "201 bytes as written");
        let span = format!("| `{}\\|` |\n", "x".repeat(198));
        assert!(d(&span).contains(&format!("code \"{}|\"", "x".repeat(198))));
    }
}
