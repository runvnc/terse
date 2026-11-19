//! The core test vectors (`terse vectors`): blob IDs, entries, profile
//! documents, a relay session and an article corpus, as plain text files
//! that any implementation of NETWORK.md and ARTICLE.md can check itself
//! against. `vectors/README` (written from [`README`]) explains every
//! file's format. Everything here is generated: the article dumps by
//! [`crate::article_parse`], the entries by [`crate::entry`], the messages
//! by [`crate::protocol`].

use std::fmt::Write as _;
use std::path::Path;

use crate::article_parse;
use crate::entry::{self, Entry, Invalid, SignedEntry};
use crate::identity::Identity;
use crate::{blob, profile, protocol};

/// A file of the vectors directory: its path under the directory and its
/// bytes.
pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
}

// --- The article corpus ------------------------------------------------------

/// An article ID, a feed key and a program publisher's key used in links.
pub const A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub const B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
pub const K: &str = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00";

fn fill(s: &str) -> Vec<u8> {
    s.replace("{A}", A)
        .replace("{B}", B)
        .replace("{K}", K)
        .replace("{A_UP}", &A.to_uppercase())
        .replace("{A63}", &A[..63])
        .into_bytes()
}

/// The corpus: (name, article bytes). Each case shows one construct of
/// ARTICLE.md and its near misses.
pub fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    let mut c: Vec<(&'static str, Vec<u8>)> = vec![
        (
            "blocks-paragraphs",
            fill(
                "A paragraph's lines\n   join with a space;\nblanks at the start of a line are dropped.\n\n\
                 Blank lines separate\nparagraphs.\n  \nA line of only blanks (above: two spaces) is a blank line too.\n",
            ),
        ),
        (
            "blocks-headings",
            fill(
                "# A heading\nText right under it.\n## A subheading\n### Three marks: ordinary text\n\
                 #No space: ordinary text\n  # Blanks before the marker: ordinary text\n\
                 #\tA tab after the marker: ordinary text\n# \n##    Blanks after the marker are dropped\n",
            ),
        ),
        (
            "blocks-lists",
            fill(
                "- First item\n* Second item, a star\ngoes on here\n-not an item: continues the second\n  - nor this\n\n\
                 - Third item after a blank line\nan ordinary line continues the item.\n\n1. Not a numbered list\n\
                 - Item\n    an indented line starts a preformatted block\n",
            ),
        ),
        (
            "blocks-preformatted",
            fill(
                "Text.\n    four spaces\n\ta tab\n      six spaces keep the two extra\n   three spaces: ordinary\n\n\
                 \x20   another block\n\n    and another after a blank line\n",
            ),
        ),
        (
            "blocks-fences",
            fill(
                "```info string is ignored\nline one\n\n    indented line\n``` not a closing line\n````\n```   \n\
                 After the fence.\n",
            ),
        ),
        ("blocks-fence-unclosed", fill("Before.\n\n```\nruns to the end\n# not a heading\n")),
        ("blocks-fence-empty", fill("Before.\n\n```\n```\n\nAfter.\n")),
        ("blocks-paragraph-ends", fill("A paragraph\n# then a heading\nand a paragraph\n```\nfence\n```\nlast\n| table\n")),
        ("text-crlf", fill("# Heading\r\nA paragraph\r\non two lines.\r\n\r\n    pre\r\n- item\r\nno final line break\r")),
        (
            "text-controls",
            b"Malformed \xff byte, a cut \xe2\x82 sequence, a control\x01 character, DEL\x7f, C1 \xc2\x85.\n\
              A lone CR in a line: car\rriage.\n"
                .to_vec(),
        ),
        ("text-blanks", fill("Runs   of  blanks\t\tinside a line, and blanks at its end.   \n")),
        (
            "code-spans",
            fill(
                "A `code span` and `two` of them.\nTwo backquotes `` are no span.\nHere ``x` the second backquote starts one.\n\
                 A `split\nspan` is none: code spans do not cross lines.\n`terse:{A}` is code, not a link.\nA lone ` backquote.\n",
            ),
        ),
        ("code-span-length", fill(&format!("`{}` is a code span.\n\n`{}` is not.\n", "x".repeat(200), "y".repeat(201)))),
        ("code-spans-in-blocks", fill("# A `heading`\n## A `subheading`\n- An `item`\n\n    `not code` in pre\n\n```\n`not code` in a fence\n```\n")),
        (
            "links-bare",
            fill(
                "An article terse:{A}, a feed terse:feed/{B}.\nAt a sentence's end terse:{A}.\n\
                 xterse:{A} is text, so is 9terse:{A}.\nterse:{A}x is text, so is terse:{A}0.\n\
                 terse:{A_UP} has capitals: text.\nterse:{A63} has 63 digits: text.\nterse:feed/{A63} too.\n\
                 (terse:{A}) in parentheses, -terse:{A}, and \u{e9}terse:{A} after a non-ASCII letter.\n\
                 TERSE:{A} in capitals is text, so is terse: {A}.\nterse:{A}terse:{B} is text.\n",
            ),
        ),
        (
            "links-program",
            fill(&format!(
                "terse:program/{{K}}/Sketch%20pad%20%C3%a9 by name.\nA final dot is not part of the name: terse:program/{{K}}/Pad.\n\
                 Nor are two: terse:program/{{K}}/Pad..\nUnreserved bytes: terse:program/{{K}}/a-b_c~d.e\n\
                 A space at the end: terse:program/{{K}}/Pad%20 is text.\nAn incomplete escape: terse:program/{{K}}/Pad%2 stops before it.\n\
                 No name: terse:program/{{K}}/ is text.\nToo long: terse:program/{{K}}/{} is text.\n\
                 Not UTF-8: terse:program/{{K}}/%FF is text.\nA control: terse:program/{{K}}/a%0Ab is text.\n",
                "x".repeat(49)
            )),
        ),
        (
            "links-with-text",
            fill(&format!(
                "[Notes](terse:{{A}}) and [a feed](terse:feed/{{B}}) and [a program](terse:program/{{K}}/Pad).\n\
                 [Notes]( terse:{{A}}) has a blank: text, then a bare link.\n[](terse:{{A}}) has no text.\n\
                 [a `b` c](terse:{{A}}) holds a backquote.\n[see terse:{{B}}](terse:{{A}}) holds a link.\n\
                 [[nested](terse:{{A}})] in brackets.\n[x](terse:{{A}}.) has more than the link.\n\
                 [x](terse:program/{{K}}/Pad.) ends in a dot.\n[{}](terse:{{A}}) is too long.\n[{}](terse:{{A}}) is not.\n",
                "x".repeat(201),
                "y".repeat(200)
            )),
        ),
        (
            "links-web",
            fill(
                "http://example.org is text.\nhttps://example.org/a?b=c#d too.\n\
                 [Example](https://example.org) is shown as written.\n[Mail](mailto:someone@example.org) as well.\n\
                 <https://example.org> and www.example.org are text.\n",
            ),
        ),
        (
            "links-in-blocks",
            fill(
                "# Heading terse:{A}\n## [Subheading](terse:feed/{B})\n- Item [text](terse:{A})\n\
                 \x20   pre terse:{A} [with text](terse:{B})\n\n```fence\n[fence](terse:{A}) terse:feed/{B}\n```\n",
            ),
        ),
        (
            "links-overlap",
            fill("`[a](terse:{A})` is code.\n[a `x](terse:{A})` y`: the brackets hold a backquote.\n[a](terse:{A})`b`\n"),
        ),
        (
            "tables-basic",
            fill(
                "| Key | What | Notes |\n|:--|:-:|--:|\n| `M` | the manual | [the index](terse:{A}) |\n\
                 | a \\| b | `x \\| y` | extra | dropped |\n|short\n",
            ),
        ),
        ("tables-no-header", fill("|---|---|\n| no | header |\\|\n| third |\n")),
        (
            "tables-alignment",
            fill(
                "| a | b | c | d | e | f | g |\n| :-: | -: | :- | : | :: | --- |\n| 1 | 2 | 3 | 4 | 5 | 6 | 7 |\n\n\
                 | a | b |\n| 1 | 2 |\n|---|:-:|\n",
            ),
        ),
        ("tables-wide", fill("| 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |\n| a | b |\n")),
        ("tables-edges", fill("|\n||\n| a ||\n|\\|\n| \\\\| x |\n\nText\n | not a table: a blank before the bar\n")),
        ("tables-inline", fill("| `a` b `c` | terse:feed/{B} | [x](terse:{A}) `y` |\n")),
        ("tables-link-with-bar", fill("| [a \\| b](terse:{A}) | c |\n")),
        (
            "figures-example",
            fill(
                "```path 200 60\nfill #2b3a55 stroke none M0 0 H200 V60 H0 Z\nfill #98c379 stroke fg 2 M20 50 L60 10 L100 50 Z\n\
                 fill none stroke #e5c07b 3 M120 50 Q150 0 180 50\n```\n",
            ),
        ),
        (
            "figures-commands",
            fill(
                "```path 200 100\nM10 10 H50 V30 h10 v-5 L70 70 l5,5 Z\nm20 0 30 0 0 10\n\
                 C10 10 20 20 30 30 S50 50 60 60 s5 5 10 10\nQ100 10 110 20 T130 20 t10 0\n\
                 M0 0 C1 1 2 2 3 3 fill #f00 S9 9 10 10\nM5 5 Q6 6 7 7 S8 8 9 9\n```\n",
            ),
        ),
        (
            "figures-words",
            fill(
                "```path 100 100\nfill #abc stroke #11223344 2.5 M0 0 L10 0 L10 10 Z\nstroke fg M20 20 L30 30\n\
                 fill none stroke none M40 40 L50 50\nfill #ff000000 stroke #00ff00 0 M1 1 L2 2\n\
                 fill #ABCDEF stroke #00ff0080 M60 60 L70 70 fill fg\n```\n",
            ),
        ),
        (
            "figures-numbers",
            fill("```path 100 100\nM10-5L.5.5 L5 5L6 6,,7,7\nM+1.03125 -0.03125 L0.96875 2.\nM2047 -2047 L-2047 2047\n```\n"),
        ),
        ("figures-implicit-moves", fill("```path 50 50\nM10 10 L20 10 Z L20 20\nfill #000 L30 30\n```\n")),
        ("figures-empty", fill("```path 40 20\n```\n\n```path 40 20\nfill #fff stroke none\n```\n")),
        (
            "figures-size-forms",
            fill("```path 0010 010\nM0 0 L1 1\n```\n\n```path +10 10\nM0 0 L1 1\n```\n\n```path\t10\t10\t\nM0 0 L1 1\n```\n"),
        ),
        ("figures-size", fill("```path 1024 1\nM0 0 L1 1\n```\n\n```  path   7 9  \nM0 0 L1 1\n```\n")),
        (
            "figures-malformed-size",
            fill(
                "```path200 60\nM0 0 L1 1\n```\n\n```paths 10 10\nM0 0 L1 1\n```\n\n```path 0 50\nM0 0 L1 1\n```\n\n\
                 ```path 1025 1\nM0 0 L1 1\n```\n\n```path 10\nM0 0 L1 1\n```\n\n```path 10 10 10\nM0 0 L1 1\n```\n\n\
                 ```path 10 10x\nM0 0 L1 1\n```\n",
            ),
        ),
        (
            "figures-malformed-path",
            fill(
                "```path 100 50\nM10 10 A 5 5 0 0 1 20 20\n```\n\n```path 100 50\nM1e2 0\n```\n\n\
                 ```path 100 50\nfill #abcd M0 0 L1 1\n```\n\n```path 100 50\n10 10 M0 0\n```\n\n\
                 ```path 100 50\nM0 0 L2048 0\n```\n\n```path 100 50\nM0 0 L2047.04 0\n```\n\n\
                 ```path 100 50\nM0 0 stroke\n```\n\n```path 100 50\nM0 0 L1 1 2\n```\n\n\
                 ```path 100 50\nL0 0 L1 1\n```\n\n```path 100 50\nfill #fffM0 0 L1 1\n```\n\n\
                 ```path 100 50\nM0 0 Z 5\n```\n\n```path 100 50\nstrokes #fff M0 0 L1 1\n```\n\n\
                 ```path 100 50\nstroke #fff 2048 M0 0 L1 1\n```\n\n```path 100 50\nstroke #fff -1 M0 0 L1 1\n```\n\n\
                 ```path 100 50\nM 1.000000000000000000000000000000001 0 L1 1\n```\n\n\
                 ```path 100 50\nfill red M0 0 L1 1\n```\n\n```path 100 50\nM0 0 L1 1 fill\n```\n\n\
                 ```path 100 50\nM0 0 L10 10\n",
            ),
        ),
        ("figures-command-without-numbers", fill("```path 100 50\nM0 0 L\n```\n\n```path 100 50\nM0 0 L10 10 Z M\n```\n")),
        ("figures-z-after-word", fill("```path 50 50\nM0 0 L10 10 fill #f00 Z L20 20\n```\n")),
        (
            "blocks-markers",
            fill("- \n-\n* \n*x\n##\n#  \n  ```\n    ```\n\t\nText\n| a |\n- item\n# heading\n- item\n```\nfence\n```\n"),
        ),
        (
            "text-bom",
            "\u{feff}# A byte order mark first is not shown: a heading\nOne later, \u{feff}here, is text.\n".as_bytes().to_vec(),
        ),
        (
            "text-lone-cr",
            fill(
                "A lone CR\ris a blank, shown as a space.\r\nA CR before an LF is not shown.\r\n\r\n    pre\rline\r\n\
                 | a\rb | `c\rd` |\n\n# Head\ring\n\n```path 10 10\nM0 0\rL1 1\n```\nlast\r",
            ),
        ),
        (
            "links-near-misses",
            fill(
                "`a`terse:{A} after a code span.\nA line break before\nterse:{A} the link.\n[a]b](terse:{A}) and [a](terse:{A})(terse:{B}).\n\
                 [a](terse:{A}x) is text.\nterse:program/{A_UP}/Pad has capitals: text.\nterse:program/{K}/Pad%2e has an escaped dot.\n\
                 terse:program/{K}Pad has no slash: text.\n",
            ),
        ),
        ("tables-cut-before-spans", fill("| `a|b` | c |\n| :| | |\n| d |\n\n| head |\n|-|\n\n| x |\n| | |\n")),
        (
            "figures-reflection",
            fill(
                "```path 100 100\nM10 10 Q20 0 30 10 S50 20 60 10\nM10 50 C20 40 30 40 40 50 T60 50\nM0 90 L10 90 Z S20 80 30 90\n\
                 m5 5 c1 1 2 2 3 3 s4 4 5 5 t1 1\n```\n",
            ),
        ),
        (
            "figures-tokens",
            fill(
                "```path 100 100\nM0 0L5 5Zm5 5l1 1\nM0 0 L5 5,fill #f00 M1 1 L2 2\nfill\n  #fff,stroke #ABC 1.5,M3 3 L4 4\n\
                 stroke #fff 0 stroke fg M5 5 L6 6\nstroke #fff 2047 M7 7 L8 8\n```\n",
            ),
        ),
        (
            "figures-out-of-range",
            fill(
                "```path 100 100\nM0 0 Q3000 0 10 10\n```\n\n```path 100 100\nM0 0 C0 0 -2000 0 1000 0 S1000 0 1000 0\n```\n\n\
                 ```path 100 100\nstroke #fff 2047.03125 M0 0 L1 1\n```\n\n```path 100 100\nM2000 0 l48 0\n```\n",
            ),
        ),
        (
            "links-program-dots",
            fill(
                "terse:program/{K}/Pad... ends a sentence.\nterse:program/{K}/a.b. keeps its inner dot.\n\
                 terse:program/{K}/... is only dots: text.\nterse:program/{K}/Pad%2E. keeps an escaped dot.\n",
            ),
        ),
        (
            "tables-limits-as-written",
            fill(&format!(
                "| `{x199}\\|` | `{x198}\\|` |\n| [{x199}\\|](terse:{{A}}) | [{x198}\\|](terse:{{A}}) |\n",
                x199 = "x".repeat(199),
                x198 = "y".repeat(198)
            )),
        ),
        (
            "figures-size-digits",
            fill(
                "```path 0100 0050\nM0 0 L1 1\n```\n\n```path 00100 50\nM0 0 L1 1\n```\n\n\
                 ```path 100 00050\nM0 0 L1 1\n```\n\n```path 0000 10\nM0 0 L1 1\n```\n",
            ),
        ),
        (
            "figures-word-after-number",
            fill(
                "```path 50 50\nM0 0 L5 5fill #f00\n```\n\n```path 50 50\nM0 0 L5 5Zstroke fg\n```\n\n\
                 ```path 50 50\nstroke #fff 2fill #f00 M0 0 L1 1\n```\n\n\
                 ```path 50 50\nM0 0L5 5L6 6Z fill #f00\tM1 1 2 2\n```\n",
            ),
        ),
        (
            "figures-range-as-written",
            fill(
                "```path 100 100\nM0 0 L2047.01 0\n```\n\n```path 100 100\nM0 0 L2046.99 -2046.99\n```\n\n\
                 ```path 100 100\nM2047 0 l0.001 0\n```\n\n```path 100 100\nM2047 0 l-0.001 0 h0\n```\n\n\
                 ```path 100 100\nM0 0 V-2047.0000001\n```\n\n```path 100 100\nstroke fg 2047.001 M0 0 L1 1\n```\n\n\
                 ```path 100 100\nstroke fg -0.001 M0 0 L1 1\n```\n\n```path 100 100\nstroke fg -0 M0 0 L1 1\n```\n",
            ),
        ),
    ];
    c.push(("not-an-article-image", b"TERS\x01\x00\x00\x00 a program image, not text\n".to_vec()));
    c.push(("not-an-article-document", b"TDOC\x00 a document, not text\n".to_vec()));
    c
}

/// The corpus files: each case's source and its dump.
fn article_files() -> Vec<File> {
    let mut out = Vec::new();
    for (name, text) in corpus() {
        let dump = article_parse::dump(&article_parse::parse(&text));
        out.push(File { path: format!("articles/{name}.txt"), bytes: text });
        out.push(File { path: format!("articles/{name}.dump"), bytes: dump.into_bytes() });
    }
    out
}

// --- Shared -------------------------------------------------------------------

/// Bytes as lowercase hex digits; `-` for no bytes.
fn hex(b: &[u8]) -> String {
    if b.is_empty() {
        return "-".into();
    }
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// The public test key: seed 00 01 02 … 1f.
pub fn test_identity() -> Identity {
    Identity::from_seed(std::array::from_fn(|i| i as u8))
}

/// A second public test key: seed 20 21 … 3f.
fn other_identity() -> Identity {
    Identity::from_seed(std::array::from_fn(|i| 0x20 + i as u8))
}

const TIME: u32 = 1_700_000_000;
const HELLO: &[u8] = b"Hello, Terse!";
const PROFILE_DOC: &[u8] = b"listed: yes\n";

fn signed(e: Entry, who: &Identity) -> SignedEntry {
    e.sign(who).expect("a valid entry")
}

/// `unsigned` (bytes 0 to 108 + n) signed by `who`, whatever they hold.
fn sign_raw(unsigned: &[u8], who: &Identity) -> Vec<u8> {
    let sig = who.sign(&[entry::DOMAIN, unsigned].concat());
    [unsigned, &sig[..]].concat()
}

/// The article entry of the session and of entries.txt.
fn hello_entry() -> SignedEntry {
    let id = test_identity();
    signed(Entry::article(id.feed_key(), 1, TIME, HELLO, None, "Hello"), &id)
}

/// An entry of a kind NETWORK.md does not define (9): valid.
const LATER: &[u8] = b"Something a later revision defines.\n";

fn unknown_kind_entry() -> SignedEntry {
    let id = test_identity();
    signed(Entry { kind: 9, ..Entry::article(id.feed_key(), 9, TIME + 300, LATER, None, " any title ") }, &id)
}

/// The entry `kind-255` of entries.txt: `article` with kind 255, signed.
fn reserved_kind_entry() -> Vec<u8> {
    let b = &hello_entry().bytes;
    let mut u = b[..b.len() - 64].to_vec();
    u[40] = 255;
    sign_raw(&u, &test_identity())
}

fn profile_entry() -> SignedEntry {
    let id = test_identity();
    signed(Entry::profile(id.feed_key(), 2, TIME + 60, PROFILE_DOC, "Alice"), &id)
}

// --- Blobs (NETWORK.md 1) --------------------------------------------------------

fn blobs_file() -> String {
    let mut out = String::from(
        "# Terse test vectors: blob IDs (NETWORK.md section 1). Format: vectors/README.\n\
         # A blob is 1 to 1,048,576 bytes; its ID is the BLAKE3-256 hash of its bytes.\n",
    );
    let pattern = |n: usize| -> Vec<u8> { (0..n).map(|i| (i % 251) as u8).collect() };
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        ("empty", Vec::new(), false),
        ("one-byte", vec![0], true),
        ("abc", b"abc".to_vec(), true),
        ("hello", HELLO.to_vec(), true),
        ("profile-document", PROFILE_DOC.to_vec(), true),
        ("chunk", pattern(1024), true),
        ("chunk-and-one", pattern(1025), true),
        ("largest", pattern(blob::MAX_BLOB), true),
        ("too-large", pattern(blob::MAX_BLOB + 1), false),
    ];
    for (name, bytes, valid) in cases {
        let _ = writeln!(out, "\nblob {name}");
        let _ = writeln!(out, "  size {}", bytes.len());
        if bytes.len() <= 64 {
            let _ = writeln!(out, "  hex {}", hex(&bytes));
        } else {
            out.push_str("  pattern mod251\n");
        }
        assert_eq!(blob::valid_size(bytes.len()), valid, "{name}");
        let _ = writeln!(out, "  valid {}", if valid { "yes" } else { "no" });
        let _ = writeln!(out, "  blake3 {}", hex(&blob::blob_id(&bytes)));
    }
    out
}

// --- Entries (NETWORK.md 4.2) ----------------------------------------------------

/// The fields of a valid entry, its signed message, signature and bytes.
fn valid_entry(out: &mut String, name: &str, e: &SignedEntry, blob: &[u8]) {
    assert_eq!(entry::decode(&e.bytes).as_ref(), Ok(e), "{name}");
    let x = &e.entry;
    let n = x.title.len();
    let _ = writeln!(out, "\nentry {name}");
    out.push_str("  valid yes\n");
    let _ = writeln!(out, "  feed-key {}", hex(&x.feed));
    let _ = writeln!(out, "  seq {}", x.seq);
    let _ = writeln!(out, "  time {}", x.time);
    let _ = writeln!(out, "  kind {}", x.kind);
    let _ = writeln!(out, "  size {}", x.size);
    if blob.len() <= 64 {
        let _ = writeln!(out, "  blob-hex {}", hex(blob));
    } else {
        out.push_str("  blob-pattern mod251\n");
    }
    let _ = writeln!(out, "  blob-id {}", hex(&x.blob));
    let _ = writeln!(out, "  reply-to {}", hex(&x.reply_to));
    let _ = writeln!(out, "  title {}", article_parse::quote(&x.title));
    let _ = writeln!(out, "  signed-message {}", hex(&[entry::DOMAIN, &e.bytes[..109 + n]].concat()));
    let _ = writeln!(out, "  signature {}", hex(&e.bytes[109 + n..]));
    let _ = writeln!(out, "  bytes {}", hex(&e.bytes));
}

fn invalid_entry(out: &mut String, name: &str, bytes: &[u8], reason: &str, why: Option<Invalid>) {
    let got = entry::decode(bytes);
    assert!(got.is_err(), "{name} decodes");
    if let Some(w) = why {
        assert_eq!(got.unwrap_err(), w, "{name}");
    }
    let _ = writeln!(out, "\nentry {name}");
    out.push_str("  valid no\n");
    let _ = writeln!(out, "  reason {}", article_parse::quote(reason));
    let _ = writeln!(out, "  bytes {}", hex(bytes));
}

fn entries_file() -> String {
    let id = test_identity();
    let key = id.feed_key();
    let mut out = String::from(
        "# Terse test vectors: feed entries (NETWORK.md sections 4.1, 4.2). Format: vectors/README.\n\
         #\n\
         # PUBLIC TEST SECRET. The secret key below is published on purpose so that\n\
         # anyone can check signing. Never use it, or any key derived from it, for\n\
         # anything but tests: every entry it signs can be forged by anyone.\n",
    );
    let _ = writeln!(out, "\nkey test\n  secret-seed {}\n  feed-key {}", hex(&std::array::from_fn::<u8, 32, _>(|i| i as u8)), hex(&key));
    let other = other_identity();
    let _ = writeln!(
        out,
        "\nkey other\n  secret-seed {}\n  feed-key {}",
        hex(&std::array::from_fn::<u8, 32, _>(|i| 0x20 + i as u8)),
        hex(&other.feed_key())
    );

    // Valid entries.
    let hello = hello_entry();
    valid_entry(&mut out, "article", &hello, HELLO);
    valid_entry(&mut out, "profile", &profile_entry(), PROFILE_DOC);
    let image = b"TERS not a valid image: still a valid program entry";
    valid_entry(&mut out, "program", &signed(Entry::program(key, 3, TIME + 120, image, "Sketch pad"), &id), image);
    let doc = b"TDOC\x00 a document";
    valid_entry(&mut out, "document", &signed(Entry::document(key, 4, TIME + 180, doc, "Notes, 1st draft"), &id), doc);
    let reply = b"A reply.\n";
    let e = Entry::article(key, 5, TIME + 240, reply, Some(hello.entry.blob), "Re: Hello");
    valid_entry(&mut out, "article-reply", &signed(e, &id), reply);
    let untitled = b"No title.\n";
    valid_entry(&mut out, "article-no-title", &signed(Entry::article(key, 6, TIME, untitled, None, ""), &id), untitled);
    let t120 = "é".repeat(60);
    valid_entry(&mut out, "article-title-120", &signed(Entry::article(key, 7, TIME, HELLO, None, &t120), &id), HELLO);
    let big: Vec<u8> = (0..blob::MAX_BLOB).map(|i| (i % 251) as u8).collect();
    valid_entry(&mut out, "article-largest-blob", &signed(Entry::article(key, 8, TIME, &big, None, "Big"), &id), &big);
    valid_entry(&mut out, "article-seq-gap", &signed(Entry::article(key, 4_000_000_000, 0, HELLO, None, "Late"), &id), HELLO);
    // A kind this revision does not define: valid, kept and passed on,
    // never opened (NETWORK.md 4.2, 6). Its title need not be a name.
    valid_entry(&mut out, "unknown-kind-9", &unknown_kind_entry(), LATER);

    // Invalid entries: one rule broken each, signed as they are (so that
    // only that rule fails) unless the signature is the point.
    let b = &hello.bytes;
    let unsigned = |b: &[u8]| b[..b.len() - 64].to_vec();
    let with = |at: usize, v: &[u8]| {
        let mut u = unsigned(b);
        u[at..at + v.len()].copy_from_slice(v);
        sign_raw(&u, &id)
    };
    let mut flipped = b.clone();
    *flipped.last_mut().unwrap() ^= 1;
    invalid_entry(&mut out, "bad-signature", &flipped, "the signature does not verify (its last byte changed)", Some(Invalid::Signature));
    let mut changed = b.clone();
    changed[109] = b'J';
    invalid_entry(&mut out, "changed-title", &changed, "the title changed after signing: the signature does not verify", Some(Invalid::Signature));
    let by_other = sign_raw(&unsigned(b), &other);
    invalid_entry(&mut out, "signed-by-another-key", &by_other, "signed by the other test key, not the feed key", Some(Invalid::Signature));
    // A non-canonical signature: S + L, where L is the order of the group.
    const L: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0x10,
    ];
    let mut nc = b.clone();
    let s_at = nc.len() - 32;
    let mut carry = 0u16;
    for i in 0..32 {
        let v = nc[s_at + i] as u16 + L[i] as u16 + carry;
        nc[s_at + i] = v as u8;
        carry = v >> 8;
    }
    invalid_entry(&mut out, "non-canonical-signature", &nc, "the signature's S is not below the group order (S + L): strict verification refuses it", Some(Invalid::Signature));
    let mut small = unsigned(b);
    small[..32].copy_from_slice(&{
        let mut k = [0u8; 32];
        k[0] = 1;
        k
    });
    let small = [&small[..], &b[b.len() - 64..]].concat();
    invalid_entry(&mut out, "small-order-key", &small, "the feed key is a point of small order (the neutral point)", None);
    invalid_entry(&mut out, "kind-0", &with(40, &[0]), "kind 0 is reserved", Some(Invalid::Kind));
    invalid_entry(&mut out, "kind-255", &reserved_kind_entry(), "kind 255 is reserved", Some(Invalid::Kind));
    invalid_entry(&mut out, "seq-0", &with(32, &0u32.to_le_bytes()), "seq is 0", Some(Invalid::Seq));
    invalid_entry(&mut out, "size-0", &with(41, &[0, 0, 0]), "the size is 0", Some(Invalid::Size));
    let over = (blob::MAX_BLOB as u32 + 1).to_le_bytes();
    invalid_entry(&mut out, "size-over-1-mib", &with(41, &over[..3]), "the size is 1,048,577", Some(Invalid::Size));
    let p = profile_entry();
    let mut u = unsigned(&p.bytes);
    u[41..44].copy_from_slice(&4097u32.to_le_bytes()[..3]);
    invalid_entry(&mut out, "profile-size-4097", &sign_raw(&u, &id), "a profile's size is over 4,096", Some(Invalid::Size));
    let retitled = |e: &SignedEntry, title: &[u8]| {
        let mut u = e.bytes[..108].to_vec();
        u.push(title.len() as u8);
        u.extend_from_slice(title);
        sign_raw(&u, &id)
    };
    invalid_entry(&mut out, "profile-title-not-a-name", &retitled(&p, b" Alice"), "a profile's title is not a display name (a space first)", Some(Invalid::Name));
    let prog = signed(Entry::program(key, 3, TIME + 120, image, "Sketch pad"), &id);
    invalid_entry(&mut out, "program-title-empty", &retitled(&prog, b""), "a program's title is not a display name (empty)", Some(Invalid::Name));
    invalid_entry(&mut out, "title-not-utf8", &retitled(&hello, b"Hell\xff"), "the title is not valid UTF-8", Some(Invalid::TitleUtf8));
    let long = vec![b'x'; 121];
    invalid_entry(&mut out, "title-121", &retitled(&hello, &long), "n is 121: titles are at most 120 bytes", Some(Invalid::TitleLength));
    let mut short = b.clone();
    short[108] = 6;
    invalid_entry(&mut out, "length-mismatch", &short, "n says 6 title bytes but the entry holds 5 (its length is not 173 + n)", Some(Invalid::Length));
    invalid_entry(&mut out, "truncated", &b[..b.len() - 1], "one byte short", Some(Invalid::Length));
    invalid_entry(&mut out, "too-short", &b[..100], "shorter than the fixed part", Some(Invalid::Length));
    out
}

// --- Profiles and names (NETWORK.md 4.6) -------------------------------------------

fn profiles_file() -> String {
    let mut out = String::from(
        "# Terse test vectors: display names and profile documents (NETWORK.md section 4.6).\n\
         # Format: vectors/README.\n",
    );
    let names: Vec<(&[u8], bool, &str)> = vec![
        (b"Alice", true, ""),
        (b"Alice Smith", true, "a space inside is allowed"),
        ("\u{e9}".repeat(24).leak().as_bytes(), true, "48 bytes"),
        ("x".repeat(48).leak().as_bytes(), true, "48 bytes"),
        (b"", false, "empty"),
        ("x".repeat(49).leak().as_bytes(), false, "49 bytes"),
        (b" Alice", false, "whitespace first"),
        (b"Alice\t", false, "whitespace last (tab)"),
        ("\u{a0}Alice".as_bytes(), false, "whitespace first (U+00A0)"),
        ("Alice\u{3000}".as_bytes(), false, "whitespace last (U+3000)"),
        ("Alice\u{2028}".as_bytes(), false, "whitespace last (U+2028)"),
        ("\u{200b}Alice".as_bytes(), true, "U+200B is not White_Space"),
        (b"Al\x01ice", false, "a control character"),
        ("Al\u{85}ice".as_bytes(), false, "a control character (U+0085)"),
        (b"Al\xffice", false, "not UTF-8"),
    ];
    for (bytes, valid, note) in names {
        assert_eq!(std::str::from_utf8(bytes).is_ok_and(profile::valid_name), valid, "{bytes:?}");
        assert_eq!(article_parse::display_name(bytes).is_some(), valid, "{bytes:?}");
        let _ = writeln!(out, "\nname {}", hex(bytes));
        let _ = writeln!(out, "  valid {}", if valid { "yes" } else { "no" });
        if !note.is_empty() {
            let _ = writeln!(out, "  note {}", article_parse::quote(note));
        }
    }
    let docs: Vec<(&str, Vec<u8>, &str)> = vec![
        ("listed", b"listed: yes\n".to_vec(), ""),
        ("full", b"listed: yes\nrelay: ws://relay.example:7878\nrelay: wss://relay.example/terse\n\nI write about bread.\nlisted: no\n".to_vec(), "after the empty line everything is description"),
        ("unlisted", b"listed: no\n".to_vec(), ""),
        ("listed-capital", b"listed: Yes\n".to_vec(), "only `yes` lists"),
        ("no-listed-line", b"relay: ws://relay.example\n".to_vec(), ""),
        ("no-final-line-break", b"listed: yes".to_vec(), ""),
        ("crlf", b"listed: yes\r\n".to_vec(), "lines end at LF: the value is `yes` and a CR, which does not list"),
        ("unknown-field", b"colour: blue\nlisted: yes\n".to_vec(), "unknown fields are ignored"),
        (
            "relays",
            b"relay: ws://a.example\nrelay: http://b.example\nrelay: wss://c.example\nrelay: ws://d.example\nrelay: ws://e.example\nrelay: ws://f.example\n".to_vec(),
            "the first four that are ws:// or wss:// URLs",
        ),
        ("description-only", b"\nJust a description.\n".to_vec(), "no header lines"),
        ("no-space", b"listed:yes\n".to_vec(), "a header line without `: `"),
        ("not-utf8", b"listed: yes\n\n\xff\n".to_vec(), "not UTF-8"),
        ("too-long", [b"listed: yes\n\n".as_slice(), &vec![b'x'; 4096 - 12]].concat(), "4,097 bytes"),
        ("longest", [b"listed: yes\n\n".as_slice(), &vec![b'x'; 4096 - 13]].concat(), "4,096 bytes"),
    ];
    for (name, bytes, note) in docs {
        let parsed = profile::Document::parse(&bytes);
        let _ = writeln!(out, "\ndocument {name}");
        if bytes.len() <= 256 {
            let _ = writeln!(out, "  hex {}", hex(&bytes));
        } else {
            let head = bytes.iter().take_while(|&&b| b != b'x').count();
            let _ = writeln!(out, "  hex-then-x {} {}", hex(&bytes[..head]), bytes.len() - head);
        }
        if !note.is_empty() {
            let _ = writeln!(out, "  note {}", article_parse::quote(note));
        }
        match parsed {
            None => out.push_str("  valid no\n"),
            Some(d) => {
                out.push_str("  valid yes\n");
                let _ = writeln!(out, "  listed {}", if d.listed { "yes" } else { "no" });
                for r in &d.relays {
                    let _ = writeln!(out, "  relay {r}");
                }
                if d.description.len() <= 256 {
                    let _ = writeln!(out, "  description {}", article_parse::quote(&d.description));
                } else {
                    let _ = writeln!(out, "  description-bytes {}", d.description.len());
                }
            }
        }
    }
    out
}

// --- A relay session (NETWORK.md 3, 4.3, 4.7, 4.8, 5.2) ------------------------------

/// The messages of a short session between a client and a fresh relay:
/// (direction, commentary, bytes); the direction is `c>` (client to
/// relay), `r>` (relay to client) or `p>` (relay to relay).
pub fn session() -> Vec<(&'static str, String, Vec<u8>)> {
    use protocol::*;
    let id = test_identity();
    let key = id.feed_key();
    let hello = hello_entry();
    let profile = profile_entry();
    let other = signed(Entry::article(key, 1, TIME, b"Other", None, "Other"), &id);
    let missing = blob::blob_id(b"missing");
    vec![
        ("c>", "Put, req 1: the blob \"Hello, Terse!\"".into(), encode_put(1, HELLO)),
        ("r>", "Put reply, req 1: done, and the blob's ID".into(), encode_put_reply(1, status::DONE, &blob::blob_id(HELLO))),
        ("c>", "Put, req 2: no bytes".into(), encode_put(2, b"")),
        ("r>", "Put reply, req 2: refused (a blob is 1 to 1,048,576 bytes); no ID".into(), encode_put_reply(2, status::REFUSED, &[0; 32])),
        ("c>", "Publish, req 3: the entry `article` of entries.txt (seq 1)".into(), encode_publish(3, &hello.bytes)),
        ("r>", "Publish reply, req 3: done".into(), encode_status_reply(PUBLISH_REPLY, 3, status::DONE)),
        ("c>", "Put, req 4: the profile document \"listed: yes\\n\"".into(), encode_put(4, PROFILE_DOC)),
        ("r>", "Put reply, req 4: done".into(), encode_put_reply(4, status::DONE, &blob::blob_id(PROFILE_DOC))),
        ("c>", "Publish, req 5: the entry `profile` of entries.txt (seq 2, \"Alice\")".into(), encode_publish(5, &profile.bytes)),
        ("r>", "Publish reply, req 5: done".into(), encode_status_reply(PUBLISH_REPLY, 5, status::DONE)),
        ("c>", "Publish, req 6: a different valid entry with seq 1".into(), encode_publish(6, &other.bytes)),
        ("r>", "Publish reply, req 6: refused (the relay holds another entry with that seq)".into(), encode_status_reply(PUBLISH_REPLY, 6, status::REFUSED)),
        ("c>", "Subscribe, req 7: the test feed, after 0".into(), encode_subscribe(7, &key, 0)),
        ("r>", "Subscribe reply, req 7: done, highest 2".into(), encode_subscribe_reply(7, status::DONE, 2)),
        ("r>", "Entry: seq 1, pushed after the reply".into(), encode_entry(&hello.bytes)),
        ("r>", "Entry: seq 2; the client has caught up (2 is highest)".into(), encode_entry(&profile.bytes)),
        ("c>", "Get, req 8: the blob of seq 1".into(), encode_get(8, &hello.entry.blob)),
        ("r>", "Get reply, req 8: done, the blob".into(), encode_get_reply(8, status::DONE, HELLO)),
        ("c>", "Get, req 9: the ID of \"missing\", which the relay does not hold".into(), encode_get(9, &missing)),
        ("r>", "Get reply, req 9: not found".into(), encode_get_reply(9, status::NOT_FOUND, b"")),
        ("c>", "Find, req 10: scope 1 (feeds by name), \"ali\"".into(), encode_find(10, SCOPE_FEEDS, b"ali")),
        ("r>", "Find reply, req 10: done, 1 result: the profile in force".into(), encode_find_reply(10, status::DONE, &[&profile.bytes])),
        ("c>", "Find, req 11: scope 2 (titles), \"HELLO\" (ASCII letters match either case)".into(), encode_find(11, SCOPE_ENTRIES, b"HELLO")),
        ("r>", "Find reply, req 11: done, 1 result".into(), encode_find_reply(11, status::DONE, &[&hello.bytes])),
        ("c>", "Find, req 12: scope 4, which does not exist".into(), encode_find(12, 4, b"x")),
        ("r>", "Find reply, req 12: refused, count 0".into(), encode_find_reply::<&[u8]>(12, status::REFUSED, &[])),
        ("c>", "Lookup, req 13: the test feed's program \"Pad\"".into(), encode_lookup(13, &key, 3, b"Pad")),
        ("r>", "Lookup reply, req 13: not found".into(), encode_lookup_reply(13, status::NOT_FOUND, b"")),
        ("c>", "Unsubscribe, req 14: the test feed".into(), encode_unsubscribe(14, &key)),
        ("r>", "Unsubscribe reply, req 14: done".into(), encode_status_reply(UNSUBSCRIBE_REPLY, 14, status::DONE)),
        ("c>", "Publish, req 15: the entry `unknown-kind-9` of entries.txt (seq 9, a kind not defined)".into(), encode_publish(15, &unknown_kind_entry().bytes)),
        ("r>", "Publish reply, req 15: done (a kind not defined is stored like any other)".into(), encode_status_reply(PUBLISH_REPLY, 15, status::DONE)),
        ("c>", "Publish, req 16: the entry `kind-255` of entries.txt".into(), encode_publish(16, &reserved_kind_entry())),
        ("r>", "Publish reply, req 16: refused (kind 255 is reserved: the entry is invalid)".into(), encode_status_reply(PUBLISH_REPLY, 16, status::REFUSED)),
        ("p>", "Peer (on a connection from another relay; no reply)".into(), encode_peer()),
    ]
}

fn session_file() -> String {
    let mut out = String::from(
        "# Terse test vectors: the messages of a short session with a relay that holds\n\
         # nothing at the start (NETWORK.md sections 3, 4.3, 4.7, 4.8, 5.2), in order.\n\
         # Format: vectors/README.\n",
    );
    for (dir, note, bytes) in session() {
        let _ = writeln!(out, "\n# {note}\n{dir} {}", hex(&bytes));
    }
    out
}

// --- Everything ----------------------------------------------------------------
// --- Everything ----------------------------------------------------------------

/// Every file of the vectors directory, in a fixed order.
pub fn generate() -> Vec<File> {
    let mut out = vec![
        File { path: "README".into(), bytes: README.as_bytes().to_vec() },
        File { path: "blobs.txt".into(), bytes: blobs_file().into_bytes() },
        File { path: "entries.txt".into(), bytes: entries_file().into_bytes() },
        File { path: "profiles.txt".into(), bytes: profiles_file().into_bytes() },
        File { path: "relay-session.txt".into(), bytes: session_file().into_bytes() },
    ];
    out.extend(article_files());
    out
}

/// Writes the vectors into `dir`, replacing files of the same names; files
/// that are no longer generated are removed (from `dir` and `dir/articles`).
pub fn write(dir: &Path) -> std::io::Result<()> {
    let files = generate();
    std::fs::create_dir_all(dir.join("articles"))?;
    for sub in [dir.to_path_buf(), dir.join("articles")] {
        for e in std::fs::read_dir(&sub)? {
            let e = e?;
            if e.file_type()?.is_file() {
                let rel = e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                if !files.iter().any(|f| f.path == rel) {
                    std::fs::remove_file(e.path())?;
                }
            }
        }
    }
    for f in &files {
        std::fs::write(dir.join(&f.path), &f.bytes)?;
    }
    Ok(())
}

/// `vectors/README`.
pub const README: &str = "\
Terse core test vectors
=======================

Test vectors for the core of Terse: the network (NETWORK.md) and the
article format (ARTICLE.md). They are for anyone writing a client, a relay
or a reader, in any language, to check an implementation against. Nothing
here needs the Terse machine (SPEC.md).

This directory is generated by `terse vectors` (crates/terse-net/src/
vectors.rs) and checked by the test suite (crates/terse-cli/tests/
vectors.rs): do not edit it by hand. Where these files and NETWORK.md or
ARTICLE.md disagree, the documents win and the vectors are wrong.

Conventions, in every file:
- Text files, UTF-8, lines end with LF. A line starting with `#` is a
  comment; blank lines separate records.
- A record starts with an unindented line `<type> <name>`; its fields
  follow, one per line, indented by two spaces: `<field> <value>`.
- Bytes are written as lowercase hexadecimal digits, two per byte, with
  no separators; `-` stands for no bytes.
- A quoted string is in double quotes, with `\\\\`, `\\\"`, `\\t`, `\\n` and
  `\\r` for a backslash, a double quote, a tab, an LF and a CR; every other
  character is itself (UTF-8).
- `pattern mod251` (or `blob-pattern mod251`) stands for bytes too long to
  write out: byte i (from 0) is i mod 251.

blobs.txt (NETWORK.md section 1)
--------------------------------
Records `blob <name>` with `size` (bytes), `hex` (the bytes, when 64 or
fewer) or `pattern mod251`, `valid` (`yes` if 1 to 1,048,576 bytes, a
size a blob may have) and `blake3`: the BLAKE3-256 hash of the bytes,
which for a valid blob is its ID. Sizes 1024 and 1025 cross BLAKE3's
chunk boundary.

entries.txt (NETWORK.md sections 4.1, 4.2)
-----------------------------------------
Records `key <name>` give a `secret-seed` (the 32-byte Ed25519 secret
key seed, RFC 8032) and its `feed-key`. These are PUBLIC TEST SECRETS:
never use them for anything but tests.

Records `entry <name>` with `valid yes` are entries signed with the key
`test`. Their fields: `feed-key`, `seq`, `time`, `kind`, `size`, the blob
(`blob-hex`, or `blob-pattern mod251`), `blob-id`, `reply-to`, `title`
(quoted), `signed-message` (the 13 bytes `terse-entry-0` and bytes 0 to
108 + n of the entry: what is signed), `signature` and `bytes` (the whole
entry). Ed25519 signatures are deterministic, so an implementation that
signs the same fields with the same key must produce exactly these bytes.

Records `entry <name>` with `valid no` give `bytes` and the `reason` the
entry is invalid; each breaks one rule of NETWORK.md 4.2 (most are signed
as they are, so that only that rule fails). A client or relay must
discard every one of them. The valid entry `unknown-kind-9` is of a kind
NETWORK.md does not define: a relay must store and pass it on like any
other, and a client keep or skip it without error and not open it.

profiles.txt (NETWORK.md section 4.6)
-------------------------------------
Records `name <hex>`: a title's bytes, `valid` (whether they are a
display name) and maybe a `note`.

Records `document <name>`: a profile document's bytes (`hex`, or for a
long one `hex-then-x <hex> <n>`: those bytes, then n bytes `x`), maybe a
`note`, and the result of parsing it: `valid no`, or `valid yes` with
`listed` (`yes` or `no`), one `relay <url>` line per relay URL a reader
takes (at most four, in order), and `description` (quoted; or
`description-bytes <n>` for a long one): the text after the first empty
line, empty if there is none.

relay-session.txt (NETWORK.md sections 3, 4.3, 4.7, 4.8, 5.2)
-------------------------------------------------------------
The messages of one session, in order, between a client and a relay
that holds nothing at the start (and that keeps no entries but those
shown). Each message is a comment line saying what it is, then a line
`<direction> <hex>`: `c>` from the client to the relay, `r>` from the
relay to the client, `p>` from a relay to another relay (on a connection
of its own). Each message is one binary WebSocket message. The entries
are those of entries.txt. A relay that answers the client messages in
this order must send exactly the `r>` messages (the test suite checks
this against the Terse relay).

articles/ (ARTICLE.md)
----------------------
An article corpus: each `<name>.txt` is an article (bytes as they are,
some not valid UTF-8, some with CRs) and `<name>.dump` what ARTICLE.md
says a reader must show for it, in the dump format below. Each case
shows one construct of ARTICLE.md and its near misses. The dumps come
from a reference parser written from ARTICLE.md (crates/terse-net/src/
article_parse.rs).

The link targets used are 0123456789abcdef repeated (an article),
fedcba9876543210 repeated (a feed) and c0ffee00 repeated (a program's
publisher).

Article dumps
-------------
A dump is the article's blocks in order, as the reader must show them.
A blob that starts with `TERS` or `TDOC` is not shown as an article; its
dump is the one line `not-an-article program-image` or
`not-an-article document`. Otherwise each block is an unindented line
naming its kind, followed by its content indented by two spaces:

  heading              a heading (`# `); then its inline content
  subheading           a subheading (`## `); then its inline content
  paragraph            then its inline content (lines joined)
  item                 a list item; then its inline content
  pre                  a preformatted block, a fence, or a figure shown
                       as its fence: one `line` per line (blank lines of a
                       fence included), each followed by the line's inline
                       content (text and links only) indented two more
  table <c> header|noheader
                       c columns (1 to 8); `header` if there is an
                       alignment row (the first row is then the header);
                       then `align` and each column's alignment (`start`,
                       `centre` or `end`), then one `row` per row shown,
                       each with exactly c `cell` lines, each followed by
                       the cell's inline content indented two more
  figure <w> <h>       a figure that is drawn: its size in units, then
                       one `path` per path that draws something

A fence with no lines shows nothing and has no block. A heading with no
text is a `heading` line with no content.

Inline content is one line per piece:

  text \"...\"          ordinary text, as shown: lines joined (each line
                       break a space, the blanks at the start of each line
                       dropped), a CR before an LF and control characters
                       other than tab and CR removed, a lone CR (not before
                       an LF) as a space, a byte order mark that starts the
                       article removed, malformed UTF-8 as U+FFFD (the maximal
                       subpart rule). Runs of blanks are as written; a
                       reader may show each run as one space, and blanks
                       at the start or end of a block are not seen, so
                       compare after collapsing runs of blanks (tabs
                       included) to one space and trimming each block's
                       or cell's text at its ends. In `pre` lines the text
                       is exact (but a tab may be shown as spaces).
  code \"...\"          a code span: the bytes between the backquotes
  link article <id>    a bare link (shown as written, or shortened)
  link feed <key>
  link program <key> \"<name>\"
                       the name decoded from its %XX escapes
  link ... as \"...\"   a link with text: the target as above, then `as`
                       and the text that is shown

A `path` line is `path fill <colour> stroke <colour> <width>` or
`path fill <colour> stroke none`: the fill and stroke in force for the
path. A colour is `none` (nothing drawn: `none`, or any colour with
opacity 0), `fg` (the reader's body text colour) or `#rrggbbaa`; the
width is in sixteenths of a unit (a stroke of width 0 is `stroke none`).
Paths that draw nothing (no segment, or neither fill nor stroke) are not
listed. Each path's commands follow, indented by four spaces, one per
line, absolute, every coordinate in sixteenths of a unit (numbers
rounded as ARTICLE.md 6.1 says, relative coordinates added up):

  M x y                starts a subpath (written out also where a segment
                       starts one implicitly: at the start, after `Z` or
                       after a word)
  L x y                a line (also `H` and `V`)
  Q x1 y1 x y          a quadratic curve (also `T`, its control point
                       reflected)
  C x1 y1 x2 y2 x y    a cubic curve (also `S`, its first control point
                       reflected)
  Z                    closes the subpath

A figure that is malformed or not closed is not a `figure`: it is shown
as its fence (`pre`).
";
