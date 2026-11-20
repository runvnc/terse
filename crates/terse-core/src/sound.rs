//! Device 6, Sound (SPEC §sound): a small wavetable synthesizer with 16
//! voices, specified bit for bit. Pure: it takes the program's command
//! lists and tables and renders 800 stereo sample pairs at the end of
//! each frame; a host plays them, writes them to a file or drops them.
//! Everything here is integer arithmetic, so every implementation renders
//! the same samples.

use crate::snapshot::{Reader, RestoreError, bad};

/// The device's slot (SPEC §image).
pub const SLOT: u8 = 6;
/// Output sample rate, in sample pairs per second.
pub const RATE: u32 = 48_000;
/// Sample pairs rendered per frame (48,000 / 60).
pub const FRAME_SAMPLES: usize = 800;
/// Voices.
pub const VOICES: usize = 16;
/// Wavetable slots, and samples in each.
pub const WAVES: usize = 32;
pub const WAVE_LEN: usize = 64;
/// Sample slots, and the longest sample.
pub const SAMPLE_SLOTS: usize = 4;
pub const MAX_SAMPLE: usize = 65_536;
/// The longest command list one `submit` takes.
pub const MAX_LIST: usize = 4_096;
/// The envelope's greatest level.
pub const ENV_MAX: u32 = 0xFF_FFFF;
/// What the feedback F (the sum of a voice's last two outputs times its
/// feedback depth) is multiplied by in the table index (SPEC §sound.sources): the
/// average × 256, so depth 255 at full output moves the index by about
/// half a pass (π), the strongest feedback that stays a tone.
pub const FEEDBACK_SCALE: u32 = 128;
/// "No voice" in a Modulator or Ring command.
pub const NONE: u8 = 0xFF;
/// Capabilities `info` reports: bit 0, sample playback.
pub const CAPS: u32 = 1;
/// Results of `submit`.
pub const RESULT_DONE: u32 = 0;
pub const RESULT_REJECTED: u32 = 1;
/// Bytes of one voice in the state (STATE.md 1.12).
pub const VOICE_BYTES: usize = 38;
/// The longest section of the state (STATE.md 1.12).
pub const MAX_STATE: usize = VOICES * VOICE_BYTES + WAVES * WAVE_LEN + SAMPLE_SLOTS * (8 + MAX_SAMPLE);

/// Command-list opcodes (SPEC §sound.list).
pub mod op {
    pub const END: u8 = 0x00;
    pub const VOICE: u8 = 0x01;
    pub const WAVE: u8 = 0x02;
    pub const NOISE: u8 = 0x03;
    pub const SAMPLE: u8 = 0x04;
    pub const FREQUENCY: u8 = 0x05;
    pub const VOLUME: u8 = 0x06;
    pub const PAN: u8 = 0x07;
    pub const ENVELOPE: u8 = 0x08;
    pub const KEY_ON: u8 = 0x09;
    pub const KEY_OFF: u8 = 0x0A;
    pub const MODULATOR: u8 = 0x0B;
    pub const FEEDBACK: u8 = 0x0C;
    pub const RING: u8 = 0x0D;
    pub const MIX: u8 = 0x0E;
}

/// A voice's source.
pub mod source {
    pub const WAVE: u8 = 0;
    pub const NOISE: u8 = 1;
    pub const SAMPLE: u8 = 2;
}

/// Envelope stages.
pub mod stage {
    pub const OFF: u8 = 0;
    pub const ATTACK: u8 = 1;
    pub const DECAY: u8 = 2;
    pub const SUSTAIN: u8 = 3;
    pub const RELEASE: u8 = 4;
}

/// The phase increment for `hz` on a wavetable or noise voice:
/// round(hz × 2^32 / 48,000). (A convenience; the device takes increments.)
pub fn increment(hz: f64) -> u32 {
    (hz * 4_294_967_296.0 / RATE as f64).round().clamp(0.0, u32::MAX as f64) as u32
}

/// A WAV file (RIFF, PCM, 48,000 Hz, two channels, 16 bits) of `samples`,
/// left and right interleaved: what `--audio-out` writes.
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// One voice (SPEC §sound.voices).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    pub source: u8,
    pub wave: u8,
    pub sample: u8,
    pub phase: u32,
    pub incr: u32,
    pub volume: u8,
    pub pan: [u8; 2],
    pub attack: u16,
    pub decay: u16,
    pub sustain: u8,
    pub release: u16,
    pub stage: u8,
    pub env: u32,
    pub modulator: u8,
    pub depth: u8,
    pub feedback: u8,
    pub ring: u8,
    pub mix: bool,
    pub lfsr: u16,
    pub ended: bool,
    /// The voice's output in the last sample rendered.
    pub out: i16,
    /// Its output in the sample before that (feedback averages the two).
    pub prev: i16,
}

impl Default for Voice {
    /// A voice after reset.
    fn default() -> Self {
        Voice {
            source: source::WAVE,
            wave: 0,
            sample: 0,
            phase: 0,
            incr: 0,
            volume: 255,
            pan: [255, 255],
            attack: 0,
            decay: 0,
            sustain: 255,
            release: 0,
            stage: stage::OFF,
            env: 0,
            modulator: NONE,
            depth: 0,
            feedback: 0,
            ring: NONE,
            mix: true,
            lfsr: 1,
            ended: false,
            out: 0,
            prev: 0,
        }
    }
}

impl Voice {
    /// The sustain level as an envelope level.
    fn sustain_level(&self) -> u32 {
        self.sustain as u32 * 0x01_0101
    }

    /// One envelope step (SPEC §sound.render).
    fn step_envelope(&mut self) {
        match self.stage {
            stage::ATTACK => {
                let a = self.attack as u32;
                if a == 0 || self.env + a >= ENV_MAX {
                    self.env = ENV_MAX;
                    self.stage = stage::DECAY;
                } else {
                    self.env += a;
                }
            }
            stage::DECAY => {
                let (d, t) = (self.decay as u32, self.sustain_level());
                if d == 0 || self.env <= t + d {
                    self.env = t;
                    self.stage = stage::SUSTAIN;
                } else {
                    self.env -= d;
                }
            }
            stage::SUSTAIN => self.env = self.sustain_level(),
            stage::RELEASE => {
                let r = self.release as u32;
                if r == 0 || self.env <= r {
                    self.env = 0;
                    self.stage = stage::OFF;
                } else {
                    self.env -= r;
                }
            }
            _ => self.env = 0,
        }
    }

    fn key_on(&mut self) {
        self.stage = stage::ATTACK;
        self.phase = 0;
        self.ended = false;
    }

    fn key_off(&mut self) {
        if matches!(self.stage, stage::ATTACK | stage::DECAY | stage::SUSTAIN) {
            self.stage = stage::RELEASE;
        }
    }
}

/// A sample slot: the bytes and where a loop starts (one at or past the
/// end: the sample plays once).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleSlot {
    pub data: Vec<u8>,
    pub loop_start: u32,
}

impl SampleSlot {
    /// A position (16.16, in samples) brought into the sample: as it is,
    /// wrapped into the loop, or `None` past the end of a sample that
    /// plays once (SPEC §sound.sources).
    fn normalize(&self, p: u64) -> Option<u32> {
        let len = self.data.len() as u64;
        if p < len << 16 {
            return Some(p as u32);
        }
        let ls = self.loop_start as u64;
        if ls >= len {
            return None;
        }
        let (start, span) = (ls << 16, (len - ls) << 16);
        Some((start + (p - start) % span) as u32)
    }
}

/// The device's state (SPEC §sound, STATE.md 1.12) and the last frame's
/// samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sound {
    pub voices: [Voice; VOICES],
    pub waves: [[i8; WAVE_LEN]; WAVES],
    pub samples: [SampleSlot; SAMPLE_SLOTS],
    /// The last frame's samples, left and right interleaved (not state).
    out: Vec<i16>,
    /// `out` holds a frame not taken yet (not state).
    fresh: bool,
}

impl Default for Sound {
    /// The device after reset: every voice as [`Voice::default`], every
    /// table zero, every sample slot empty.
    fn default() -> Self {
        Sound {
            voices: std::array::from_fn(|_| Voice::default()),
            waves: [[0; WAVE_LEN]; WAVES],
            samples: std::array::from_fn(|_| SampleSlot::default()),
            out: vec![0; FRAME_SAMPLES * 2],
            fresh: false,
        }
    }
}

impl Sound {
    /// Carries out a command list (SPEC §sound.list) at once: the commands set
    /// the voices' registers, which the next frame's samples use. The list
    /// ends at End, at its last byte, at an unknown opcode, at a command
    /// whose operands are cut off or at an operand out of range; the
    /// commands before that have taken effect. Returns [`RESULT_REJECTED`]
    /// for a list of 0 or more than 4,096 bytes (nothing done).
    pub fn submit(&mut self, list: &[u8]) -> u32 {
        if list.is_empty() || list.len() > MAX_LIST {
            return RESULT_REJECTED;
        }
        let mut v = 0usize;
        let mut at = 0usize;
        let voice_or_none = |b: u8| (b as usize) < VOICES || b == NONE;
        while at < list.len() {
            let code = list[at];
            let n = match code {
                op::END => return RESULT_DONE,
                op::NOISE | op::KEY_ON | op::KEY_OFF => 0,
                op::VOICE | op::WAVE | op::SAMPLE | op::VOLUME | op::FEEDBACK | op::RING | op::MIX => 1,
                op::PAN | op::MODULATOR => 2,
                op::FREQUENCY => 4,
                op::ENVELOPE => 7,
                _ => return RESULT_DONE,
            };
            let Some(a) = list.get(at + 1..at + 1 + n) else { return RESULT_DONE };
            at += 1 + n;
            let u16_at = |i: usize| u16::from_le_bytes([a[i], a[i + 1]]);
            let voice = &mut self.voices[v];
            match code {
                op::VOICE if (a[0] as usize) < VOICES => v = a[0] as usize,
                op::WAVE if (a[0] as usize) < WAVES => {
                    voice.source = source::WAVE;
                    voice.wave = a[0];
                }
                op::NOISE => voice.source = source::NOISE,
                op::SAMPLE if (a[0] as usize) < SAMPLE_SLOTS => {
                    voice.source = source::SAMPLE;
                    voice.sample = a[0];
                }
                op::FREQUENCY => voice.incr = u32::from_le_bytes([a[0], a[1], a[2], a[3]]),
                op::VOLUME => voice.volume = a[0],
                op::PAN => voice.pan = [a[0], a[1]],
                op::ENVELOPE => {
                    voice.attack = u16_at(0);
                    voice.decay = u16_at(2);
                    voice.sustain = a[4];
                    voice.release = u16_at(5);
                }
                op::KEY_ON => voice.key_on(),
                op::KEY_OFF => voice.key_off(),
                op::MODULATOR if voice_or_none(a[0]) => {
                    voice.modulator = a[0];
                    voice.depth = a[1];
                }
                op::FEEDBACK => voice.feedback = a[0],
                op::RING if voice_or_none(a[0]) => voice.ring = a[0],
                op::MIX if a[0] <= 1 => voice.mix = a[0] == 1,
                // An operand out of range ends the list.
                _ => return RESULT_DONE,
            }
        }
        RESULT_DONE
    }

    /// Loads wavetable slot `slot` (below 32) from 64 bytes.
    pub fn load_wave(&mut self, slot: usize, bytes: &[u8]) {
        for (w, &b) in self.waves[slot].iter_mut().zip(bytes) {
            *w = b as i8;
        }
    }

    /// Loads sample slot `slot` (below 4) with `bytes` (at most 65,536)
    /// and a loop start.
    pub fn load_sample(&mut self, slot: usize, bytes: &[u8], loop_start: u32) {
        let s = &mut self.samples[slot];
        s.data.clear();
        s.data.extend_from_slice(bytes);
        s.loop_start = loop_start;
    }

    /// Voice `v`'s output in this sample (SPEC §sound.render): `cur` holds the
    /// outputs of voices 0 to v − 1 in this sample; a voice at or above v
    /// is read as it was in the last sample.
    fn voice_sample(&mut self, v: usize, cur: &[i16; VOICES]) -> i16 {
        let other = |voices: &[Voice; VOICES], q: u8| -> i32 {
            let q = q as usize;
            if q < v { cur[q] as i32 } else { voices[q].out as i32 }
        };
        let modulation = if self.voices[v].modulator != NONE {
            (other(&self.voices, self.voices[v].modulator) * self.voices[v].depth as i32) as u32
        } else {
            0
        };
        let ring = (self.voices[v].ring != NONE).then(|| other(&self.voices, self.voices[v].ring));
        let voice = &mut self.voices[v];
        voice.step_envelope();
        let raw: i32 = match voice.source {
            source::WAVE => {
                // The sum of the last two outputs (twice their average), so
                // × 128 is the average × 256: at most about half a turn.
                let feedback = ((voice.out as i32 + voice.prev as i32) * voice.feedback as i32) as u32;
                let p = voice.phase.wrapping_add(modulation.wrapping_mul(1024)).wrapping_add(feedback.wrapping_mul(FEEDBACK_SCALE));
                let r = self.waves[voice.wave as usize][(p >> 26) as usize] as i32;
                voice.phase = voice.phase.wrapping_add(voice.incr);
                r
            }
            source::NOISE => {
                let r = if voice.lfsr & 1 != 0 { 127 } else { -127 };
                let (p, carry) = voice.phase.overflowing_add(voice.incr);
                voice.phase = p;
                if carry {
                    let bit = (voice.lfsr ^ (voice.lfsr >> 1)) & 1;
                    voice.lfsr = (voice.lfsr >> 1) | (bit << 14);
                }
                r
            }
            _ => {
                let slot = &self.samples[voice.sample as usize];
                if voice.ended {
                    0
                } else {
                    match slot.normalize(voice.phase as u64) {
                        None => {
                            voice.ended = true;
                            voice.phase = 0;
                            0
                        }
                        Some(p) => {
                            let r = slot.data[(p >> 16) as usize] as i8 as i32;
                            match slot.normalize(p as u64 + voice.incr as u64) {
                                Some(q) => voice.phase = q,
                                None => {
                                    voice.ended = true;
                                    voice.phase = 0;
                                }
                            }
                            r
                        }
                    }
                }
            }
        };
        let mut o = (raw * (voice.env >> 8) as i32 * voice.volume as i32) >> 16;
        if let Some(q) = ring {
            o = (o * q) >> 15;
        }
        o as i16
    }

    /// Renders the frame's 800 sample pairs (SPEC §sound.render): the end of a
    /// frame. [`Sound::take_frame`] gives them.
    pub fn render_frame(&mut self) {
        let mut cur = [0i16; VOICES];
        for n in 0..FRAME_SAMPLES {
            for v in 0..VOICES {
                cur[v] = self.voice_sample(v, &cur);
            }
            let (mut l, mut r) = (0i32, 0i32);
            for (voice, &o) in self.voices.iter_mut().zip(&cur) {
                if voice.mix {
                    l += o as i32 * voice.pan[0] as i32;
                    r += o as i32 * voice.pan[1] as i32;
                }
                voice.prev = voice.out;
                voice.out = o;
            }
            self.out[2 * n] = (l >> 10).clamp(-32768, 32767) as i16;
            self.out[2 * n + 1] = (r >> 10).clamp(-32768, 32767) as i16;
        }
        self.fresh = true;
    }

    /// The samples of the frame rendered last, left and right interleaved,
    /// once: `None` if no frame was rendered since the last call.
    pub fn take_frame(&mut self) -> Option<&[i16]> {
        std::mem::take(&mut self.fresh).then_some(&self.out[..])
    }

    /// STATE.md 1.12.
    pub fn write_state(&self, out: &mut Vec<u8>) {
        for v in &self.voices {
            out.extend_from_slice(&[v.source, v.wave, v.sample]);
            out.extend_from_slice(&v.phase.to_le_bytes());
            out.extend_from_slice(&v.incr.to_le_bytes());
            out.extend_from_slice(&[v.volume, v.pan[0], v.pan[1]]);
            out.extend_from_slice(&v.attack.to_le_bytes());
            out.extend_from_slice(&v.decay.to_le_bytes());
            out.push(v.sustain);
            out.extend_from_slice(&v.release.to_le_bytes());
            out.push(v.stage);
            out.extend_from_slice(&v.env.to_le_bytes());
            out.extend_from_slice(&[v.modulator, v.depth, v.feedback, v.ring, v.mix as u8]);
            out.extend_from_slice(&v.lfsr.to_le_bytes());
            out.push(v.ended as u8);
            out.extend_from_slice(&v.out.to_le_bytes());
            out.extend_from_slice(&v.prev.to_le_bytes());
        }
        for w in &self.waves {
            out.extend(w.iter().map(|&b| b as u8));
        }
        for s in &self.samples {
            out.extend_from_slice(&(s.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&s.loop_start.to_le_bytes());
            out.extend_from_slice(&s.data);
        }
    }

    /// Reads the section STATE.md 1.12 lays out, refusing a field out of
    /// range.
    pub fn restore(&mut self, r: &mut Reader) -> Result<(), RestoreError> {
        let mut s = Sound::default();
        let voice_or_none = |b: u8| (b as usize) < VOICES || b == NONE;
        for v in &mut s.voices {
            let [source, wave, sample] = r.array()?;
            if source > source::SAMPLE || wave as usize >= WAVES || sample as usize >= SAMPLE_SLOTS {
                return Err(bad("a sound voice's source"));
            }
            *v = Voice {
                source,
                wave,
                sample,
                phase: r.u32()?,
                incr: r.u32()?,
                volume: r.u8()?,
                pan: [r.u8()?, r.u8()?],
                attack: r.u16()?,
                decay: r.u16()?,
                sustain: r.u8()?,
                release: r.u16()?,
                stage: r.u8()?,
                env: r.u32()?,
                modulator: r.u8()?,
                depth: r.u8()?,
                feedback: r.u8()?,
                ring: r.u8()?,
                mix: r.flag()?,
                lfsr: r.u16()?,
                ended: r.flag()?,
                out: r.u16()? as i16,
                prev: r.u16()? as i16,
            };
            if v.stage > stage::RELEASE || v.env > ENV_MAX || !voice_or_none(v.modulator) || !voice_or_none(v.ring) || !(1..=0x7FFF).contains(&v.lfsr) {
                return Err(bad("a sound voice's field out of range"));
            }
        }
        for w in &mut s.waves {
            for (x, &b) in w.iter_mut().zip(r.take(WAVE_LEN)?) {
                *x = b as i8;
            }
        }
        for slot in &mut s.samples {
            let len = r.u32()? as usize;
            if len > MAX_SAMPLE {
                return Err(bad("a sample longer than 65,536 bytes"));
            }
            slot.loop_start = r.u32()?;
            slot.data = r.take(len)?.to_vec();
        }
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// round(127 sin(2π i / 64)), programs/tune's table.
    fn sine() -> [u8; 64] {
        std::array::from_fn(|i| (127.0 * (2.0 * std::f64::consts::PI * i as f64 / 64.0).sin()).round() as i8 as u8)
    }

    /// programs/tune's feedback depth (its brass, section 3).
    const TUNE_FEEDBACK: u8 = 192;

    /// Voice 0's outputs over `n` samples, rendered as `render_frame` does.
    fn render(s: &mut Sound, n: usize) -> Vec<i16> {
        (0..n)
            .map(|_| {
                let o = s.voice_sample(0, &[0; VOICES]);
                s.voices[0].prev = s.voices[0].out;
                s.voices[0].out = o;
                o
            })
            .collect()
    }

    fn square() -> [u8; 64] {
        std::array::from_fn(|i| if i < 32 { 100 } else { (-100i8) as u8 })
    }

    #[test]
    fn envelope_steps_exactly() {
        let mut s = Sound::default();
        s.load_wave(0, &[127; 64]);
        // Attack 0x8000 a sample, decay 0x1000 to sustain 128, release 0x4000.
        s.submit(&[op::ENVELOPE, 0x00, 0x80, 0x00, 0x10, 128, 0x00, 0x40, op::KEY_ON, op::END]);
        let mut levels = Vec::new();
        for _ in 0..4000 {
            s.voices[0].step_envelope();
            levels.push((s.voices[0].stage, s.voices[0].env));
        }
        // 0xFFFFFF / 0x8000 = 511.99…: 511 steps up, the 512th reaches the top.
        assert_eq!(levels[510], (stage::ATTACK, 511 * 0x8000));
        assert_eq!(levels[511], (stage::DECAY, ENV_MAX));
        let t = 128 * 0x01_0101;
        let decay_steps = (ENV_MAX - t).div_ceil(0x1000) as usize;
        assert_eq!(levels[511 + decay_steps], (stage::SUSTAIN, t));
        assert_eq!(levels[511 + decay_steps - 1].0, stage::DECAY);
        s.submit(&[op::KEY_OFF]);
        s.voices[0].step_envelope();
        assert_eq!((s.voices[0].stage, s.voices[0].env), (stage::RELEASE, t - 0x4000));
        for _ in 0..1000 {
            s.voices[0].step_envelope();
        }
        assert_eq!((s.voices[0].stage, s.voices[0].env), (stage::OFF, 0));
    }

    #[test]
    fn instant_gate_and_volume() {
        let mut s = Sound::default();
        s.load_wave(0, &[127; 64]);
        // The reset envelope is a gate: full at once, silent at once.
        s.submit(&[op::KEY_ON]);
        s.render_frame();
        let f = s.take_frame().unwrap().to_vec();
        // 127 × 65,535 × 255 >> 16 = 32,384; × 255 >> 10 = 8,064.
        assert_eq!(s.voices[0].out, 32384);
        assert!(f.iter().all(|&x| x == 8064), "{:?}", &f[..4]);
        assert!(s.take_frame().is_none(), "taken once");
        s.submit(&[op::KEY_OFF, op::PAN, 255, 0]);
        s.render_frame();
        assert!(s.take_frame().unwrap().iter().all(|&x| x == 0));
    }

    #[test]
    fn mix_saturates() {
        let mut s = Sound::default();
        s.load_wave(0, &[0x80; 64]);
        let mut list = Vec::new();
        for v in 0..16 {
            list.extend([op::VOICE, v, op::KEY_ON]);
        }
        s.submit(&list);
        s.render_frame();
        // -128 × 65,535 × 255 >> 16 = -32,640 (floor); 16 voices sum far below -32,768.
        assert_eq!(s.voices[5].out, -32640);
        assert!(s.take_frame().unwrap().iter().all(|&x| x == -32768));
        // Four voices: -32,640 × 255 × 4 >> 10 = -32,513 (floor).
        let mut s2 = Sound::default();
        s2.load_wave(0, &[0x80; 64]);
        s2.submit(&[op::KEY_ON, op::VOICE, 1, op::KEY_ON, op::VOICE, 2, op::KEY_ON, op::VOICE, 3, op::KEY_ON]);
        s2.render_frame();
        assert_eq!(s2.take_frame().unwrap()[0], ((-32640i32 * 255 * 4) >> 10) as i16);
    }

    #[test]
    fn modulation_reads_lower_voices_now_and_higher_ones_late() {
        // Voice 0 modulates voice 1 (this sample) and voice 3 modulates
        // voice 2 (last sample). A modulator at a constant output shifts
        // the carrier's table index by out × depth × 1,024 / 2^26.
        let mut s = Sound::default();
        s.load_wave(0, &[127; 64]);
        let ramp: [u8; 64] = std::array::from_fn(|i| i as u8);
        s.load_wave(1, &ramp);
        let list = [
            op::KEY_ON, op::MIX, 0, // voice 0: constant 32,384, muted
            op::VOICE, 1, op::WAVE, 1, op::MODULATOR, 0, 1, op::KEY_ON,
            op::VOICE, 3, op::KEY_ON, op::MIX, 0,
            op::VOICE, 2, op::WAVE, 1, op::MODULATOR, 3, 1, op::KEY_ON,
        ];
        assert_eq!(s.submit(&list), RESULT_DONE);
        let mut cur = [0i16; VOICES];
        for v in 0..4 {
            cur[v] = s.voice_sample(v, &cur);
        }
        // 32,384 × 1 × 1,024 = 33,162,240: index 33,162,240 >> 26 = 0. Use
        // the ramp's value through the output: index 0 → raw 0.
        assert_eq!(cur[1], 0);
        // Depth 255 shifts by 32,384 × 255 × 1,024 >> 26 = 126 → index 62.
        let mut s = Sound::default();
        s.load_wave(0, &[127; 64]);
        s.load_wave(1, &ramp);
        let list = [
            op::KEY_ON, op::MIX, 0,
            op::VOICE, 1, op::WAVE, 1, op::MODULATOR, 0, 255, op::KEY_ON,
            op::VOICE, 3, op::KEY_ON, op::MIX, 0,
            op::VOICE, 2, op::WAVE, 1, op::MODULATOR, 3, 255, op::KEY_ON,
        ];
        s.submit(&list);
        let idx = ((32384u32 * 255).wrapping_mul(1024) >> 26) as i32;
        let expect = |raw: i32| ((raw * 65535 * 255) >> 16) as i16;
        let mut cur = [0i16; VOICES];
        for v in 0..4 {
            cur[v] = s.voice_sample(v, &cur);
        }
        for v in 0..4 {
            s.voices[v].out = cur[v];
        }
        assert_eq!(cur[1], expect(idx), "voice 1 sees voice 0 of this sample");
        assert_eq!(cur[2], 0, "voice 2 sees voice 3 of the last sample: 0 at first");
        let mut cur = [0i16; VOICES];
        for v in 0..4 {
            cur[v] = s.voice_sample(v, &cur);
        }
        // The carriers' own phase is 0 (increment 0), so only the offset counts.
        assert_eq!(cur[2], expect(idx), "a sample later");
    }

    #[test]
    fn feedback_and_ring() {
        let mut s = Sound::default();
        let ramp: [u8; 64] = std::array::from_fn(|i| (i as i8 - 32) as u8);
        s.load_wave(0, &ramp);
        s.load_wave(1, &square());
        s.submit(&[op::FEEDBACK, 200, op::KEY_ON, op::VOICE, 1, op::WAVE, 1, op::RING, 0, op::KEY_ON]);
        let mut cur = [0i16; VOICES];
        let mut outs = Vec::new();
        for _ in 0..4 {
            for v in 0..2 {
                cur[v] = s.voice_sample(v, &cur);
            }
            for v in 0..2 {
                s.voices[v].prev = s.voices[v].out;
                s.voices[v].out = cur[v];
            }
            outs.push(cur);
        }
        let o = |raw: i32| (raw * 65535 * 255) >> 16;
        // Sample 0: no feedback yet; ramp[0] = -32.
        assert_eq!(outs[0][0] as i32, o(-32));
        // The index moves by (the sum of the last two outputs × 200) × 128.
        let fb_idx = |a: i16, b: i16| ((((a as i32 + b as i32) * 200) as u32).wrapping_mul(FEEDBACK_SCALE) >> 26) as i32;
        assert_eq!(outs[1][0] as i32, o(fb_idx(outs[0][0], 0) - 32));
        assert_eq!(outs[2][0] as i32, o(fb_idx(outs[1][0], outs[0][0]) - 32));
        assert_eq!(outs[3][0] as i32, o(fb_idx(outs[2][0], outs[1][0]) - 32));
        // Ring: (square × voice 0 of this sample) >> 15.
        for k in 0..4 {
            assert_eq!(outs[k][1] as i32, (o(100) * outs[k][0] as i32) >> 15);
        }
    }

    /// A sine (programs/tune's table) at full volume with feedback 255:
    /// these samples pin SPEC §sound.sources's feedback, the sum of the voice's last
    /// two outputs × depth × 128 (the average × 256). An independent model
    /// of SPEC §sound, written in Python, gave them.
    #[test]
    fn feedback_samples_exactly() {
        let mut s = Sound::default();
        s.load_wave(0, &sine());
        // round(2^32 / 160.25): a period of 160.25 samples.
        let mut list = vec![op::FEEDBACK, 255, op::FREQUENCY];
        list.extend(26_801_668u32.to_le_bytes());
        list.push(op::KEY_ON);
        s.submit(&list);
        let outs = render(&mut s, 1000);
        assert_eq!(outs[..12], [0, 0, 0, 3059, 9434, 22949, 31874, 9434, 24989, 29834, 6374, 27029]);
        assert_eq!(outs[500..508], [-29835, -31110, -27030, -29835, -31110, -29835, -29835, -31110]);
        assert_eq!((s.voices[0].out, s.voices[0].prev), (outs[999], outs[998]));
    }

    /// Feedback at programs/tune's depth stays a tone. A note of period
    /// 160.25 samples: once settled, each sample is close to the one 160
    /// later. (A whole-number period would not do: in integer arithmetic
    /// a voice whose phase repeats exactly locks into an exact cycle even
    /// when its feedback is chaotic; the quarter-sample drift a period
    /// keeps it from that.) Feedback of the last output alone, scaled as
    /// SPEC §sound first had it, gives a mean difference of 8,000 to 17,000
    /// here on outputs of about 14,000: it was heard as static.
    #[test]
    fn feedback_stays_periodic() {
        for depth in [0u8, 128, TUNE_FEEDBACK] {
            let mut s = Sound::default();
            s.load_wave(0, &sine());
            let mut list = vec![op::FEEDBACK, depth, op::VOLUME, 200, op::FREQUENCY];
            list.extend(26_801_668u32.to_le_bytes());
            list.push(op::KEY_ON);
            s.submit(&list);
            let outs = render(&mut s, 9600);
            let level = outs[4800..6080].iter().map(|&x| (x as i64).abs()).sum::<i64>() / 1280;
            let diff = (4800..6080).map(|n| (outs[n] as i64 - outs[n + 160] as i64).abs()).sum::<i64>() / 1280;
            assert!(level > 10_000, "depth {depth}: level {level}");
            assert!(diff < 2_000, "depth {depth}: mean difference between periods {diff}");
        }
    }

    #[test]
    fn noise_and_samples() {
        let mut s = Sound::default();
        // Noise stepping every sample (increment 2^31 carries every other
        // sample; 0xFFFFFFFF carries on all but the first).
        s.submit(&[op::NOISE, op::FREQUENCY, 0xFF, 0xFF, 0xFF, 0xFF, op::KEY_ON]);
        let mut lfsr = 1u16;
        let mut phase = 0u32;
        let mut cur = [0i16; VOICES];
        for _ in 0..100 {
            let o = s.voice_sample(0, &cur);
            let raw = if lfsr & 1 != 0 { 127 } else { -127 };
            assert_eq!(o as i32, (raw * 65535 * 255) >> 16);
            let (p, c) = phase.overflowing_add(u32::MAX);
            phase = p;
            if c {
                let bit = (lfsr ^ (lfsr >> 1)) & 1;
                lfsr = (lfsr >> 1) | (bit << 14);
            }
            cur[0] = o;
        }
        // A one-shot sample of 4 bytes at half speed plays 8 samples, then stops.
        let mut s = Sound::default();
        s.load_sample(1, &[10, 20, 30, 40], 4);
        s.submit(&[op::SAMPLE, 1, op::FREQUENCY, 0x00, 0x80, 0x00, 0x00, op::KEY_ON]);
        let raws: Vec<i32> = (0..10).map(|_| s.voice_sample(0, &[0; VOICES]) as i32).collect();
        let o = |raw: i32| (raw * 65535 * 255) >> 16;
        assert_eq!(raws, [o(10), o(10), o(20), o(20), o(30), o(30), o(40), o(40), 0, 0]);
        assert!(s.voices[0].ended);
        // Looped from byte 2, at speed 1.5: positions 0, 1.5, 3, 4.5→2.5, 4→2 …
        let mut s = Sound::default();
        s.load_sample(0, &[1, 2, 3, 4], 2);
        s.submit(&[op::SAMPLE, 0, op::FREQUENCY, 0x00, 0x80, 0x01, 0x00, op::KEY_ON]);
        let raws: Vec<i32> = (0..6).map(|_| s.voice_sample(0, &[0; VOICES]) as i32).collect();
        assert_eq!(raws, [o(1), o(2), o(4), o(3), o(3), o(4)]);
    }

    #[test]
    fn list_ends() {
        let mut s = Sound::default();
        assert_eq!(s.submit(&[]), RESULT_REJECTED);
        assert_eq!(s.submit(&vec![op::KEY_ON; MAX_LIST + 1]), RESULT_REJECTED);
        // Voice 16 is out of range: the list ends there; the volume before it stays.
        assert_eq!(s.submit(&[op::VOLUME, 9, op::VOICE, 16, op::VOLUME, 1]), RESULT_DONE);
        assert_eq!(s.voices[0].volume, 9);
        // An unknown opcode ends it; so do cut-off operands.
        s.submit(&[op::VOLUME, 8, 0x0F, op::VOLUME, 1]);
        assert_eq!(s.voices[0].volume, 8);
        s.submit(&[op::VOLUME, 7, op::FREQUENCY, 1, 2, 3]);
        assert_eq!((s.voices[0].volume, s.voices[0].incr), (7, 0));
    }

    #[test]
    fn state_round_trips() {
        let mut s = Sound::default();
        s.load_wave(3, &square());
        s.load_sample(2, &[1, 2, 3], 1);
        s.submit(&[op::VOICE, 4, op::WAVE, 3, op::FREQUENCY, 1, 2, 3, 4, op::ENVELOPE, 1, 0, 2, 0, 3, 4, 0, op::KEY_ON]);
        s.render_frame();
        let mut bytes = Vec::new();
        s.write_state(&mut bytes);
        assert_eq!(bytes.len(), VOICES * VOICE_BYTES + WAVES * WAVE_LEN + 4 * 8 + 3);
        let mut back = Sound::default();
        back.restore(&mut Reader::new(&bytes)).unwrap();
        let mut again = Vec::new();
        back.write_state(&mut again);
        assert_eq!(again, bytes);
        s.render_frame();
        back.render_frame();
        assert_eq!(s.take_frame(), back.take_frame());
    }
}
