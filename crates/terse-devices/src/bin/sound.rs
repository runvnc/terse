//! `terse-dev-sound`: the sound device (SPEC §sound, slot 6). terse-core's
//! `Sound` for the state and the synthesis: the calls set its registers
//! and tables, and at END it renders the frame's 800 sample pairs. It
//! plays nothing itself: it publishes the frame's samples as a property
//! (`sound.frame`, DEVICES.md 7.2), which the core reads after each frame
//! and mixes with its other machines' (SPEC §sound.presentation).

use std::collections::HashMap;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, Mem, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, Open, SlotInfo, access, property, rule};
use terse_core::machine::{charge, copy_charge};
use terse_core::sound::{CAPS, MAX_LIST, MAX_SAMPLE, SAMPLE_SLOTS, SLOT, Sound, VOICES, WAVE_LEN, WAVES};
use terse_core::{ERR_BUFFER, ERR_UNKNOWN};

/// SPEC §sound.calls as a call table (DEVICES.md 5): every call deferred.
fn table() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        // submit: 1 to 4,096 bytes in ROM or RAM, else 1; 2 per byte.
        e(0).buffer(0, 1, 1, 1, MAX_LIST as u32, access::READ | access::ROM)
            .results(1, 1)
            .batch(rule::CONSTANT, 0, 0, 0, 0)
            .per_byte(charge::WRITE_BYTE as u16),
        // wave: 64 bytes at a1, slot a0 below 32.
        e(1).fixed(1, WAVE_LEN as u32, access::READ | access::ROM).check(0, WAVES as u32 - 1).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        // sample: a2 bytes (0 to 65,536) at a1, slot a0 below 4.
        e(2).buffer(1, 2, 1, 0, MAX_SAMPLE as u32, access::READ | access::ROM)
            .check(0, SAMPLE_SLOTS as u32 - 1)
            .batch(rule::CONSTANT, 0, 0, 0, 0)
            .per4(),
        // info: 16 voices, the capabilities in a1.
        e(3).sets_a1().batch(rule::CONSTANT, 0, VOICES as u32, CAPS, 0),
    ]
}

/// One machine's sound device and the frame it rendered last.
struct Inst {
    sound: Sound,
    /// The frame counter after the frame rendered last, and its samples
    /// (`sound.frame`); empty before the first.
    frame: Vec<u8>,
}

#[derive(Default)]
struct SoundDriver {
    inst: HashMap<u32, Inst>,
}

impl Device for SoundDriver {
    fn info(&mut self, _: &Session) -> DeviceInfo {
        DeviceInfo {
            name: "terse-dev-sound (Rust)".into(),
            native: (0, 0),
            slots: vec![SlotInfo { slot: SLOT, version: 0, layers: Vec::new(), table: table() }],
            publishes: vec![property::SOUND_FRAME.into()],
            subscribes: Vec::new(),
            services: Vec::new(),
            ..Default::default()
        }
    }

    fn open(&mut self, instance: u32, _: &Open, _: &Session) -> Result<(), String> {
        self.inst.insert(instance, Inst { sound: Sound::default(), frame: Vec::new() });
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.inst.remove(&instance);
    }

    fn reset(&mut self, instance: u32) {
        if let Some(i) = self.inst.get_mut(&instance) {
            *i = Inst { sound: Sound::default(), frame: Vec::new() };
        }
    }

    fn call(&mut self, instance: u32, c: &CallIn, _: &mut dyn Mem) -> Reply {
        let Some(i) = self.inst.get_mut(&instance) else { return Reply { a0: ERR_UNKNOWN, ..Default::default() } };
        let s = &mut i.sound;
        let [a0, _, a2, a3, ..] = c.args;
        match c.function {
            0 => Reply { a0: s.submit(c.buf), a1: None, charge: c.buf.len() as u32 * charge::WRITE_BYTE },
            1 if (a0 as usize) < WAVES && c.buf.len() == WAVE_LEN => {
                s.load_wave(a0 as usize, c.buf);
                Reply { a0: 0, a1: None, charge: copy_charge(WAVE_LEN) }
            }
            2 if (a0 as usize) < SAMPLE_SLOTS && c.buf.len() == a2 as usize => {
                s.load_sample(a0 as usize, c.buf, a3);
                Reply { a0: 0, a1: None, charge: copy_charge(c.buf.len()) }
            }
            1 | 2 => Reply { a0: ERR_BUFFER, a1: None, charge: 0 },
            3 => Reply { a0: VOICES as u32, a1: Some(CAPS), charge: 0 },
            _ => Reply { a0: ERR_UNKNOWN, a1: None, charge: 0 },
        }
    }

    fn begin(&mut self, _: u32, _: &Begin, _: &mut dyn Mem) -> Inputs {
        Inputs::default()
    }

    /// SPEC §sound.render: the frame's samples, rendered when it ends.
    fn end(&mut self, instance: u32, frame: u32, _: bool) {
        let Some(i) = self.inst.get_mut(&instance) else { return };
        i.sound.render_frame();
        let samples = i.sound.take_frame().unwrap_or_default();
        i.frame.clear();
        i.frame.extend_from_slice(&frame.to_le_bytes());
        for s in samples {
            i.frame.extend_from_slice(&s.to_le_bytes());
        }
    }

    fn state(&self, instance: u32, _: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(i) = self.inst.get(&instance) {
            i.sound.write_state(&mut out);
        }
        out
    }

    fn restore(&mut self, instance: u32, _: u8, bytes: &[u8]) -> Option<usize> {
        let i = self.inst.get_mut(&instance)?;
        let mut r = terse_core::snapshot::Reader::new(bytes);
        let mut s = Sound::default();
        s.restore(&mut r).ok()?;
        i.sound = s;
        i.frame.clear();
        Some(r.position())
    }

    fn get(&mut self, instance: u32, name: &str) -> Option<Vec<u8>> {
        let i = self.inst.get(&instance)?;
        (name == property::SOUND_FRAME).then(|| i.frame.clone())
    }
}

fn main() -> std::process::ExitCode {
    terse_bus::driver::run(&mut SoundDriver::default(), "terse-dev-sound")
}
