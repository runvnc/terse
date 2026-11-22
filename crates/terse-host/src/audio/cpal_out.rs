//! The system's default output through cpal (macOS: CoreAudio; Windows:
//! WASAPI), which pulls samples from the queue in its own callback.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{RATE, Shared};

pub type Stream = cpal::Stream;

pub fn open(shared: Arc<Shared>) -> Result<(Stream, String), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let name = format!("{} ({})", device.name().unwrap_or_else(|_| "the default output".into()), host.id().name());
    let config = cpal::StreamConfig { channels: 2, sample_rate: cpal::SampleRate(RATE), buffer_size: cpal::BufferSize::Default };
    let (data, errors) = (shared.clone(), shared);
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                let mut q = data.q.lock().unwrap();
                for d in out.iter_mut() {
                    *d = q.samples.pop_front().map_or(0.0, |s| s as f32 / 32768.0);
                }
            },
            move |e| match e {
                cpal::StreamError::DeviceNotAvailable => errors.q.lock().unwrap().dead = Some(e.to_string()),
                e => eprintln!("terse-host: sound: {e}"),
            },
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, name))
}
