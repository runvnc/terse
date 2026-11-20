//! Plays the sound device (SPEC §sound.presentation) through the system's audio output
//! (cpal; on Linux ALSA, which PipeWire and PulseAudio serve). Only with
//! the `audio` feature, and only in a window: the device is opened the
//! first time a machine gives samples, so a program without sound never
//! opens it. The samples are presentation, not machine state: when the
//! output falls behind, the oldest are dropped; when it runs dry, it plays
//! silence. `TERSE_AUDIO=off` keeps the device closed.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// At most this many sample pairs wait (four frames): more is latency.
const MAX_QUEUED: usize = 4 * terse_core::sound::FRAME_SAMPLES;

pub struct Output {
    queue: Arc<Mutex<VecDeque<i16>>>,
    /// Kept alive while it plays.
    _stream: cpal::Stream,
}

impl Output {
    /// The default output device at 48,000 Hz, stereo, or why not.
    pub fn open() -> Result<Output, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let config = cpal::StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(terse_core::sound::RATE),
            buffer_size: cpal::BufferSize::Default,
        };
        let queue: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::with_capacity(4 * MAX_QUEUED)));
        let q = queue.clone();
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let mut q = q.lock().unwrap();
                    for d in data.iter_mut() {
                        *d = q.pop_front().map_or(0.0, |s| s as f32 / 32768.0);
                    }
                },
                |e| eprintln!("terse-host: audio: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Output { queue, _stream: stream })
    }

    /// One frame's samples, left and right interleaved.
    pub fn push(&self, frame: &[i16]) {
        let mut q = self.queue.lock().unwrap();
        q.extend(frame);
        let excess = q.len().saturating_sub(2 * MAX_QUEUED);
        q.drain(..excess);
    }
}

/// The host's sound: closed until a machine first gives samples.
#[derive(Default)]
pub struct Audio {
    output: Option<Output>,
    /// It was asked for and could not be opened, or is turned off.
    off: bool,
}

impl Audio {
    /// Off from the start (a self-test, `TERSE_AUDIO=off`).
    pub fn disabled() -> Audio {
        Audio { output: None, off: true }
    }

    /// The system frame just run: its samples, if any machine gave some.
    pub fn frame(&mut self, system: &mut terse_core::System) {
        let mut frame = [0i16; 2 * terse_core::sound::FRAME_SAMPLES];
        if !system.take_audio(&mut frame) || self.off {
            return;
        }
        if self.output.is_none() {
            match Output::open() {
                Ok(o) => self.output = Some(o),
                Err(e) => {
                    eprintln!("terse-host: no sound: {e}");
                    self.off = true;
                    return;
                }
            }
        }
        self.output.as_ref().unwrap().push(&frame);
    }
}
