//! `terse-host <link>` (README "Links on the command line"): a `terse:`
//! link names what to open. An article opens in the built-in reader inside
//! the built-in shell (the shell's argument, SPEC §system `argument`); a
//! program link runs the program; a feed link, a malformed link and one
//! that cannot be fetched say why, without a window. The windows run in
//! `--drive`'s own headless compositor (skipped where there is none).

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

/// The tests run one at a time: each runs a window in real time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_weston() -> bool {
    let ok = Command::new("weston").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ok {
        terse_testkit::skip!("weston", "no weston: --drive needs a headless compositor");
    }
    ok
}

/// `terse-host <args>` with the store, config and data in `dir`, no relay.
fn host(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_terse-host"))
        .args(args)
        .arg("--store")
        .arg(dir.join("store"))
        .arg("--config")
        .arg(dir.join("config"))
        .arg("--data")
        .arg(dir.join("data"))
        .args(["--consent", "deny", "--mute"])
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env_remove("TERSE_DRIVE_DISPLAY")
        .env_remove("TERSE_DRIVE_NESTED")
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!("status {:?}\n--- stdout\n{}--- stderr\n{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// Puts `data` into the store in `dir` with `terse put`; its ID.
fn put(dir: &Path, data: &[u8]) -> String {
    let file = dir.join("blob");
    std::fs::write(&file, data).unwrap();
    let o = Command::new(terse_testkit::program("terse"))
        .args(["put", file.to_str().unwrap(), "--relay", "none", "--store"])
        .arg(dir.join("store"))
        .arg("--config")
        .arg(dir.join("config"))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    terse_net::to_hex(&terse_net::blob_id(data))
}

/// A link to an article: the shell opens it in the reader, which has the
/// focus; the window is the shell's.
#[test]
fn an_article_link_opens_in_the_reader() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let id = put(dir.path(), b"# Given by a friend\n\nOpened from the command line.\n");
    let script = dir.path().join("script.txt");
    std::fs::write(&script, "settle max 600\nexpect page contains Opened from the command line.\nshot article.png\nquit\n").unwrap();
    let keep = std::env::var_os("TERSE_TEST_KEEP_SHOT");
    let link = format!("terse:{id}");
    let o = host(dir.path(), &[&link, "--drive", script.to_str().unwrap(), "--renderer", "software"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(dir.path().join("article.png").exists());
    // TERSE_TEST_KEEP_SHOT=<file>: the picture, to look at.
    if let Some(keep) = keep {
        std::fs::copy(dir.path().join("article.png"), keep).unwrap();
    }
}

/// A link to a built-in program runs it as `terse-host <image>` would.
#[test]
fn a_program_link_runs_the_program() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.txt");
    std::fs::write(&script, "settle max 600\ndump state\nquit\n").unwrap();
    let link = format!("terse:program/{}/hello", "0".repeat(64));
    let o = host(dir.path(), &[&link, "--drive", script.to_str().unwrap(), "--renderer", "software"]);
    assert!(o.status.success(), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("hello"), "the hello machine: {}", text(&o));
}

/// What cannot be opened says why, and no window is opened.
#[test]
fn links_that_cannot_be_opened() {
    let dir = tempfile::tempdir().unwrap();
    let key = &terse_net::to_hex(&terse_net::Identity::from_seed([9; 32]).feed_key());
    let shot = dir.path().join("shot.png");
    let shot = shot.to_str().unwrap();
    for (link, said) in [
        (format!("terse:feed/{key}@relay.example"), format!("a feed: subscribe with `terse feed subscribe {key}@relay.example`")),
        ("terse:1234".to_string(), "not a link".to_string()),
        (format!("terse:{}", "7".repeat(64)), "could not be fetched".to_string()),
        (format!("terse:program/{}/nothing", "0".repeat(64)), "no program nothing is built into this host".to_string()),
        (format!("terse:program/{key}/tool"), format!("subscribe to it (terse feed subscribe {key})")),
    ] {
        let o = host(dir.path(), &[&link, "--screenshot", shot, "--frames", "1"]);
        assert_eq!(o.status.code(), Some(2), "{link}: {}", text(&o));
        assert!(String::from_utf8_lossy(&o.stderr).contains(&said), "{link}: {}", text(&o));
        assert!(!Path::new(shot).exists());
    }
}

/// The reader's O (programs/reader): a link pasted as text events, as the
/// window delivers a paste (SPEC §events), opens the article.
#[test]
fn the_reader_opens_a_pasted_link() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !have_weston() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let id = put(dir.path(), b"# Pasted\n\nA link someone gave you.\n");
    let reader = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../programs/reader/reader.terse");
    let script = dir.path().join("script.txt");
    let steps = format!(
        "settle max 600\npress o\nwait 2\ntext  terse:{} {}\nwait 2\nexpect terminal contains Link: terse:{id}\npress enter\n\
         settle max 600\nexpect page contains A link someone gave you.\nshot pasted.png\nquit\n",
        &id[..32],
        &id[32..]
    );
    std::fs::write(&script, steps).unwrap();
    let o = host(dir.path(), &[reader.to_str().unwrap(), "--drive", script.to_str().unwrap(), "--renderer", "software"]);
    assert!(o.status.success(), "{}", text(&o));
    if let Some(keep) = std::env::var_os("TERSE_TEST_KEEP_SHOT") {
        std::fs::copy(dir.path().join("pasted.png"), keep).unwrap();
    }
}
