//! programs/tune/tune.terse, the sound device's demonstration (SPEC 18):
//! its samples frame by frame, hashed, are the golden ones
//! (programs/tune/expected-audio.txt: a conformance check of the
//! synthesis, which is specified bit for bit), with the state hashes
//! beside them; a snapshot taken in the middle of a note restores to the
//! same samples and hashes; `terse run --audio-out` writes those samples
//! as a WAV file; the reading profile refuses the image; and the image is
//! up to date with tune.c.

mod common;

use terse_core::sound::FRAME_SAMPLES;
use terse_core::{FrameInput, Image, Machine, Snapshot};
use terse_net::{blob_id, to_hex};

const IMAGE: &[u8] = include_bytes!("../../../programs/tune/tune.terse");
/// The frames the piece runs, its exit frame included.
const FRAMES: u32 = 1107;

fn image() -> Image {
    Image::parse(IMAGE).unwrap()
}

/// Runs `m` until it stops or `frames` frames have run; each frame's
/// samples, as bytes.
fn run(m: &mut Machine, frames: u32) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for _ in 0..frames {
        if m.stopped().is_some() {
            break;
        }
        m.run_frame(&FrameInput::default());
        assert!(m.fault().is_none(), "{:?}", m.fault());
        let s = m.take_audio().expect("a frame of samples");
        assert_eq!(s.len(), 2 * FRAME_SAMPLES);
        out.push(s.iter().flat_map(|x| x.to_le_bytes()).collect());
    }
    out
}

/// The whole piece: per 100 frames, the hash of their samples and the
/// state hash after them; then the hash of all the samples.
#[test]
fn samples_are_golden() {
    let mut m = Machine::new(&image());
    let mut all = Vec::new();
    let mut part = Vec::new();
    let mut text = String::new();
    for f in 0..FRAMES {
        let frame = run(&mut m, 1);
        assert_eq!(frame.len(), 1, "stopped early, at frame {f}");
        all.extend_from_slice(&frame[0]);
        part.extend_from_slice(&frame[0]);
        if m.frame() % 100 == 0 || m.stopped().is_some() {
            text += &format!("{} {} {}\n", m.frame(), to_hex(&blob_id(&part)), to_hex(&m.state_hash()));
            part.clear();
        }
    }
    assert_eq!(m.exit_code(), Some(0));
    text += &format!("all {}\n", to_hex(&blob_id(&all)));
    common::golden("tune/expected-audio.txt", &text);
}

/// STATE.md 1.12: the sound device's state is the machine's, so a machine
/// restored from a snapshot taken while notes sound (their envelopes,
/// phases, the noise and the sample voice mid-way) renders the same
/// samples as the one that went on.
#[test]
fn snapshot_mid_note_continues_identically() {
    let image = image();
    // Sections are 158 frames: 130 is in the plain melody, 400 in the
    // feedback (its last two outputs are state), 700 in the
    // noise, 903 in the pluck (a sample voice mid-way), 1000 in the last,
    // where all of them sound.
    for at in [130, 400, 700, 903, 1000] {
        let mut a = Machine::new(&image);
        run(&mut a, at);
        let snap = Snapshot::of(&a);
        let sound = a.sound().unwrap();
        assert!(sound.voices.iter().any(|v| v.env > 0), "frame {at}: something sounds");
        let mut b = Snapshot::decode(&snap.encode()).unwrap().restore(&image).unwrap();
        assert_eq!(b.state_hash(), a.state_hash());
        let (sa, sb) = (run(&mut a, 120), run(&mut b, 120));
        assert_eq!(sa, sb, "from frame {at}");
        assert_eq!(a.state_hash(), b.state_hash());
    }
}

/// `terse run --audio-out`: a WAV file of exactly those samples.
#[test]
fn audio_out_writes_the_samples() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tune.wav");
    let image_path = common::programs().join("tune/tune.terse");
    let out = common::terse_command()
        .args(["run", image_path.to_str().unwrap(), "--frames", "200", "--audio-out", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let wav = std::fs::read(&path).unwrap();
    let mut m = Machine::new(&image());
    let samples: Vec<u8> = run(&mut m, 200).concat();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48_000);
    assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize, samples.len());
    assert_eq!(&wav[44..], &samples[..]);
}

/// SPEC 2.1: sound is not in the reading profile; a reading host refuses
/// an image that lists it (and conform-reading lists only devices 2, 4
/// and 7: its own test runs it unchanged).
#[test]
fn reading_profile_refuses_sound() {
    let image_path = common::programs().join("tune/tune.terse");
    let out = common::terse_command().args(["run", image_path.to_str().unwrap(), "--profile", "reading", "--frames", "1"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("lists device 6"), "{}", String::from_utf8_lossy(&out.stderr));
    let reading = Image::parse(include_bytes!("../../../programs/conform-reading/conform-reading.terse")).unwrap();
    assert!(!reading.has_device(terse_core::sound::SLOT));
}

#[test]
fn image_is_up_to_date() {
    common::check_up_to_date(IMAGE, &["tune/tune.c"], true, "tune", 1, &[(2, 0), (6, 0)]);
}
