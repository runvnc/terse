//! The implementation guide's reader examples, examples/reader/go and
//! examples/reader/rust, built and run as the guide runs them:
//!
//! - step 1 (`show`): every article of vectors/articles that holds only
//!   headings, subheadings and paragraphs of plain text is shown as its
//!   dump says (the blocks and their text, wrapped to 68 columns), and
//!   examples/reader/hello.txt exactly as the guide shows it;
//! - step 2 (`fetch`): an article put on a `terse-relay` (random port,
//!   scratch store) is shown; an ID the relay lacks, a blob that fails the
//!   hash check and a program image are refused, with nothing shown;
//! - step 3 (`follow`): a feed published with `terse feed publish` is
//!   listed and an entry opened; every entry of vectors/entries.txt, sent
//!   by a fake relay, is listed if and only if it is a valid article entry
//!   (so the strict Ed25519 checks are all made).
//!
//! The Go examples need Go and their modules (go.sum; downloaded once into
//! Go's module cache), the Rust ones the crates of their Cargo.lock (the
//! versions of the workspace's): each test skips, saying so, when it
//! cannot get them. `articles_show_the_examples` checks that the guide's
//! pages show each file whole and unchanged.

use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use terse_net::blob::{blob_id, to_hex};
use terse_testkit::{program, skip, workspace};
use tungstenite::Message;

const WIDTH: usize = 68;

/// The guide's pages and the files each shows, under examples/reader.
const PAGES: [(&str, &[&str]); 6] = [
    ("guide-reader-go", &["go/article/article.go", "go/show/main.go"]),
    ("guide-reader-fetch-go", &["go/go.mod", "go/relay/relay.go", "go/fetch/main.go"]),
    ("guide-reader-feed-go", &["go/follow/main.go"]),
    ("guide-reader-rust", &["rust/src/lib.rs", "rust/src/article.rs", "rust/src/bin/show.rs"]),
    ("guide-reader-fetch-rust", &["rust/Cargo.toml", "rust/src/relay.rs", "rust/src/bin/fetch.rs"]),
    ("guide-reader-feed-rust", &["rust/src/bin/follow.rs"]),
];

/// What `show examples/reader/hello.txt` prints (the guide shows it).
const HELLO: &str = "Hello
=====

My first article. It has a heading, and this paragraph, which is
written on two lines.

A subheading
------------

A reader joins the lines of a paragraph into one, then wraps it
again to the width of its own column, so the lines you see may not
be the lines written.

";

fn examples() -> PathBuf {
    workspace().join("examples/reader")
}

#[test]
fn go_examples() {
    if !Command::new("go").arg("version").output().is_ok_and(|o| o.status.success()) {
        skip!("go", "needs Go (`go` not found) to build examples/reader/go");
        return;
    }
    let dir = examples().join("go");
    let go = |args: &[&str]| {
        Command::new("go").args(args).current_dir(&dir).env("GOFLAGS", "-mod=readonly").env("CGO_ENABLED", "0").output().unwrap()
    };
    let download = go(&["mod", "download"]);
    if !download.status.success() {
        let why = String::from_utf8_lossy(&download.stderr);
        skip!("go-modules", "cannot get the Go modules of examples/reader/go/go.sum (offline?): {}", why.trim());
        return;
    }
    let bin = tempfile::tempdir().unwrap();
    let out = format!("{}/", bin.path().display());
    let built = go(&["build", "-p", "4", "-o", &out, "./show", "./fetch", "./follow"]);
    assert!(built.status.success(), "go build: {}", String::from_utf8_lossy(&built.stderr));
    check(bin.path());
}

#[test]
fn rust_examples() {
    let manifest = examples().join("rust/Cargo.toml");
    let target = terse_testkit::skip_log().parent().unwrap().join("reader-examples");
    let cargo = || {
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.arg("--quiet").env("CARGO_TARGET_DIR", &target);
        cmd
    };
    let fetch = cargo().args(["fetch", "--locked", "--manifest-path"]).arg(&manifest).output().unwrap();
    if !fetch.status.success() {
        let why = String::from_utf8_lossy(&fetch.stderr);
        skip!("crates", "cannot get the crates of examples/reader/rust/Cargo.lock (offline?): {}", why.trim());
        return;
    }
    let built = cargo().args(["build", "--locked", "--offline", "--bins", "--manifest-path"]).arg(&manifest).output().unwrap();
    assert!(built.status.success(), "cargo build: {}", String::from_utf8_lossy(&built.stderr));
    check(&target.join("debug"));
}

#[test]
fn articles_show_the_examples() {
    for (page, files) in PAGES {
        let text = std::fs::read_to_string(workspace().join(format!("docs/manual/{page}.txt"))).unwrap();
        let fences = fences(&text);
        for file in files {
            let source = std::fs::read_to_string(examples().join(file)).unwrap().replace('\t', "    ");
            for (n, line) in source.lines().enumerate() {
                assert!(line.chars().count() <= 64, "{file}:{}: wider than 64 characters", n + 1);
            }
            assert!(fences.contains(&source), "{page} does not show {file} as it is (tabs as four spaces)");
        }
    }
}

/// The contents of every fence of an article, each line with its LF.
fn fences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut body: Option<String> = None;
    for line in text.lines() {
        match &mut body {
            None if line.starts_with("```") => body = Some(String::new()),
            None => {}
            Some(_) if line.trim_end() == "```" => out.extend(body.take()),
            Some(b) => {
                b.push_str(line);
                b.push('\n');
            }
        }
    }
    out
}

/// Runs the three programs in `bin`.
fn check(bin: &Path) {
    let exe = |name: &str| bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let (show, fetch, follow) = (exe("show"), exe("fetch"), exe("follow"));
    check_show(&show);
    let scratch = tempfile::tempdir().unwrap();
    let relay = Relay::start(&scratch.path().join("relay"));
    let terse_here = |args: &[&str]| terse(scratch.path(), args);

    // Step 2.
    let hello = examples().join("hello.txt");
    let id = terse_here(&["put", hello.to_str().unwrap(), "--relay", &relay.url]).trim().to_string();
    assert_eq!(ok(run(&fetch, &[&relay.url, &id])), HELLO);
    let missing = "00".repeat(32);
    refused(run(&fetch, &[&relay.url, &missing]), "the relay does not have it");
    let liar = fake_relay(Vec::new(), b"not hello".to_vec());
    refused(run(&fetch, &[&liar, &id]), "hash check failed");
    let image = b"TERS and the rest of a program image".to_vec();
    let image_relay = fake_relay(Vec::new(), image.clone());
    refused(run(&fetch, &[&image_relay, &to_hex(&blob_id(&image))]), "not an article");

    // Step 3, on a real relay.
    let key = terse_here(&["key", "new"]).trim().to_string();
    terse_here(&["feed", "publish", hello.to_str().unwrap(), "--title", "Hello", "--relay", &relay.url]);
    let second = scratch.path().join("second.txt");
    std::fs::write(&second, "# Second\n\nAnother article.\n").unwrap();
    terse_here(&["feed", "publish", second.to_str().unwrap(), "--title", "Second", "--relay", &relay.url]);
    assert_eq!(ok(run(&follow, &[&relay.url, &key])), "1  Hello\n2  Second\n");
    let opened = ok(run(&follow, &[&relay.url, &key, "2"]));
    assert_eq!(opened, "1  Hello\n2  Second\n\nSecond\n======\n\nAnother article.\n\n");

    // Step 3, every entry vector from a fake relay.
    let vectors = std::fs::read_to_string(workspace().join("vectors/entries.txt")).unwrap();
    let mut checked = 0;
    for record in vectors.split("\n\n").filter(|r| r.starts_with("entry ")) {
        let field = |name: &str| record.lines().find_map(|l| l.strip_prefix(&format!("  {name} ")));
        let bytes = hex(field("bytes").unwrap());
        if bytes.len() < 32 {
            continue;
        }
        let shown = if field("valid") == Some("yes") && field("kind") == Some("1") {
            format!("{}  {}\n", field("seq").unwrap(), visible(&unquote(field("title").unwrap())))
        } else {
            String::new()
        };
        let url = fake_relay(bytes.clone(), Vec::new());
        let out = ok(run(&follow, &[&url, &to_hex(&bytes[..32].try_into().unwrap())]));
        assert_eq!(out, shown, "follow, for the entry {}", record.lines().next().unwrap());
        checked += 1;
    }
    assert!(checked > 30, "only {checked} entry vectors");
}

/// Step 1 on hello.txt and on the vectors it handles in full: those whose
/// text starts no other block and whose dump holds only headings,
/// subheadings and paragraphs of plain text.
fn check_show(show: &Path) {
    assert_eq!(ok(run(show, &[examples().join("hello.txt").to_str().unwrap()])), HELLO);
    let mut shown = 0;
    for dump in std::fs::read_dir(workspace().join("vectors/articles")).unwrap() {
        let dump = dump.unwrap().path();
        if dump.extension().is_none_or(|e| e != "dump") {
            continue;
        }
        let article = dump.with_extension("txt");
        let other_blocks = |l: &[u8]| ["```", "    ", "\t", "|", "- ", "* "].iter().any(|m| l.starts_with(m.as_bytes()));
        if std::fs::read(&article).unwrap().split(|&b| b == b'\n').any(other_blocks) {
            continue;
        }
        let Some(blocks) = simple_blocks(&std::fs::read_to_string(&dump).unwrap()) else { continue };
        let out = ok(run(show, &[article.to_str().unwrap()]));
        assert_eq!(out, render(&blocks), "show {}", article.display());
        assert!(out.lines().all(|l| l.chars().count() <= WIDTH || !l.contains(' ')));
        shown += 1;
    }
    assert!(shown >= 5, "only {shown} vectors with headings and paragraphs alone");
}

/// The blocks of a dump that holds only headings, subheadings and
/// paragraphs of plain text (with the underline of each heading), or None.
fn simple_blocks(dump: &str) -> Option<Vec<(Option<char>, String)>> {
    let mut blocks: Vec<(Option<char>, String)> = Vec::new();
    for line in dump.lines() {
        if let Some(text) = line.strip_prefix("  text ") {
            blocks.last_mut()?.1.push_str(&unquote(text));
            continue;
        }
        let rule = match line {
            "heading" => Some('='),
            "subheading" => Some('-'),
            "paragraph" => None,
            _ => return None,
        };
        blocks.push((rule, String::new()));
    }
    Some(blocks)
}

/// What the examples print for these blocks.
fn render(blocks: &[(Option<char>, String)]) -> String {
    let mut out = String::new();
    for (rule, text) in blocks {
        let mut lines: Vec<String> = vec![String::new()];
        for word in visible(text).split(' ').filter(|w| !w.is_empty()) {
            let line = lines.last_mut().unwrap();
            if line.is_empty() {
                line.push_str(word);
            } else if line.chars().count() + 1 + word.chars().count() > WIDTH {
                lines.push(word.to_string());
            } else {
                line.push(' ');
                line.push_str(word);
            }
        }
        if let Some(rule) = rule {
            let n = lines.join(" ").chars().count().min(WIDTH);
            lines.push(rule.to_string().repeat(n));
        }
        out += &format!("{}\n\n", lines.join("\n"));
    }
    out
}

/// Text as ARTICLE.md section 1 shows it: tabs and CRs as spaces, no
/// other control characters.
fn visible(text: &str) -> String {
    text.chars().filter_map(|c| if c == '\t' || c == '\r' { Some(' ') } else { (!c.is_control()).then_some(c) }).collect()
}

/// A quoted string of vectors/README.
fn unquote(quoted: &str) -> String {
    let mut out = String::new();
    let mut chars = quoted.trim().strip_prefix('"').unwrap().strip_suffix('"').unwrap().chars();
    while let Some(c) = chars.next() {
        out.push(match (c, c == '\\') {
            (_, true) => match chars.next().unwrap() {
                't' => '\t',
                'n' => '\n',
                'r' => '\r',
                other => other,
            },
            (c, false) => c,
        });
    }
    out
}

fn hex(text: &str) -> Vec<u8> {
    if text == "-" {
        return Vec::new();
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

/// Runs `exe` with `args`, killing it after 30 seconds.
fn run(exe: &Path, args: &[&str]) -> Output {
    let mut child: Child =
        Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        stdout.read_to_end(&mut b).unwrap();
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = Vec::new();
        stderr.read_to_end(&mut b).unwrap();
        b
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("{} {args:?} did not finish in 30 seconds", exe.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Output { status, stdout: out.join().unwrap(), stderr: err.join().unwrap() }
}

fn ok(out: Output) -> String {
    assert!(out.status.success(), "failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// A run that failed with `why` and showed nothing.
fn refused(out: Output, why: &str) {
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && err.contains(why), "expected a failure with {why:?}: {err}");
    assert!(out.stdout.is_empty(), "shown although refused: {}", String::from_utf8_lossy(&out.stdout));
}

/// `terse` with every directory of its own under `home`, and no local
/// relay port of the user's.
fn terse(home: &Path, args: &[&str]) -> String {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let out = Command::new(program("terse"))
        .args(args)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env(terse_net::local::PORT_ENV, unused.to_string())
        .output()
        .unwrap();
    assert!(out.status.success(), "terse {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// `terse-relay` on a random loopback port.
struct Relay {
    child: Child,
    url: String,
}

impl Relay {
    fn start(store: &Path) -> Relay {
        let mut child = Command::new(program("terse-relay"))
            .args(["--listen", "127.0.0.1:0", "--store", store.to_str().unwrap()])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let url = line.trim().strip_prefix("listening on ").unwrap().to_string();
        Relay { child, url }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A relay for one connection that answers a Subscribe with `entry` (its
/// highest seq said to be 1, then a malformed entry numbered 2^32 - 1 so
/// that the client is caught up whatever `entry` holds) and every Get with
/// `blob`, whatever its ID.
fn fake_relay(entry: Vec<u8>, blob: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        while let Ok(message) = ws.read() {
            let m = message.into_data();
            if m.len() < 5 {
                continue;
            }
            let reply = |kind: u8, rest: &[u8]| Message::binary([&[kind][..], &m[1..5], rest].concat());
            let sent = match m[0] {
                0x04 => {
                    let mut last = vec![0; 32];
                    last.extend([0xff; 4]);
                    last.extend([0; 4]);
                    ws.send(reply(0x84, &[1, 1, 0, 0, 0]))
                        .and_then(|_| ws.send(Message::binary([&[0x90][..], &entry].concat())))
                        .and_then(|_| ws.send(Message::binary([&[0x90][..], &last].concat())))
                }
                0x01 => ws.send(reply(0x81, &[&[1][..], &blob].concat())),
                _ => Ok(()),
            };
            if sent.is_err() {
                break;
            }
        }
    });
    url
}
