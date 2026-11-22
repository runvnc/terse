//! The arcade font: the terminal's text as a vector monitor's character
//! generator drew it, a handful of long straight strokes per letter on a
//! small grid. Original glyphs, drawn for this crate (Apache-2.0).
//!
//! The grid, in screen units within the 8 × 16 cell: x from 0 to 5 is
//! 1.5 to 6.5 (the letter's ink, six pixels at 640 × 480, with two dark
//! pixels between letters); y = 0 is the capital and ascender height (2.5),
//! 4 the small letters' height (6.5), 5 the capitals' middle (7.5), 7 the
//! small letters' middle (9.5), 10 the baseline (12.5), 13 the descender
//! (15.5). Whole grid units fall on pixel centres at 640 × 480, so the
//! horizontal and vertical strokes are one pixel sharp there.
//!
//! The rules: straight strokes only, diagonals allowed; corners cut by one
//! unit (one and a half for O) where a pen would round them; parallel
//! strokes at least two units apart; each glyph as few polylines as it can
//! be, so that the beam's bright beads (where strokes meet) fall on real
//! corners and junctions, never in the middle of a straight line. The
//! small letters are their own shapes, not small capitals: a large
//! x-height (six units of ten) for prose at one pixel per unit.
//!
//! Glyphs are written as text: polylines separated by `/`, points `x,y`.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::stroke::{CELL_W, Poly, Stroke};

/// Screen y of the grid's y = 0, and x of its x = 0 (in an 8-unit cell).
const TOP: f32 = 2.5;
const LEFT: f32 = 1.5;
/// The grid's centre column.
const MID: f32 = 2.5;

const GLYPHS: &[(char, &str)] = &[
    // Capitals.
    ('A', "0,10 0,2 2.5,0 5,2 5,10 / 0,6 5,6"),
    ('B', "0,5 4,5 5,6 5,9 4,10 0,10 0,0 4,0 5,1 5,4 4,5"),
    ('C', "5,0 1,0 0,1 0,9 1,10 5,10"),
    ('D', "0,0 0,10 3,10 5,8 5,2 3,0 0,0"),
    ('E', "5,0 0,0 0,10 5,10 / 0,5 4,5"),
    ('F', "5,0 0,0 0,10 / 0,5 4,5"),
    ('G', "5,0 1,0 0,1 0,9 1,10 5,10 5,6 2.5,6"),
    ('H', "0,0 0,10 / 5,0 5,10 / 0,5 5,5"),
    ('I', "1,0 4,0 / 2.5,0 2.5,10 / 1,10 4,10"),
    ('J', "5,0 5,9 4,10 1,10 0,9 0,7"),
    ('K', "0,0 0,10 / 5,0 0,5 5,10"),
    ('L', "0,0 0,10 5,10"),
    ('M', "0,10 0,0 2.5,5 5,0 5,10"),
    ('N', "0,10 0,0 5,10 5,0"),
    ('O', "1.5,0 3.5,0 5,1.5 5,8.5 3.5,10 1.5,10 0,8.5 0,1.5 1.5,0"),
    ('P', "0,10 0,0 4,0 5,1 5,4 4,5 0,5"),
    ('Q', "1.5,0 3.5,0 5,1.5 5,8.5 3.5,10 1.5,10 0,8.5 0,1.5 1.5,0 / 2.5,7.5 5,11"),
    ('R', "0,10 0,0 4,0 5,1 5,4 4,5 0,5 / 2,5 5,10"),
    ('S', "5,1 4,0 1,0 0,1 0,4 1,5 4,5 5,6 5,9 4,10 1,10 0,9"),
    ('T', "0,0 5,0 / 2.5,0 2.5,10"),
    ('U', "0,0 0,9 1,10 4,10 5,9 5,0"),
    ('V', "0,0 2.5,10 5,0"),
    ('W', "0,0 0,10 2.5,6 5,10 5,0"),
    ('X', "0,0 5,10 / 5,0 0,10"),
    ('Y', "0,0 2.5,5 5,0 / 2.5,5 2.5,10"),
    ('Z', "0,0 5,0 0,10 5,10"),
    // Small letters: x-height 4, middle 7, ascender 0, descender 13.
    ('a', "1,4 4,4 5,5 5,10 1,10 0,9 0,8 1,7 5,7"),
    ('b', "0,0 0,10 4,10 5,9 5,5 4,4 0,4"),
    ('c', "5,4 1,4 0,5 0,9 1,10 5,10"),
    ('d', "5,0 5,10 1,10 0,9 0,5 1,4 5,4"),
    ('e', "0,7 5,7 5,5 4,4 1,4 0,5 0,9 1,10 5,10"),
    ('f', "4.5,0 3,0 2,1 2,10 / 0,4 4.5,4"),
    ('g', "5,10 1,10 0,9 0,5 1,4 5,4 5,12 4,13 0.5,13"),
    ('h', "0,0 0,10 / 0,4 4,4 5,5 5,10"),
    ('i', "1,4 2.5,4 2.5,10 / 1,10 4,10 / 2.5,1 2.5,2"),
    ('j', "1.5,4 3.5,4 3.5,12 2.5,13 0.5,13 / 3.5,1 3.5,2"),
    ('k', "0,0 0,10 / 4.5,4 0,7 4.5,10"),
    ('l', "0.5,0 2,0 2,9 3,10 5,10"),
    ('m', "0,10 0,4 4,4 5,5 5,10 / 2.5,4 2.5,10"),
    ('n', "0,10 0,4 4,4 5,5 5,10"),
    ('o', "1,4 4,4 5,5 5,9 4,10 1,10 0,9 0,5 1,4"),
    ('p', "0,13 0,4 4,4 5,5 5,9 4,10 0,10"),
    ('q', "5,13 5,4 1,4 0,5 0,9 1,10 5,10"),
    ('r', "0,10 0,4 / 0,6.5 2.5,4 5,4"),
    ('s', "5,4 1,4 0,5.5 1,7 4,7 5,8.5 4,10 0,10"),
    ('t', "2,1 2,9 3,10 4.5,10 / 0,4 4.5,4"),
    ('u', "0,4 0,9 1,10 5,10 5,4"),
    ('v', "0,4 2.5,10 5,4"),
    ('w', "0,4 1.25,10 2.5,6 3.75,10 5,4"),
    ('x', "0,4 5,10 / 5,4 0,10"),
    ('y', "0,4 2.5,10 / 5,4 2.5,10 1,13"),
    ('z', "0,4 5,4 0,10 5,10"),
    // Digits.
    ('0', "1,0 4,0 5,1 5,9 4,10 1,10 0,9 0,1 1,0 / 4,2 1,8"),
    ('1', "0.5,2.5 2.5,0 2.5,10"),
    ('2', "0,1 1,0 4,0 5,1 5,4 0,10 5,10"),
    ('3', "0,0 5,0 2,4 4,4 5,5 5,9 4,10 1,10 0,9"),
    ('4', "4,10 4,0 0,7 5,7"),
    ('5', "5,0 0,0 0,4 4,4 5,5 5,9 4,10 0,10"),
    ('6', "4,0 0,4 0,9 1,10 4,10 5,9 5,6 4,5 0,5"),
    ('7', "0,0 5,0 2,10"),
    ('8', "1,5 0.5,4 0.5,1 1.5,0 3.5,0 4.5,1 4.5,4 4,5 1,5 0,6 0,9 1,10 4,10 5,9 5,6 4,5"),
    ('9', "1,10 5,6 5,1 4,0 1,0 0,1 0,4 1,5 5,5"),
    // Punctuation and signs.
    ('!', "2.5,0 2.5,7 / 2.5,9 2.5,10"),
    ('"', "1.5,0 1.5,2.5 / 3.5,0 3.5,2.5"),
    ('#', "0,4 5,4 / 0,7 5,7 / 1.5,2 1.5,9 / 3.5,2 3.5,9"),
    ('$', "5,2 0,2 0,5 5,5 5,8 0,8 / 2.5,0 2.5,10"),
    ('%', "0,0 2,0 2,3 0,3 0,0 / 5,0 0,10 / 3,7 5,7 5,10 3,10 3,7"),
    ('&', "5,10 1,3 1,1 2,0 3,0 4,1 4,2 0,6 0,9 1,10 3,10 5,7"),
    ('\'', "2.5,0 2.5,2.5"),
    ('(', "3.5,-1 2,1 2,9 3.5,11"),
    (')', "1.5,-1 3,1 3,9 1.5,11"),
    ('*', "2.5,1 2.5,7 / 0,2.5 5,5.5 / 0,5.5 5,2.5"),
    ('+', "0,6 5,6 / 2.5,3.5 2.5,8.5"),
    (',', "2.5,9 2.5,10 1.5,12"),
    ('-', "1,6 4,6"),
    ('.', "2.5,9 2.5,10"),
    ('/', "5,0 0,10"),
    (':', "2.5,4 2.5,5 / 2.5,9 2.5,10"),
    (';', "2.5,4 2.5,5 / 2.5,9 2.5,10 1.5,12"),
    ('<', "4.5,2.5 0.5,6 4.5,9.5"),
    ('=', "0,5 5,5 / 0,7 5,7"),
    ('>', "0.5,2.5 4.5,6 0.5,9.5"),
    ('?', "0,1 1,0 4,0 5,1 5,3 2.5,5.5 2.5,7 / 2.5,9 2.5,10"),
    ('@', "3.5,6.5 3.5,3.5 1.5,3.5 1.5,6.5 5,6.5 5,1 4,0 1,0 0,1 0,9 1,10 5,10"),
    ('[', "3.5,-1 1.5,-1 1.5,11 3.5,11"),
    ('\\', "0,0 5,10"),
    (']', "1.5,-1 3.5,-1 3.5,11 1.5,11"),
    ('^', "0.5,3 2.5,0 4.5,3"),
    // Across the whole cell (−1.5 to 6.5 is 0 to 8), so that a run of
    // them is one stroke (text::merge).
    ('_', "-1.5,12 6.5,12"),
    ('`', "1.5,0 3,2"),
    ('{', "3.5,-1 2.5,-1 2.5,4 1.5,5 2.5,6 2.5,11 3.5,11"),
    ('|', "2.5,-1 2.5,11"),
    ('}', "1.5,-1 2.5,-1 2.5,4 3.5,5 2.5,6 2.5,11 1.5,11"),
    ('~', "0,6.5 1.5,5 3.5,7 5,5.5"),
    // Latin-1's signs.
    (' ', ""),
    ('\u{a0}', ""),
    ('¡', "2.5,4 2.5,5 / 2.5,7 2.5,13"),
    ('¿', "5,12 4,13 1,13 0,12 0,10 2.5,7.5 2.5,6 / 2.5,4 2.5,3"),
    ('¢', "5,4 1,4 0,5 0,9 1,10 5,10 / 3,2.5 3,11.5"),
    ('£', "5,1 4,0 2,0 1,1 1,10 / 0,10 5,10 / 0,5 3,5"),
    ('¤', "1,4 4,4 4,7 1,7 1,4 / 0,3 1,4 / 5,3 4,4 / 0,8 1,7 / 5,8 4,7"),
    ('¥', "0,0 2.5,4 5,0 / 2.5,4 2.5,10 / 0.5,5 4.5,5 / 0.5,7 4.5,7"),
    ('¦', "2.5,-1 2.5,4 / 2.5,6 2.5,11"),
    ('§', "4.5,0 0.5,0 0.5,3 4.5,3 4.5,10 0.5,10 / 0.5,3 0.5,7 4.5,7"),
    ('©', "1.5,0 3.5,0 5,1.5 5,8.5 3.5,10 1.5,10 0,8.5 0,1.5 1.5,0 / 3.5,3 1.5,3 1.5,7 3.5,7"),
    ('ª', "1,0 4,0 4,4 1,4 1,2 4,2 / 1,6 4,6"),
    ('«', "2.5,4 0.5,6 2.5,8 / 4.5,4 2.5,6 4.5,8"),
    ('¬', "0,5 5,5 5,7.5"),
    ('\u{ad}', "1,6 4,6"),
    ('®', "1.5,0 3.5,0 5,1.5 5,8.5 3.5,10 1.5,10 0,8.5 0,1.5 1.5,0 / 1.5,7 1.5,3 3.5,3 3.5,5 1.5,5 / 2.5,5 3.5,7"),
    ('¯', "0.5,-1 4.5,-1"),
    ('°', "1.5,0 3.5,0 3.5,2 1.5,2 1.5,0"),
    ('±', "0,4 5,4 / 2.5,1.5 2.5,6.5 / 0,9 5,9"),
    ('µ', "0,13 0,4 / 0,9 1,10 5,10 5,4"),
    ('¶', "5,10 5,0 1,0 0,1 0,4 1,5 2.5,5 / 2.5,0 2.5,10"),
    ('·', "2.5,6.5 2.5,7.5"),
    ('¸', "2.5,10 2.5,11 3.5,12 1.5,13"),
    ('º', "1,0 4,0 4,4 1,4 1,0 / 1,6 4,6"),
    ('»', "0.5,4 2.5,6 0.5,8 / 2.5,4 4.5,6 2.5,8"),
    ('×', "0.5,4 4.5,8 / 4.5,4 0.5,8"),
    ('÷', "0,6 5,6 / 2.5,3.5 2.5,4.5 / 2.5,7.5 2.5,8.5"),
    // Latin-1's letters that are not a letter and an accent.
    ('Æ', "5,0 2.5,0 0,3 0,10 / 2.5,0 2.5,10 5,10 / 0,5 4.5,5"),
    ('æ', "0,4 5,4 5,7 0,7 0,10 5,10 / 2.5,4 2.5,10"),
    ('Ð', "0,0 0,10 3,10 5,8 5,2 3,0 0,0 / -1,5 1.5,5"),
    ('ð', "4,4 1,4 0,5 0,9 1,10 4,10 5,9 5,3 3,0 / 2.5,1 5,1"),
    ('Ø', "1.5,0 3.5,0 5,1.5 5,8.5 3.5,10 1.5,10 0,8.5 0,1.5 1.5,0 / 5.5,-0.5 -0.5,10.5"),
    ('ø', "1,4 4,4 5,5 5,9 4,10 1,10 0,9 0,5 1,4 / 5,3 0,11"),
    ('Þ', "0,0 0,10 / 0,2 4,2 5,3 5,6 4,7 0,7"),
    ('þ', "0,0 0,13 / 0,4 4,4 5,5 5,9 4,10 0,10"),
    ('ß', "0,10 0,1 1,0 4,0 5,1 5,3 3,5 5,7 5,9 4,10 2,10"),
    // The other marks of the basic repertoire.
    ('–', "-0.5,6 5.5,6"),
    // Across the whole cell, as '_'.
    ('—', "-1.5,6 6.5,6"),
    ('\u{2212}', "0,6 5,6"),
    ('‘', "3,0 2,0 2,2.5"),
    ('’', "2,2.5 3,2.5 3,0"),
    ('“', "2,0 1,0 1,2.5 / 4,0 3,0 3,2.5"),
    ('”', "1,2.5 2,2.5 2,0 / 3,2.5 4,2.5 4,0"),
    ('•', "1.5,5 3.5,5 3.5,7 1.5,7 1.5,5 / 1.5,6 3.5,6"),
    ('…', "0,9 0,10 / 2.5,9 2.5,10 / 5,9 5,10"),
    ('\u{fffd}', "2.5,-1 6,5 2.5,11 -1,5 2.5,-1 / 1.5,3 2.5,2 3.5,3 2.5,5 2.5,6 / 2.5,7.5 2.5,8.5"),
];

fn parse(s: &str) -> Vec<Poly> {
    s.split('/')
        .map(|p| {
            p.split_whitespace()
                .map(|xy| {
                    let (x, y) = xy.split_once(',').expect("x,y");
                    (x.parse().expect("x"), y.parse().expect("y"))
                })
                .collect::<Poly>()
        })
        .filter(|p| !p.is_empty())
        .collect()
}

fn table() -> &'static HashMap<char, Vec<Poly>> {
    static T: OnceLock<HashMap<char, Vec<Poly>>> = OnceLock::new();
    T.get_or_init(|| GLYPHS.iter().map(|&(c, s)| (c, parse(s))).collect())
}

fn plain(c: char) -> Vec<Poly> {
    table().get(&c).cloned().unwrap_or_default()
}

/// (x, y) → (x × sx + dx, y × sy + dy).
fn tf(polys: Vec<Poly>, sx: f32, sy: f32, dx: f32, dy: f32) -> Vec<Poly> {
    polys.into_iter().map(|p| p.into_iter().map(|(x, y)| (x * sx + dx, y * sy + dy)).collect()).collect()
}

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

/// An accent between `a` (its top) and `b` (its bottom).
fn accent(k: Accent, a: f32, b: f32) -> Vec<Poly> {
    let m = (a + b) / 2.0;
    match k {
        Accent::Grave => vec![vec![(1.5, a), (3.5, b)]],
        Accent::Acute => vec![vec![(3.5, a), (1.5, b)]],
        Accent::Circumflex => vec![vec![(0.5, b), (MID, a), (4.5, b)]],
        Accent::Tilde => vec![vec![(0.0, b), (1.5, a), (3.5, b), (5.0, a)]],
        Accent::Diaeresis => vec![vec![(1.0, m - 0.5), (1.0, m + 0.5)], vec![(4.0, m - 0.5), (4.0, m + 0.5)]],
        Accent::Ring => vec![vec![(MID, a), (3.75, m), (MID, b), (1.25, m), (MID, a)]],
        Accent::Cedilla => plain('¸'),
    }
}

/// A letter and its accent. A capital is drawn shorter (from y = 3 to the
/// baseline) to make room for its accent above, as terminal fonts do; a
/// small letter's accent sits between the ascender line and its top.
fn accented(base: char, k: Accent) -> Vec<Poly> {
    if let Accent::Cedilla = k {
        let mut g = plain(base);
        g.extend(accent(k, 0.0, 0.0));
        return g;
    }
    if base.is_uppercase() {
        let mut g = tf(plain(base), 1.0, 0.7, 0.0, 3.0);
        g.extend(accent(k, -1.5, 1.5));
        g
    } else {
        let mut g = plain(base);
        g.extend(accent(k, 0.5, 2.5));
        g
    }
}

/// The dotless i, for the small i's accents.
const DOTLESS_I: &str = "1,4 2.5,4 2.5,10 / 1,10 4,10";

fn base(c: char, upper: char) -> char {
    if c.is_uppercase() { upper } else { upper.to_ascii_lowercase() }
}

/// The polylines of a character on the grid, or None if this font does not
/// draw it.
pub fn glyph(c: char) -> Option<Vec<Poly>> {
    use Accent::*;
    if let Some(g) = table().get(&c) {
        return Some(g.clone());
    }
    Some(match c {
        '¨' => accent(Diaeresis, 0.5, 2.5),
        '´' => accent(Acute, 0.5, 2.5),
        '¹' => tf(plain('1'), 0.5, 0.45, 1.25, 0.0),
        '²' => tf(plain('2'), 0.5, 0.45, 1.25, 0.0),
        '³' => tf(plain('3'), 0.5, 0.45, 1.25, 0.0),
        '¼' | '½' | '¾' => {
            let (n, d) = match c {
                '¼' => ('1', '4'),
                '½' => ('1', '2'),
                _ => ('3', '4'),
            };
            let mut g = tf(plain(n), 0.4, 0.4, 0.0, 0.0);
            g.extend(tf(plain(d), 0.4, 0.4, 3.0, 6.0));
            g.push(vec![(5.0, 0.0), (0.0, 10.0)]);
            g
        }
        'À' | 'à' => accented(base(c, 'A'), Grave),
        'Á' | 'á' => accented(base(c, 'A'), Acute),
        'Â' | 'â' => accented(base(c, 'A'), Circumflex),
        'Ã' | 'ã' => accented(base(c, 'A'), Tilde),
        'Ä' | 'ä' => accented(base(c, 'A'), Diaeresis),
        'Å' | 'å' => accented(base(c, 'A'), Ring),
        'Ç' | 'ç' => accented(base(c, 'C'), Cedilla),
        'È' | 'è' => accented(base(c, 'E'), Grave),
        'É' | 'é' => accented(base(c, 'E'), Acute),
        'Ê' | 'ê' => accented(base(c, 'E'), Circumflex),
        'Ë' | 'ë' => accented(base(c, 'E'), Diaeresis),
        'Ì' => accented('I', Grave),
        'Í' => accented('I', Acute),
        'Î' => accented('I', Circumflex),
        'Ï' => accented('I', Diaeresis),
        'ì' | 'í' | 'î' | 'ï' => {
            let mut g = parse(DOTLESS_I);
            g.extend(accent([Grave, Acute, Circumflex, Diaeresis][c as usize - 'ì' as usize], 0.5, 2.5));
            g
        }
        'Ñ' | 'ñ' => accented(base(c, 'N'), Tilde),
        'Ò' | 'ò' => accented(base(c, 'O'), Grave),
        'Ó' | 'ó' => accented(base(c, 'O'), Acute),
        'Ô' | 'ô' => accented(base(c, 'O'), Circumflex),
        'Õ' | 'õ' => accented(base(c, 'O'), Tilde),
        'Ö' | 'ö' => accented(base(c, 'O'), Diaeresis),
        'Ù' | 'ù' => accented(base(c, 'U'), Grave),
        'Ú' | 'ú' => accented(base(c, 'U'), Acute),
        'Û' | 'û' => accented(base(c, 'U'), Circumflex),
        'Ü' | 'ü' => accented(base(c, 'U'), Diaeresis),
        'Ý' | 'ý' => accented(base(c, 'Y'), Acute),
        'ÿ' => accented('y', Diaeresis),
        _ => return None,
    })
}

/// The strokes of a character in a cell `cell_w` wide (8, or 16 for a wide
/// character's replacement), or false if this font does not draw it.
pub fn cell_strokes(c: char, cell_w: f32, out: &mut Vec<Stroke>) -> bool {
    let Some(polys) = glyph(c) else { return false };
    let k = cell_w / CELL_W;
    let map = |(x, y): (f32, f32)| (cell_w / 2.0 + (LEFT + x - CELL_W / 2.0) * k, TOP + y);
    for p in &polys {
        for w in p.windows(2) {
            let ((x0, y0), (x1, y1)) = (map(w[0]), map(w[1]));
            out.push(Stroke { x0, y0, x1, y1, width: 1.0, level: 1.0 });
        }
    }
    true
}
