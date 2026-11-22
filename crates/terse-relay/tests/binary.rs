//! The `terse-relay` program: its options for a public relay, and a
//! graceful stop on SIGTERM.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use terse_net::protocol::{self, status};
use tungstenite::Message;

fn relay() -> Command {
    Command::new(env!("CARGO_BIN_EXE_terse-relay"))
}

#[test]
fn bad_options_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().to_str().unwrap();
    for args in [
        &["--feed-bytes", "lots"][..],
        &["--unreferenced-time", "5m"],
        &["--trust-proxy", "10.0.0.0/99"],
        &["--writers", "/nonexistent/writers"],
        &["--upload-bytes-per-hour", "10K"],
    ] {
        let out = relay().args(["--store", store, "--listen", "127.0.0.1:0"]).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(!out.stderr.is_empty());
    }
    let help = relay().arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("--public"));
}

#[cfg(unix)]
#[test]
fn public_relay_and_graceful_stop() {
    let dir = tempfile::tempdir().unwrap();
    let writers = dir.path().join("writers");
    std::fs::write(&writers, "# nobody yet\n").unwrap();
    let status_file = dir.path().join("status");
    let mut child = relay()
        .args(["--public", "--listen", "127.0.0.1:0", "--store"])
        .arg(dir.path().join("store"))
        .arg("--writers")
        .arg(&writers)
        .args(["--trust-proxy", "127.0.0.1", "--max-store-bytes", "1G", "--status-file"])
        .arg(&status_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.trim().strip_prefix("listening on ").unwrap().to_string();
    let stream = TcpStream::connect(url.strip_prefix("ws://").unwrap()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let (mut ws, _) = tungstenite::client::client(url.as_str(), stream).unwrap();
    let id = terse_net::Identity::from_seed([1; 32]);
    let entry = terse_net::Entry::article(id.feed_key(), 1, 1, b"x", None, "x").sign(&id).unwrap();
    ws.send(Message::binary(protocol::encode_publish(1, &entry.bytes))).unwrap();
    let reply = loop {
        if let Message::Binary(b) = ws.read().unwrap() {
            break b.to_vec();
        }
    };
    assert_eq!(reply, [protocol::PUBLISH_REPLY, 1, 0, 0, 0, status::NOT_WRITER]);
    // The status file appears within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&status_file).is_ok_and(|s| s.contains("not-writer 1")) {
        assert!(Instant::now() < deadline, "no status file, or it does not count the refusal");
        std::thread::sleep(Duration::from_millis(50));
    }
    // SIGTERM: the connection is closed with a close frame, the relay
    // exits with success.
    assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success());
    loop {
        match ws.read() {
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => break,
            Ok(_) => {}
            Err(e) => panic!("not closed cleanly: {e}"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        assert!(Instant::now() < deadline, "the relay did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}
