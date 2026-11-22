//! The stroke fonts for the terminal: each character as lines the beam
//! draws, as a vector monitor's character generator does, not as a bitmap.
//! Three fonts ([`Font`], chosen by `TERSE_MAME_FONT`):
//!
//! - **arcade** (the default, [`crate::arcade`]): a vector monitor's
//!   character generator, a few long straight strokes per letter on a small
//!   grid; original glyphs.
//! - **hershey**: the Hershey Roman Simplex font, a plotter's curves.
//! - **drafting**: Hershey's strokes simplified (fewer, longer segments) and
//!   inclined, like single-stroke engineering lettering.
//!
//! In every font the box-drawing characters and block elements are the
//! ones below. For the hershey and drafting fonts, printable ASCII is the
//! Hershey Roman Simplex font
//! (`third_party/hershey/futural.jhf`: "The Hershey Fonts were originally
//! created by Dr. A. V. Hershey while working at the U. S. National Bureau
//! of Standards. The format of the Font data in this distribution was
//! originally created by James Hurt, Cognition, Inc."). The rest of the
//! terminal's basic repertoire (SPEC §terminal.text, `vectors/basic-text.txt`)
//! is drawn here: Latin-1's letters as Hershey letters with accents made of
//! strokes, its signs from Hershey glyphs and a few strokes, the box-drawing
//! characters as lines from the cell's centre to its edges, the block
//! elements as rasters of horizontal strokes (how a beam fills an area),
//! the quotation marks, dashes, bullet, ellipsis and minus. Anything else
//! is the replacement character's diamond.
//!
//! Coordinates: a cell is 8 × 16 screen units (SPEC §terminal: 80 × 30
//! cells on the 640 × 480 screen). Hershey's glyphs have their cap height
//! from y = −12 to the baseline at 9 and descend to 16; they are scaled by
//! 10/21 vertically (a cap height of 10 units, the baseline 12.5 units from
//! the cell's top) and by 0.4 across, less for a glyph whose ink would be
//! wider than 6.8 units, and centred in the cell.

use std::sync::OnceLock;

/// A polyline in Hershey coordinates (y down).
pub type Poly = Vec<(f32, f32)>;

/// One stroke in screen units within the cell, relative to its top left:
/// from, to, the width in screen units and a brightness factor (1 full).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub width: f32,
    pub level: f32,
}

/// A stroke font (`TERSE_MAME_FONT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Arcade,
    Hershey,
    Drafting,
}

impl Font {
    pub const ALL: [Font; 3] = [Font::Arcade, Font::Hershey, Font::Drafting];

    pub fn name(self) -> &'static str {
        match self {
            Font::Arcade => "arcade",
            Font::Hershey => "hershey",
            Font::Drafting => "drafting",
        }
    }

    pub fn parse(s: &str) -> Option<Font> {
        Font::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(s.trim()))
    }

    /// `TERSE_MAME_FONT` (arcade, hershey or drafting); arcade when unset,
    /// empty or not a font's name (which is said on stderr).
    pub fn from_env() -> Font {
        match std::env::var("TERSE_MAME_FONT") {
            Ok(v) if !v.trim().is_empty() => Font::parse(&v).unwrap_or_else(|| {
                eprintln!("terse-dev-mame: TERSE_MAME_FONT={v:?} is not arcade, hershey or drafting: using arcade");
                Font::Arcade
            }),
            _ => Font::Arcade,
        }
    }

    /// The font of this process: [`Font::from_env`], read once.
    pub fn current() -> Font {
        static F: OnceLock<Font> = OnceLock::new();
        *F.get_or_init(Font::from_env)
    }
}

pub const CELL_W: f32 = 8.0;
pub const CELL_H: f32 = 16.0;
const SY: f32 = 10.0 / 21.0;
const SX: f32 = 0.4;
const MAX_INK: f32 = 6.8;
const BASELINE: f32 = 12.5;

const JHF: &str = include_str!("../../../third_party/hershey/futural.jhf");

/// The Hershey glyphs of ASCII 32 to 126, centred on their advance's middle.
fn ascii() -> &'static [Vec<Poly>] {
    static GLYPHS: OnceLock<Vec<Vec<Poly>>> = OnceLock::new();
    GLYPHS.get_or_init(|| JHF.lines().take(95).map(parse_jhf).collect())
}

/// One line of a .jhf file: number (5 columns), vertex count (3), left and
/// right (1 each, relative to 'R'), then pairs, " R" lifting the pen.
fn parse_jhf(line: &str) -> Vec<Poly> {
    let b = line.as_bytes();
    if b.len() < 10 {
        return Vec::new();
    }
    let v = |c: u8| c as f32 - b'R' as f32;
    let (left, right) = (v(b[8]), v(b[9]));
    let mid = (left + right) / 2.0;
    let mut polys = Vec::new();
    let mut cur: Poly = Vec::new();
    for p in b[10..].chunks_exact(2) {
        if p == b" R" {
            if cur.len() > 0 {
                polys.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push((v(p[0]) - mid, v(p[1])));
    }
    if !cur.is_empty() {
        polys.push(cur);
    }
    polys
}

fn glyph(c: char) -> Vec<Poly> {
    let i = c as u32;
    if (32..127).contains(&i) { ascii()[(i - 32) as usize].clone() } else { Vec::new() }
}

/// Polylines moved and scaled: (x, y) → (x × sx + dx, y × sy + dy).
fn tf(polys: Vec<Poly>, sx: f32, sy: f32, dx: f32, dy: f32) -> Vec<Poly> {
    polys.into_iter().map(|p| p.into_iter().map(|(x, y)| (x * sx + dx, y * sy + dy)).collect()).collect()
}

fn ink_x(polys: &[Poly]) -> (f32, f32) {
    let xs = polys.iter().flatten().map(|p| p.0);
    let lo = xs.clone().fold(f32::INFINITY, f32::min);
    let hi = xs.fold(f32::NEG_INFINITY, f32::max);
    if lo.is_finite() { (lo, hi) } else { (0.0, 0.0) }
}

fn dot(x: f32, y: f32) -> Poly {
    vec![(x - 0.5, y), (x + 0.5, y)]
}

fn circle(x: f32, y: f32, r: f32, n: usize) -> Poly {
    (0..=n).map(|k| {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        (x + r * a.cos(), y + r * a.sin())
    })
    .collect()
}

/// The accents, above a letter whose top is at `t` (−12 capitals, −5 small
/// letters); `t` = −10 for an accent alone.
#[derive(Clone, Copy)]
enum Accent {
    Grave,
    Acute,
    Circumflex,
    Tilde,
    Diaeresis,
    Ring,
    Cedilla,
}

fn accent(a: Accent, t: f32) -> Vec<Poly> {
    match a {
        Accent::Grave => vec![vec![(-2.0, t - 5.0), (1.0, t - 2.0)]],
        Accent::Acute => vec![vec![(2.0, t - 5.0), (-1.0, t - 2.0)]],
        Accent::Circumflex => vec![vec![(-3.0, t - 2.0), (0.0, t - 5.0), (3.0, t - 2.0)]],
        Accent::Tilde => vec![vec![(-4.0, t - 2.5), (-2.5, t - 4.5), (-0.5, t - 3.5), (1.5, t - 2.5), (3.0, t - 4.5)]],
        Accent::Diaeresis => vec![dot(-3.0, t - 3.0), dot(3.0, t - 3.0)],
        Accent::Ring => vec![circle(0.0, t - 3.5, 1.8, 10)],
        Accent::Cedilla => vec![vec![(0.0, 9.0), (0.0, 11.0), (2.0, 12.0), (1.0, 14.0), (-1.5, 14.0)]],
    }
}

/// A letter with an accent.
fn accented(base: char, a: Accent) -> Vec<Poly> {
    let mut g = match base {
        'ı' => vec![vec![(0.0, -5.0), (0.0, 9.0)]],
        c => glyph(c),
    };
    let t = if base.is_uppercase() { -12.0 } else { -5.0 };
    g.extend(accent(a, t));
    g
}

/// A glyph turned half round about its middle (¡, ¿, ‘).
fn turned(c: char, centre: f32) -> Vec<Poly> {
    tf(glyph(c), -1.0, -1.0, 0.0, 2.0 * centre)
}

/// A glyph made small: scale `s`, centred on (0, `y`).
fn small(c: char, s: f32, dx: f32, y: f32) -> Vec<Poly> {
    tf(glyph(c), s, s, dx, y + 1.5 * s)
}

/// The polylines of a character in Hershey coordinates, or None if it is
/// not one this font draws.
fn hershey(c: char) -> Option<Vec<Poly>> {
    use Accent::*;
    let mut g: Vec<Poly>;
    Some(match c {
        // Hershey's I is a bare stem, its l too: serifs tell them apart.
        'I' => vec![vec![(0.0, -12.0), (0.0, 9.0)], vec![(-3.0, -12.0), (3.0, -12.0)], vec![(-3.0, 9.0), (3.0, 9.0)]],
        ' '..='~' => glyph(c),
        '\u{a0}' => Vec::new(),
        '¡' => turned('!', -1.5),
        '¿' => turned('?', -1.5),
        '¢' => {
            g = glyph('c');
            g.push(vec![(0.5, -9.0), (0.5, 13.0)]);
            g
        }
        '£' => vec![
            vec![(6.0, -9.0), (5.0, -11.0), (3.0, -12.0), (0.0, -12.0), (-2.0, -11.0), (-3.0, -9.0), (-3.0, 6.0), (-4.0, 8.0), (-6.0, 9.0), (7.0, 9.0)],
            vec![(-7.0, -2.0), (3.0, -2.0)],
        ],
        '¤' => {
            let mut g = vec![circle(0.0, -1.5, 4.0, 14)];
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                g.push(vec![(x * 3.0, -1.5 + y * 3.0), (x * 6.0, -1.5 + y * 6.0)]);
            }
            g
        }
        '¥' => {
            g = glyph('Y');
            g.push(vec![(-5.0, 0.0), (5.0, 0.0)]);
            g.push(vec![(-5.0, 3.5), (5.0, 3.5)]);
            g
        }
        '¦' => vec![vec![(0.0, -14.0), (0.0, -3.0)], vec![(0.0, 2.0), (0.0, 13.0)]],
        '§' => {
            let mut g = small('S', 0.55, 0.0, -6.0);
            g.extend(small('S', 0.55, 0.0, 3.0));
            g
        }
        '¨' => accent(Diaeresis, -10.0),
        '©' | '®' => {
            let mut g = vec![circle(0.0, -1.5, 9.5, 28)];
            g.extend(small(if c == '©' { 'C' } else { 'R' }, 0.5, 0.0, -1.5));
            g
        }
        'ª' | 'º' => {
            let mut g = small(if c == 'ª' { 'a' } else { 'o' }, 0.6, 0.0, -9.0);
            g.push(vec![(-4.0, -1.0), (4.0, -1.0)]);
            g
        }
        '«' | '»' => {
            let k = if c == '«' { '<' } else { '>' };
            let mut g = small(k, 0.5, -3.0, 1.0);
            g.extend(small(k, 0.5, 3.0, 1.0));
            g
        }
        '¬' => vec![vec![(-7.0, -2.0), (7.0, -2.0), (7.0, 3.0)]],
        '\u{ad}' => glyph('-'),
        '¯' => vec![vec![(-5.0, -14.0), (5.0, -14.0)]],
        '°' => vec![circle(0.0, -9.0, 2.5, 12)],
        '±' => {
            let mut g = tf(glyph('+'), 0.8, 0.8, 0.0, -3.0);
            g.push(vec![(-7.0, 7.0), (7.0, 7.0)]);
            g
        }
        '¹' | '²' | '³' => small(['1', '2', '3'][match c {
            '¹' => 0,
            '²' => 1,
            _ => 2,
        }], 0.5, 0.0, -7.5),
        '´' => accent(Acute, -10.0),
        'µ' => {
            g = glyph('u');
            let (lo, _) = ink_x(&g);
            g.push(vec![(lo, -5.0), (lo, 16.0)]);
            g
        }
        '¶' => vec![
            vec![(2.0, -12.0), (2.0, 9.0)],
            vec![(6.0, -12.0), (6.0, 9.0)],
            vec![(6.0, -12.0), (-1.0, -12.0), (-4.0, -11.0), (-5.0, -8.0), (-4.0, -5.0), (-1.0, -4.0), (2.0, -4.0)],
        ],
        '·' => vec![dot(0.0, -1.5)],
        '¸' => accent(Cedilla, 0.0),
        '¼' | '½' | '¾' => {
            let (n, d) = match c {
                '¼' => ('1', '4'),
                '½' => ('1', '2'),
                _ => ('3', '4'),
            };
            let mut g = small(n, 0.42, -4.5, -6.5);
            g.extend(small(d, 0.42, 4.5, 4.0));
            g.push(vec![(6.0, -12.0), (-6.0, 9.0)]);
            g
        }
        'À' | 'à' => accented(base(c, 'A'), Grave),
        'Á' | 'á' => accented(base(c, 'A'), Acute),
        'Â' | 'â' => accented(base(c, 'A'), Circumflex),
        'Ã' | 'ã' => accented(base(c, 'A'), Tilde),
        'Ä' | 'ä' => accented(base(c, 'A'), Diaeresis),
        'Å' | 'å' => accented(base(c, 'A'), Ring),
        'Æ' => vec![
            vec![(-9.0, 9.0), (1.0, -12.0), (9.0, -12.0)],
            vec![(1.0, -12.0), (1.0, 9.0), (9.0, 9.0)],
            vec![(1.0, -2.0), (7.0, -2.0)],
            vec![(-5.0, 1.0), (1.0, 1.0)],
        ],
        'æ' => {
            let mut g = tf(glyph('a'), 0.6, 1.0, -3.5, 0.0);
            g.extend(tf(glyph('e'), 0.6, 1.0, 3.5, 0.0));
            g
        }
        'Ç' | 'ç' => accented(base(c, 'C'), Cedilla),
        'È' | 'è' => accented(base(c, 'E'), Grave),
        'É' | 'é' => accented(base(c, 'E'), Acute),
        'Ê' | 'ê' => accented(base(c, 'E'), Circumflex),
        'Ë' | 'ë' => accented(base(c, 'E'), Diaeresis),
        'Ì' => accented('I', Grave),
        'Í' => accented('I', Acute),
        'Î' => accented('I', Circumflex),
        'Ï' => accented('I', Diaeresis),
        'ì' => accented('ı', Grave),
        'í' => accented('ı', Acute),
        'î' => accented('ı', Circumflex),
        'ï' => accented('ı', Diaeresis),
        'Ð' => {
            g = glyph('D');
            let (lo, _) = ink_x(&g);
            g.push(vec![(lo - 2.5, -1.5), (lo + 4.0, -1.5)]);
            g
        }
        'ð' => {
            g = glyph('o');
            g.push(vec![(4.0, -2.0), (3.0, -7.0), (0.0, -11.0), (-3.0, -13.0)]);
            g.push(vec![(-3.0, -7.0), (4.0, -11.0)]);
            g
        }
        'Ñ' | 'ñ' => accented(base(c, 'N'), Tilde),
        'Ò' | 'ò' => accented(base(c, 'O'), Grave),
        'Ó' | 'ó' => accented(base(c, 'O'), Acute),
        'Ô' | 'ô' => accented(base(c, 'O'), Circumflex),
        'Õ' | 'õ' => accented(base(c, 'O'), Tilde),
        'Ö' | 'ö' => accented(base(c, 'O'), Diaeresis),
        '×' => vec![vec![(-5.0, -6.5), (5.0, 3.5)], vec![(5.0, -6.5), (-5.0, 3.5)]],
        '÷' => vec![vec![(-7.0, -1.5), (7.0, -1.5)], dot(0.0, -6.0), dot(0.0, 3.0)],
        'Ø' => {
            g = glyph('O');
            g.push(vec![(-8.0, 11.0), (8.0, -14.0)]);
            g
        }
        'ø' => {
            g = glyph('o');
            g.push(vec![(-6.0, 11.0), (6.0, -7.0)]);
            g
        }
        'Ù' | 'ù' => accented(base(c, 'U'), Grave),
        'Ú' | 'ú' => accented(base(c, 'U'), Acute),
        'Û' | 'û' => accented(base(c, 'U'), Circumflex),
        'Ü' | 'ü' => accented(base(c, 'U'), Diaeresis),
        'Ý' | 'ý' => accented(base(c, 'Y'), Acute),
        'ÿ' => accented('y', Diaeresis),
        'Þ' => vec![
            vec![(-6.0, -12.0), (-6.0, 9.0)],
            vec![(-6.0, -7.0), (2.0, -7.0), (5.0, -6.0), (6.0, -4.0), (6.0, -1.0), (5.0, 1.0), (2.0, 2.0), (-6.0, 2.0)],
        ],
        'þ' => {
            g = glyph('p');
            let (lo, _) = ink_x(&g);
            g.push(vec![(lo, -12.0), (lo, -5.0)]);
            g
        }
        'ß' => vec![vec![
            (-6.0, 9.0),
            (-6.0, -7.0),
            (-5.0, -10.0),
            (-3.0, -12.0),
            (0.0, -12.0),
            (2.0, -11.0),
            (3.0, -9.0),
            (3.0, -7.0),
            (1.0, -4.0),
            (-1.0, -3.0),
            (2.0, -2.0),
            (4.0, 0.0),
            (5.0, 3.0),
            (4.0, 6.0),
            (2.0, 8.0),
            (-1.0, 9.0),
        ]],
        '–' => vec![vec![(-6.0, 0.0), (6.0, 0.0)]],
        '—' => vec![vec![(-10.0, 0.0), (10.0, 0.0)]],
        '\u{2212}' => vec![vec![(-7.0, 0.0), (7.0, 0.0)]],
        '’' => glyph('\''),
        '‘' => turned('\'', -9.0),
        '”' => {
            let mut g = tf(glyph('\''), 1.0, 1.0, -2.5, 0.0);
            g.extend(tf(glyph('\''), 1.0, 1.0, 2.5, 0.0));
            g
        }
        '“' => {
            let mut g = tf(turned('\'', -9.0), 1.0, 1.0, -2.5, 0.0);
            g.extend(tf(turned('\'', -9.0), 1.0, 1.0, 2.5, 0.0));
            g
        }
        '•' => vec![circle(0.0, -1.5, 2.5, 10), circle(0.0, -1.5, 1.3, 8), dot(0.0, -1.5)],
        '…' => vec![dot(-6.0, 8.0), dot(0.0, 8.0), dot(6.0, 8.0)],
        '\u{fffd}' => {
            let mut g = vec![vec![(0.0, -14.0), (9.0, -1.5), (0.0, 11.0), (-9.0, -1.5), (0.0, -14.0)]];
            g.extend(small('?', 0.45, 0.0, -1.5));
            g
        }
        _ => return None,
    })
}

/// The unaccented letter of an accented one, in its case.
fn base(c: char, upper: char) -> char {
    if c.is_uppercase() { upper } else { upper.to_ascii_lowercase() }
}

/// The strokes of a character in a cell (the top left at (0, 0)) in this
/// process's font ([`Font::current`]); a wide character spans two cells.
/// Nothing for a blank.
pub fn strokes(code: u32, wide: bool, out: &mut Vec<Stroke>) {
    strokes_in(Font::current(), code, wide, out)
}

/// [`strokes`] in a given font.
pub fn strokes_in(font: Font, code: u32, wide: bool, out: &mut Vec<Stroke>) {
    let Some(c) = char::from_u32(code) else { return placeholder(font, wide, out) };
    if c == ' ' {
        return;
    }
    if (0x2500..=0x259F).contains(&code) {
        return boxes(code, out);
    }
    if wide {
        return placeholder(font, wide, out);
    }
    let drawn = match font {
        Font::Arcade => crate::arcade::cell_strokes(c, CELL_W, out),
        Font::Hershey | Font::Drafting => match hershey(c) {
            Some(polys) => {
                cell_strokes(&polys, CELL_W, font == Font::Drafting, out);
                true
            }
            None => false,
        },
    };
    if !drawn {
        placeholder(font, wide, out)
    }
}

/// The drafting font's slant: x moves right by this much per unit up.
const SLANT: f32 = 0.22;
/// The drafting font's widest ink, its slant included.
const MAX_INK_DRAFTING: f32 = 6.6;
/// How far a simplified drafting polyline may stray from Hershey's, in
/// Hershey units (about half a screen unit).
const SIMPLIFY: f32 = 1.0;

/// Ramer–Douglas–Peucker: the points of `p` that keep it within `eps`.
fn simplify(p: &[(f32, f32)], eps: f32) -> Poly {
    if p.len() < 3 {
        return p.to_vec();
    }
    let (a, b) = (p[0], p[p.len() - 1]);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let dist = |q: (f32, f32)| {
        if len < 1e-6 { ((q.0 - a.0).powi(2) + (q.1 - a.1).powi(2)).sqrt() } else { ((q.0 - a.0) * dy - (q.1 - a.1) * dx).abs() / len }
    };
    let (i, d) = p[1..p.len() - 1].iter().enumerate().map(|(i, &q)| (i + 1, dist(q))).fold((0, -1.0), |m, x| if x.1 > m.1 { x } else { m });
    if d <= eps {
        return vec![a, b];
    }
    let mut left = simplify(&p[..=i], eps);
    left.pop();
    left.extend(simplify(&p[i..], eps));
    left
}

/// The drafting font: Hershey's polylines simplified, then inclined about
/// the capitals' middle and fitted to the cell's width.
fn drafting_strokes(polys: &[Poly], cell_w: f32, out: &mut Vec<Stroke>) {
    let polys: Vec<Poly> = polys.iter().map(|p| simplify(p, SIMPLIFY)).collect();
    let y_of = |y: f32| BASELINE + (y - 9.0) * SY;
    let mid_y = BASELINE - 5.0;
    let lean = |x: f32, y: f32, sx: f32| x * sx + (mid_y - y_of(y)) * SLANT;
    let bounds = |sx: f32| {
        polys.iter().flatten().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &(x, y)| {
            let v = lean(x, y, sx);
            (lo.min(v), hi.max(v))
        })
    };
    let max = MAX_INK_DRAFTING * cell_w / CELL_W;
    let mut sx = SX * 0.92;
    let (mut lo, mut hi) = bounds(sx);
    for _ in 0..20 {
        if !lo.is_finite() || hi - lo <= max {
            break;
        }
        sx *= 0.94;
        (lo, hi) = bounds(sx);
    }
    // The ink's centre on the cell's centre.
    let centre = if lo.is_finite() { (lo + hi) / 2.0 } else { 0.0 };
    let map = |(x, y): (f32, f32)| (cell_w / 2.0 + lean(x, y, sx) - centre, y_of(y));
    push(&polys, map, out);
}

fn push(polys: &[Poly], map: impl Fn((f32, f32)) -> (f32, f32), out: &mut Vec<Stroke>) {
    for p in polys {
        if p.len() == 1 {
            // A point: a short stroke (never one of no length).
            let (x, y) = map(p[0]);
            out.push(Stroke { x0: x, y0: y - 0.25, x1: x, y1: y + 0.25, width: 1.0, level: 1.0 });
        }
        for w in p.windows(2) {
            let ((x0, y0), (x1, y1)) = (map(w[0]), map(w[1]));
            if (x0, y0) != (x1, y1) {
                out.push(Stroke { x0, y0, x1, y1, width: 1.0, level: 1.0 });
            }
        }
    }
}

fn cell_strokes(polys: &[Poly], cell_w: f32, drafting: bool, out: &mut Vec<Stroke>) {
    if drafting {
        return drafting_strokes(polys, cell_w, out);
    }
    let (lo, hi) = ink_x(polys);
    let sx = if (hi - lo) * SX > MAX_INK * cell_w / CELL_W { MAX_INK * cell_w / CELL_W / (hi - lo) } else { SX };
    let mid = (lo + hi) / 2.0;
    // Centre the ink when the glyph had to be narrowed, else the advance.
    let centre = if sx < SX { mid } else { 0.0 };
    let map = |(x, y): (f32, f32)| (cell_w / 2.0 + (x - centre) * sx, BASELINE + (y - 9.0) * SY);
    push(polys, map, out);
}

/// The replacement character, also for a character a font lacks (two
/// cells wide for a wide one).
fn placeholder(font: Font, wide: bool, out: &mut Vec<Stroke>) {
    let cell_w = if wide { 2.0 * CELL_W } else { CELL_W };
    if font == Font::Arcade {
        crate::arcade::cell_strokes('\u{fffd}', cell_w, out);
    } else {
        cell_strokes(&hershey('\u{fffd}').unwrap_or_default(), cell_w, font == Font::Drafting, out);
    }
}

/// The arms of the box-drawing characters U+2500 to U+257F, four digits a
/// character (up, down, left, right: 0 none, 1 light, 2 heavy, 3 double);
/// `-` for the dashed lines and the arcs and diagonals, drawn on their own.
const ARMS: [&str; 128] = [
    "0011", "0022", "1100", "2200", "-", "-", "-", "-", "-", "-", "-", "-", "0101", "0102", "0201", "0202", // 2500
    "0110", "0120", "0210", "0220", "1001", "1002", "2001", "2002", "1010", "1020", "2010", "2020", "1101", "1102", "2101", "1201", // 2510
    "2201", "2102", "1202", "2202", "1110", "1120", "2110", "1210", "2210", "2120", "1220", "2220", "0111", "0121", "0112", "0122", // 2520
    "0211", "0221", "0212", "0222", "1011", "1021", "1012", "1022", "2011", "2021", "2012", "2022", "1111", "1121", "1112", "1122", // 2530
    "2111", "1211", "2211", "2121", "2112", "1221", "1212", "2122", "1222", "2221", "2212", "2222", "-", "-", "-", "-", // 2540
    "0033", "3300", "0103", "0301", "0303", "0130", "0310", "0330", "1003", "3001", "3003", "1030", "3010", "3030", "1103", "3301", // 2550
    "3303", "1130", "3310", "3330", "0133", "0311", "0333", "1033", "3011", "3033", "1133", "3311", "3333", "-", "-", "-", // 2560
    "-", "-", "-", "-", "0010", "1000", "0001", "0100", "0020", "2000", "0002", "0200", "0012", "1200", "0021", "2100", // 2570
];

fn line(out: &mut Vec<Stroke>, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) {
    out.push(Stroke { x0, y0, x1, y1, width, level: 1.0 });
}

/// A horizontal raster of strokes one unit apart filling a rectangle of
/// the cell: how a beam fills an area. `every` > 1 leaves lines out (the
/// shades), `level` dims them.
fn raster(out: &mut Vec<Stroke>, x0: f32, y0: f32, x1: f32, y1: f32, every: usize, level: f32) {
    if x1 - x0 <= 1.0 {
        // One unit wide (the one-eighth blocks): one vertical stroke, not
        // rows of no length.
        let x = (x0 + x1) / 2.0;
        out.push(Stroke { x0: x, y0: y0 + 0.5, x1: x, y1: y1 - 0.5, width: 1.0, level });
        return;
    }
    let rows = (y1 - y0).round() as usize;
    for k in (0..rows).filter(|k| k % every == 0) {
        let y = y0 + k as f32 + 0.5;
        out.push(Stroke { x0: x0 + 0.5, y0: y, x1: x1 - 0.5, y1: y, width: 1.0, level });
    }
}

fn boxes(code: u32, out: &mut Vec<Stroke>) {
    let (w, h, cx, cy) = (CELL_W, CELL_H, CELL_W / 2.0, CELL_H / 2.0);
    if code >= 0x2580 {
        // Block elements: rectangles of the cell, in eighths.
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| (x0 * w / 8.0, y0 * h / 8.0, x1 * w / 8.0, y1 * h / 8.0);
        let (ul, ur, ll, lr) = (r(0.0, 0.0, 4.0, 4.0), r(4.0, 0.0, 8.0, 4.0), r(0.0, 4.0, 4.0, 8.0), r(4.0, 4.0, 8.0, 8.0));
        let rects: Vec<(f32, f32, f32, f32)> = match code {
            0x2580 => vec![r(0.0, 0.0, 8.0, 4.0)],
            0x2581..=0x2588 => vec![r(0.0, 8.0 - (code - 0x2580) as f32, 8.0, 8.0)],
            0x2589..=0x258F => vec![r(0.0, 0.0, 8.0 - (code - 0x2588) as f32, 8.0)],
            0x2590 => vec![r(4.0, 0.0, 8.0, 8.0)],
            0x2591..=0x2593 => {
                let (every, level) = [(4, 0.6), (2, 0.8), (1, 0.8)][(code - 0x2591) as usize];
                return raster(out, 0.0, 0.0, w, h, every, level);
            }
            0x2594 => vec![r(0.0, 0.0, 8.0, 1.0)],
            0x2595 => vec![r(7.0, 0.0, 8.0, 8.0)],
            0x2596 => vec![ll],
            0x2597 => vec![lr],
            0x2598 => vec![ul],
            0x2599 => vec![ul, ll, lr],
            0x259A => vec![ul, lr],
            0x259B => vec![ul, ur, ll],
            0x259C => vec![ul, ur, lr],
            0x259D => vec![ur],
            0x259E => vec![ur, ll],
            _ => vec![ur, ll, lr],
        };
        for (x0, y0, x1, y1) in rects {
            raster(out, x0, y0, x1, y1, 1, 1.0);
        }
        return;
    }
    let arms = ARMS[(code - 0x2500) as usize].as_bytes();
    if arms[0] == b'-' {
        match code {
            // Dashed: 2504–250B triple and quadruple, 254C–254F double.
            0x2504..=0x250B | 0x254C..=0x254F => {
                let (n, k) = if code >= 0x254C { (2, code - 0x254C) } else { (3 + (code - 0x2504) / 4, (code - 0x2504) % 4) };
                let (vertical, heavy) = (k >= 2, k % 2 == 1);
                let width = if heavy { 2.0 } else { 1.0 };
                let len = if vertical { h } else { w };
                let step = len / n as f32;
                for i in 0..n {
                    let (a, b) = (i as f32 * step + step * 0.2, (i + 1) as f32 * step - step * 0.2);
                    if vertical { line(out, cx, a, cx, b, width) } else { line(out, a, cy, b, cy, width) }
                }
            }
            // Arcs: a quarter of an ellipse from the middle of one edge to
            // the middle of another, about the cell's corner between them.
            0x256D..=0x2570 => {
                let (ox, oy) = [(w, h), (0.0, h), (0.0, 0.0), (w, 0.0)][(code - 0x256D) as usize];
                let n = 8;
                // From the middle of the horizontal edge (ox ± w/2, oy) to the
                // middle of the vertical edge (ox, oy ∓ h/2).
                let (sx, sy) = (if ox > 0.0 { -1.0 } else { 1.0 }, if oy > 0.0 { -1.0 } else { 1.0 });
                let pts: Vec<(f32, f32)> = (0..=n)
                    .map(|k| {
                        let a = k as f32 / n as f32 * std::f32::consts::FRAC_PI_2;
                        (ox + sx * cx * a.cos(), oy + sy * cy * a.sin())
                    })
                    .collect();
                // (ox + sx·cx, oy) at a = 0: the middle of the horizontal
                // edge; (ox, oy + sy·cy) at a = π/2: the vertical edge's.
                for p in pts.windows(2) {
                    line(out, p[0].0, p[0].1, p[1].0, p[1].1, 1.0);
                }
            }
            0x2571 => line(out, w, 0.0, 0.0, h, 1.0),
            0x2572 => line(out, 0.0, 0.0, w, h, 1.0),
            _ => {
                line(out, w, 0.0, 0.0, h, 1.0);
                line(out, 0.0, 0.0, w, h, 1.0);
            }
        }
        return;
    }
    let [up, down, left, right] = [arms[0] - b'0', arms[1] - b'0', arms[2] - b'0', arms[3] - b'0'];
    // An arm: from the centre (or, double, from just across it) to an edge.
    let arm = |out: &mut Vec<Stroke>, kind: u8, ex: f32, ey: f32| {
        let vertical = ex == cx;
        match kind {
            1 | 2 => line(out, cx, cy, ex, ey, kind as f32),
            3 => {
                for d in [-1.0, 1.0] {
                    if vertical {
                        line(out, cx + d, cy - (ey - cy).signum(), cx + d, ey, 1.0)
                    } else {
                        line(out, cx - (ex - cx).signum(), cy + d, ex, cy + d, 1.0)
                    }
                }
            }
            _ => {}
        }
    };
    // A straight line through: one stroke, not two meeting in the middle.
    if up == down && up != 0 {
        if up == 3 {
            line(out, cx - 1.0, 0.0, cx - 1.0, h, 1.0);
            line(out, cx + 1.0, 0.0, cx + 1.0, h, 1.0);
        } else {
            line(out, cx, 0.0, cx, h, up as f32);
        }
    } else {
        arm(out, up, cx, 0.0);
        arm(out, down, cx, h);
    }
    if left == right && left != 0 {
        if left == 3 {
            line(out, 0.0, cy - 1.0, w, cy - 1.0, 1.0);
            line(out, 0.0, cy + 1.0, w, cy + 1.0, 1.0);
        } else {
            line(out, 0.0, cy, w, cy, left as f32);
        }
    } else {
        arm(out, left, 0.0, cy);
        arm(out, right, w, cy);
    }
}

/// Every character of the terminal's basic repertoire (SPEC §terminal.text)
/// that this font draws as itself (not as the replacement diamond).
/// (In every font.)
pub fn covers(code: u32) -> bool {
    Font::ALL.into_iter().all(|f| covers_in(f, code))
}

/// [`covers`] for one font.
pub fn covers_in(font: Font, code: u32) -> bool {
    (0x2500..=0x259F).contains(&code)
        || char::from_u32(code).is_some_and(|c| match font {
            Font::Arcade => crate::arcade::glyph(c).is_some(),
            Font::Hershey | Font::Drafting => hershey(c).is_some(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The basic repertoire, from `vectors/basic-text.txt`'s ranges.
    fn repertoire() -> Vec<u32> {
        let v = include_str!("../../../vectors/basic-text.txt");
        let mut codes = Vec::new();
        let mut inside = false;
        for line in v.lines() {
            let l = line.trim();
            if l.starts_with("repertoire basic-text") {
                inside = true;
            } else if inside && let Some(r) = l.strip_prefix("range ") {
                let (a, b) = r.split_once(' ').unwrap();
                let (a, b) = (u32::from_str_radix(a, 16).unwrap(), u32::from_str_radix(b, 16).unwrap());
                codes.extend(a..=b);
            } else if inside && !l.is_empty() && !l.starts_with("count") {
                break;
            }
        }
        codes
    }

    fn glyph(font: Font, code: u32) -> Vec<Stroke> {
        let mut s = Vec::new();
        strokes_in(font, code, false, &mut s);
        s
    }

    #[test]
    fn font_names() {
        for f in Font::ALL {
            assert_eq!(Font::parse(f.name()), Some(f));
        }
        assert_eq!(Font::parse(" Hershey "), Some(Font::Hershey));
        assert_eq!(Font::parse("courier"), None);
    }

    #[test]
    fn the_fonts_differ() {
        for c in "aAgR&".chars() {
            let g: Vec<Vec<Stroke>> = Font::ALL.iter().map(|&f| glyph(f, c as u32)).collect();
            assert_ne!(g[0], g[1], "{c}");
            assert_ne!(g[0], g[2], "{c}");
            assert_ne!(g[1], g[2], "{c}");
        }
        // The box drawing is the same in all.
        assert_eq!(glyph(Font::Arcade, 0x253C), glyph(Font::Hershey, 0x253C));
    }

    #[test]
    fn ascii_is_hershey() {
        assert_eq!(ascii().len(), 95);
        // 'H': two stems and a bar.
        assert_eq!(glyph(Font::Hershey, 'H' as u32).len(), 3);
        assert!(glyph(Font::Hershey, ' ' as u32).is_empty());
        assert!(glyph(Font::Arcade, ' ' as u32).is_empty());
    }

    #[test]
    fn basic_repertoire_is_covered() {
        let codes = repertoire();
        assert_eq!(codes.len(), 361);
        for f in Font::ALL {
            let diamond = glyph(f, 0xFFFD);
            for &c in &codes {
                assert!(covers_in(f, c), "{} U+{c:04X}", f.name());
                let g = glyph(f, c);
                if c != 0x20 && c != 0xA0 {
                    assert!(!g.is_empty(), "{} U+{c:04X} blank", f.name());
                }
                if c != 0xFFFD {
                    assert_ne!(g, diamond, "{} U+{c:04X} is the replacement", f.name());
                }
            }
            // Not in the repertoire: the replacement.
            assert_eq!(glyph(f, 0x4E00), diamond);
        }
    }

    #[test]
    fn every_glyph_stays_in_its_cell() {
        for f in Font::ALL {
            for code in repertoire() {
                for st in glyph(f, code) {
                    for (x, y) in [(st.x0, st.y0), (st.x1, st.y1)] {
                        assert!((-0.01..=CELL_W + 0.01).contains(&x) && (-0.01..=CELL_H + 0.01).contains(&y), "{} U+{code:04X} {st:?}", f.name());
                    }
                }
            }
            let mut s = Vec::new();
            strokes_in(f, 0x4E00, true, &mut s);
            assert!(!s.is_empty());
            for st in s {
                assert!(st.x0.max(st.x1) <= 2.0 * CELL_W && st.x0.min(st.x1) >= 0.0 && st.y0.max(st.y1) <= CELL_H, "{} wide {st:?}", f.name());
            }
        }
    }

    #[test]
    fn no_stroke_has_no_length() {
        for f in Font::ALL {
            for code in repertoire() {
                for st in glyph(f, code) {
                    assert!((st.x1 - st.x0).hypot(st.y1 - st.y0) > 0.1, "{} U+{code:04X} {st:?}", f.name());
                }
            }
        }
    }

    /// The arcade letters and digits keep to six pixels' width (1.5 to 6.5)
    /// and the cap height to the descender (2.5 to 15.5).
    #[test]
    fn arcade_letters_keep_to_the_grid() {
        for c in ('A'..='Z').chain('a'..='z').chain('0'..='9') {
            for st in glyph(Font::Arcade, c as u32) {
                for (x, y) in [(st.x0, st.y0), (st.x1, st.y1)] {
                    assert!((1.5..=6.5).contains(&x) && (2.5..=15.5).contains(&y), "{c} {st:?}");
                }
            }
        }
    }

    /// A straight line is one stroke: no polyline goes on in the same
    /// direction from a vertex (where the beam would make a bead).
    #[test]
    fn arcade_beads_only_at_corners() {
        for code in repertoire().into_iter().filter(|c| !(0x2500..=0x259F).contains(c)) {
            let g = glyph(Font::Arcade, code);
            for w in g.windows(2) {
                let (a, b) = (w[0], w[1]);
                if (a.x1, a.y1) != (b.x0, b.y0) {
                    continue;
                }
                let (ax, ay, bx, by) = (a.x1 - a.x0, a.y1 - a.y0, b.x1 - b.x0, b.y1 - b.y0);
                let cross = ax * by - ay * bx;
                let dot = ax * bx + ay * by;
                assert!(cross.abs() > 1e-3 || dot < 0.0, "U+{code:04X} {a:?} {b:?}");
            }
        }
    }

    /// The pixels a glyph lights at 640 × 480 (one unit a pixel): those
    /// whose centre is within 0.6 of a stroke.
    fn pixels(s: &[Stroke]) -> Vec<bool> {
        let mut px = vec![false; 8 * 16];
        for (i, p) in px.iter_mut().enumerate() {
            let (x, y) = ((i % 8) as f32 + 0.5, (i / 8) as f32 + 0.5);
            *p = s.iter().any(|st| {
                let (dx, dy) = (st.x1 - st.x0, st.y1 - st.y0);
                let t = (((x - st.x0) * dx + (y - st.y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                (st.x0 + t * dx - x).hypot(st.y0 + t * dy - y) <= 0.6
            });
        }
        px
    }

    const CONFUSABLE: [&str; 16] = ["0O", "1l", "1I", "lI", "5S", "8B", "2Z", "uv", "ce", "il", "DO", "6G", "gq", "nh", "o0", "Il"];

    #[test]
    fn confusable_pairs_differ() {
        for f in Font::ALL {
            for p in CONFUSABLE {
                let mut c = p.chars();
                let (a, b) = (c.next().unwrap(), c.next().unwrap());
                assert_ne!(glyph(f, a as u32), glyph(f, b as u32), "{} {a} {b}", f.name());
            }
        }
        // The arcade font: at one pixel a unit they differ by several pixels.
        for p in CONFUSABLE {
            let mut c = p.chars();
            let (a, b) = (c.next().unwrap(), c.next().unwrap());
            let (pa, pb) = (pixels(&glyph(Font::Arcade, a as u32)), pixels(&glyph(Font::Arcade, b as u32)));
            let d = pa.iter().zip(&pb).filter(|(x, y)| x != y).count();
            // (n and h: the ascender's four pixels.)
            assert!(d >= 4, "{a} {b}: {d} pixels");
        }
    }
}

