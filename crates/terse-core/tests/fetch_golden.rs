//! Golden test: programs/fetch/fetch.terse with a simulated host that knows
//! one blob, message.txt, and completes every request at the start of the next
//! frame (as `terse run` does). Set UPDATE_GOLDEN=1 to rewrite the expected file.

use terse_core::content::status;
use terse_core::{Completion, Event, FrameInput, Image, Machine, Outcome, Request, RequestKind};

const IMAGE: &[u8] = include_bytes!("../../../programs/fetch/fetch.terse");
const MESSAGE: &[u8] = include_bytes!("../../../programs/fetch/message.txt");
const EXPECTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../programs/fetch/expected-screen.txt");

/// What a host with `blobs` in its store answers.
fn serve(requests: Vec<Request>, blobs: &[&[u8]]) -> Vec<Completion> {
    requests
        .into_iter()
        .map(|Request { handle, kind }| {
            let outcome = match kind {
                RequestKind::Get { id, offset, length } => {
                    match blobs.iter().find(|b| blake3::hash(b).as_bytes() == &id) {
                        Some(blob) => {
                            let start = (offset as usize).min(blob.len());
                            let end = (start + length as usize).min(blob.len());
                            Outcome::Got { size: blob.len() as u32, data: blob[start..end].to_vec() }
                        }
                        None => Outcome::NotFound,
                    }
                }
                RequestKind::Put { data } => Outcome::Put { id: *blake3::hash(&data).as_bytes() },
                RequestKind::Subscriptions { .. } | RequestKind::Timeline { .. } => {
                    Outcome::Records { total: 0, data: Vec::new() }
                }
            };
            Completion { handle, outcome }
        })
        .collect()
}

fn run(frames: u32, blobs: &[&[u8]], escape_at: Option<u32>) -> Machine {
    let mut m = Machine::new(&Image::parse(IMAGE).unwrap());
    let mut completions = Vec::new();
    for f in 0..frames {
        let events: Vec<Event> = (escape_at == Some(f)).then(|| Event::key_down(0x29, 0)).into_iter().collect();
        let r = m.run_frame(&FrameInput { events: &events, completions: &completions, ..Default::default() });
        assert_eq!(r.completions_ignored, 0);
        if r.exit.is_some() {
            break;
        }
        completions = serve(m.take_content_requests(), blobs);
    }
    m
}

#[test]
fn fetch_screen() {
    let m = run(10, &[MESSAGE], None);
    let got = m.terminal().unwrap().text();
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(EXPECTED, &got).unwrap();
    }
    let want = std::fs::read_to_string(EXPECTED).expect("expected file (run with UPDATE_GOLDEN=1)");
    assert_eq!(got, want);
    assert!(got.contains(std::str::from_utf8(MESSAGE).unwrap().lines().nth(3).unwrap()));
    let put_id: Vec<u8> = (0..=255).collect();
    let hex: String = blake3::hash(&put_id).as_bytes()[..8].iter().map(|b| format!("{b:02x}")).collect();
    assert!(got.contains(&format!("blob ID: {hex}…")), "{got}");
}

#[test]
fn fetch_not_found() {
    let m = run(10, &[], None);
    let text = m.terminal().unwrap().text();
    assert!(text.contains("fetching… status 02 not found"), "{text}");
    assert!(text.contains("Putting 256 bytes built in RAM… status 01 done"), "{text}");
    // Request blocks: get at 8000h (linear 0000h), put at 8040h (linear 0040h).
    assert_eq!(m.ram()[1], status::NOT_FOUND);
    assert_eq!(m.ram()[0x41], status::DONE);
}

#[test]
fn fetch_exits_on_escape() {
    let m = run(20, &[MESSAGE], Some(6));
    assert_eq!(m.exit_code(), Some(0));
}
