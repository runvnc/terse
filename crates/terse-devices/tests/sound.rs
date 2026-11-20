//! The sound device as a driver process (terse-dev-sound) and compiled in
//! (terse-core's `Sound`, as `terse run` and terse-host have it) are twins:
//! programs/tune gives the same samples, frame by frame, and the same
//! state hashes on both, over both transports, batched or not; a snapshot
//! taken in the middle of a note on one continues identically on the
//! other.

use std::path::{Path, PathBuf};

use terse_bus::bus::BusConfig;
use terse_bus::host::{self, BusSystem, Start};
use terse_core::sound::FRAME_SAMPLES;
use terse_core::{FrameInput, Image, Machine, Snapshot};

const TUNE: &[u8] = include_bytes!("../../../programs/tune/tune.terse");

fn bin_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_terse-dev-sound")).parent().unwrap().to_path_buf()
}

fn start(image: &Image, socket: bool, batching: bool, snapshot: Option<&Snapshot>) -> BusSystem {
    let slots = host::needed_slots(image, false);
    assert!(slots.contains(&terse_core::sound::SLOT));
    let drivers = host::drivers(&bin_dir(), &slots, &[], &[]).unwrap();
    let mut config = BusConfig::new(PathBuf::new());
    config.shm = !socket;
    config.batching = batching;
    Start { config, drivers, image, shell: None, identity: None, snapshot }.start().unwrap()
}

/// The in-process machine's next `frames` frames: samples and state hash
/// after each.
fn reference(m: &mut Machine, frames: u32) -> Vec<(Vec<i16>, [u8; 32])> {
    let mut out = Vec::new();
    for _ in 0..frames {
        if m.stopped().is_some() {
            break;
        }
        m.run_frame(&FrameInput::default());
        out.push((m.take_audio().unwrap(), m.state_hash()));
    }
    out
}

/// The same on the bus; the samples as the core mixes them (a single
/// machine: its own).
fn on_bus(bs: &mut BusSystem, frames: u32) -> Vec<(Vec<i16>, [u8; 32])> {
    let mut out = Vec::new();
    for _ in 0..frames {
        if bs.system.stopped().is_some() {
            break;
        }
        bs.frame(&[], Default::default()).unwrap();
        let mut frame = [0i16; 2 * FRAME_SAMPLES];
        assert!(bs.system.take_audio(&mut frame), "the driver gave the frame's samples");
        let mut again = [0i16; 2 * FRAME_SAMPLES];
        assert!(!bs.system.take_audio(&mut again), "once");
        out.push((frame.to_vec(), bs.system.shell_mut().try_state_hash().unwrap()));
    }
    out
}

#[test]
fn bus_and_in_process_give_the_same_samples() {
    let image = Image::parse(TUNE).unwrap();
    let want = reference(&mut Machine::new(&image), 2000);
    assert_eq!(want.len(), 1107, "the piece runs to its exit");
    for (socket, batching) in [(false, true), (true, true), (false, false)] {
        let mut bs = start(&image, socket, batching, None);
        let got = on_bus(&mut bs, 2000);
        assert_eq!(got.len(), want.len(), "socket {socket}, batching {batching}");
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert!(g.0 == w.0, "socket {socket}, batching {batching}: frame {i}'s samples differ");
            assert_eq!(g.1, w.1, "socket {socket}, batching {batching}: frame {i}'s state");
        }
        assert_eq!(bs.system.shell().exit_code(), Some(0));
    }
}

#[test]
fn snapshot_mid_note_crosses_between_bus_and_in_process() {
    let image = Image::parse(TUNE).unwrap();
    // In the pluck section and in the last, where everything sounds.
    for at in [903, 1000] {
        let mut a = Machine::new(&image);
        reference(&mut a, at);
        let snap = Snapshot::of(&a);
        // In-process → bus.
        let mut bs = start(&image, false, true, Some(&snap));
        assert_eq!(bs.system.shell_mut().try_state_hash().unwrap(), a.state_hash());
        let want = reference(&mut a, 60);
        assert_eq!(on_bus(&mut bs, 60), want, "restored on the bus at frame {at}");
        // Bus → in-process.
        let back = Snapshot::try_of(bs.system.shell_mut()).unwrap();
        let mut b = Snapshot::decode(&back.encode()).unwrap().restore(&image).unwrap();
        assert_eq!(reference(&mut b, 60), on_bus(&mut bs, 60), "restored in-process at frame {}", at + 60);
    }
}
