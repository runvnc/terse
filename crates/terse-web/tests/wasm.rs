//! The website's try page: the WebAssembly builds, the copy the site uses
//! is built from the current sources, `terse manual site` writes the page
//! and its files within the size budget, and (where a Chrome and Node are
//! found) the page runs in a headless browser.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    terse_testkit::workspace()
}

/// The hash scripts/web-sources.py prints.
fn sources_hash() -> String {
    use sha2::{Digest, Sha256};
    let root = root();
    let mut files = Vec::new();
    for krate in ["crates/terse-core", "crates/terse-web"] {
        files.push(format!("{krate}/Cargo.toml"));
        let mut dirs = vec![root.join(krate).join("src")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    files.sort();
    let mut h = Sha256::new();
    for f in files {
        h.update(f.as_bytes());
        h.update([0]);
        h.update(std::fs::read(root.join(&f)).unwrap());
        h.update([0]);
    }
    format!("sha256 {}\n", h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Whether rustc has the wasm32-unknown-unknown target.
fn has_wasm_target() -> bool {
    let Ok(out) = Command::new("rustc").args(["--print", "sysroot"]).output() else { return false };
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Path::new(&sysroot).join("lib/rustlib/wasm32-unknown-unknown").is_dir()
}

/// The module builds for the browser, as scripts/build-web builds it, and
/// has the functions try.js calls.
#[test]
fn wasm_builds() {
    if !has_wasm_target() {
        terse_testkit::skip!("wasm32", "the wasm32-unknown-unknown target is not installed (rustup target add wasm32-unknown-unknown)");
        return;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(root())
        .env("CARGO_PROFILE_RELEASE_LTO", "true")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
        .env("CARGO_PROFILE_RELEASE_PANIC", "abort")
        .args(["rustc", "-p", "terse-web", "--release", "--target", "wasm32-unknown-unknown", "--crate-type", "cdylib"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let wasm = std::fs::read(root().join("target/wasm32-unknown-unknown/release/terse_web.wasm")).unwrap();
    assert!(wasm.starts_with(b"\0asm"));
    assert!(wasm.len() < 1_500_000, "the module is {} bytes", wasm.len());
    for name in ["tw_alloc", "tw_start", "tw_key", "tw_text", "tw_paste", "tw_pointer", "tw_buttons", "tw_wheel", "tw_home", "tw_frame", "tw_draw", "tw_draw_len", "tw_take_copied", "memory"] {
        assert!(wasm.windows(name.len()).any(|w| w == name.as_bytes()), "exports {name}");
    }
}

/// The module the site uses (docs/manual/site-files/terse-web.wasm) was
/// built from the current terse-core and terse-web.
#[test]
fn committed_wasm_is_current() {
    let wasm = std::fs::read(root().join("docs/manual/site-files/terse-web.wasm")).unwrap();
    assert!(wasm.starts_with(b"\0asm"));
    let recorded = std::fs::read_to_string(root().join("crates/terse-web/wasm.sources")).unwrap_or_default();
    assert_eq!(
        recorded,
        sources_hash(),
        "docs/manual/site-files/terse-web.wasm is older than terse-core or terse-web: run scripts/build-web and commit it"
    );
}

/// `terse manual site` in a temporary directory.
fn site() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let terse = terse_testkit::program("terse");
    let out = Command::new(terse)
        .current_dir(root())
        .args(["manual", "site", "--dir", "docs/manual", "--out", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    tmp
}

/// The site has the page, its script, the module and the content, which
/// holds the shell, the programs and the manual; the page loads nothing
/// before Start, and what Start loads is under 2 MB compressed.
#[test]
fn site_has_the_try_page() {
    let tmp = site();
    let read = |n: &str| std::fs::read(tmp.path().join(n)).unwrap_or_else(|e| panic!("{n}: {e}"));
    let page = String::from_utf8(read("try.html")).unwrap();
    assert!(page.contains("curl -fsSL https://tersenet.org/install.sh | sh"));
    assert!(page.contains("https://github.com/runvnc/terse/releases/latest"));
    assert!(page.contains("<script src=\"try.js\" defer></script>") && page.contains("data-start=\"\""));
    assert!(!page.contains("terse-web.wasm") && !page.contains("try-content.bin"), "nothing heavy before Start");
    let js = String::from_utf8(read("try.js")).unwrap();
    assert!(js.contains("\"terse-web.wasm\"") && js.contains("\"try-content.bin\""));
    assert!(page.len() + js.len() < 40_000, "the page is small");
    let wasm = read("terse-web.wasm");
    assert!(wasm.starts_with(b"\0asm"));
    let content = read("try-content.bin");
    let bundle = miniz_oxide::inflate::decompress_to_vec_zlib(&content).unwrap();
    let entries = terse_web::bundle::decode(&bundle).unwrap();
    assert_eq!(entries.iter().filter(|e| e.0 == terse_web::bundle::SHELL).count(), 1);
    assert_eq!(entries.iter().filter(|e| e.0 == terse_web::bundle::PROGRAM).count(), 10);
    assert_eq!(entries.iter().filter(|e| e.0 == terse_web::bundle::MANUAL_INDEX).count(), 1);
    let wasm_gz = miniz_oxide::deflate::compress_to_vec(&wasm, 9).len();
    eprintln!("try page: terse-web.wasm {} bytes ({wasm_gz} compressed), try-content.bin {} bytes ({} uncompressed)", wasm.len(), content.len(), bundle.len());
    assert!(wasm_gz + content.len() < 2_000_000, "Start loads {} bytes", wasm_gz + content.len());
}

/// Serves `dir` on 127.0.0.1 (a free port) until the test ends.
fn serve(dir: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                }
                let path = line.split(' ').nth(1).unwrap_or("/").split(['?', '#']).next().unwrap_or("/").trim_start_matches('/');
                let path = if path.is_empty() { "index.html" } else { path };
                let body = (!path.contains("..")).then(|| std::fs::read(dir.join(path)).ok()).flatten();
                let kind = match path.rsplit('.').next() {
                    Some("html") => "text/html; charset=utf-8",
                    Some("js") => "text/javascript",
                    Some("css") => "text/css",
                    Some("wasm") => "application/wasm",
                    _ => "application/octet-stream",
                };
                let _ = match body {
                    Some(b) => {
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                        stream.write_all(&b)
                    }
                    None => write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                };
            });
        }
    });
    port
}

fn find_chrome() -> Option<PathBuf> {
    if let Some(c) = std::env::var_os("TERSE_CHROME") {
        return Some(c.into());
    }
    let path = std::env::var_os("PATH")?;
    for name in ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser", "chrome", "headless_shell"] {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// In a headless Chrome (never a window), Start runs the shell, Up and Enter
/// open the manual, the page's Home button goes back to the launcher and a program starts; the page
/// logs no error. Screenshots go to target/try-browser/ to look at.
#[test]
fn runs_in_a_headless_browser() {
    let Some(chrome) = find_chrome() else {
        terse_testkit::skip!("chrome", "no Chrome or Chromium found (TERSE_CHROME names one)");
        return;
    };
    let node_ok = Command::new("node").arg("-e").arg("process.exit(typeof WebSocket === 'function' ? 0 : 1)").status().is_ok_and(|s| s.success());
    if !node_ok {
        terse_testkit::skip!("node", "no Node 22 or later (with WebSocket) to drive the browser");
        return;
    }
    let tmp = site();
    let port = serve(tmp.path().to_path_buf());
    let shots = root().join("target/try-browser");
    std::fs::create_dir_all(&shots).unwrap();
    let steps = "start wait 60 shot 1-shell.png press ArrowUp press Enter wait 90 shot 2-manual.png \
                 press Tab press Enter wait 60 press PageDown wait 30 shot 3-article.png \
                 home wait 10 shot 4-launcher.png press ArrowUp press Enter wait 120 shot 5-two-machines.png \
                 home wait 5 press KeyL wait 30 shot 8-layout1.png press KeyL wait 30 shot 9-layout2.png press KeyL wait 40 shot 10-layout3.png";
    let out = Command::new("node")
        .arg(root().join("scripts/try-browser.mjs"))
        .arg(&chrome)
        .arg(format!("http://127.0.0.1:{port}/try.html"))
        .arg(&shots)
        .args(steps.split(' ').filter(|s| !s.is_empty()))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    eprintln!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "the page failed:\n{text}");
    assert!(text.contains("frames ") && !text.contains("frames 0 "), "{text}");
    // A program alone, started by the page's address (try.html#sparks).
    let out = Command::new("node")
        .arg(root().join("scripts/try-browser.mjs"))
        .arg(&chrome)
        .arg(format!("http://127.0.0.1:{port}/try.html#sparks"))
        .arg(&shots)
        .args("wait 120 shot 6-sparks.png press Enter wait 90 shot 7-sparks-playing.png".split(' '))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    eprintln!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "sparks alone failed:\n{text}");
    for s in ["1-shell.png", "2-manual.png", "4-launcher.png", "5-two-machines.png", "7-sparks-playing.png"] {
        assert!(shots.join(s).metadata().is_ok_and(|m| m.len() > 1000), "{s}");
    }
}
