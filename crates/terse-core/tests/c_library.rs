//! programs/lib/terse.c must agree with the machine: its wide-character
//! table is the one of SPEC 8.1 (`terse_core::wide`).

const TERSE_C: &str = include_str!("../../../programs/lib/terse.c");

#[test]
fn c_wide_table_matches_the_terminal() {
    let start = TERSE_C.find("static const uint16_t wide[] = {").expect("the table in terse.c");
    let body = &TERSE_C[start..];
    let body = &body[body.find('{').unwrap() + 1..body.find("};").unwrap()];
    let numbers: Vec<u16> = body
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| u16::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
        .collect();
    assert_eq!(numbers.len() % 2, 0);
    let ranges: Vec<(u16, u16)> = numbers.chunks(2).map(|r| (r[0], r[1])).collect();
    assert!(ranges.windows(2).all(|w| w[0].1 < w[1].0), "sorted, for the binary search");
    for cp in 0..=0xFFFFu16 {
        let in_c = ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp));
        assert_eq!(in_c, terse_core::wide::is_wide(cp), "U+{cp:04X}");
    }
}
