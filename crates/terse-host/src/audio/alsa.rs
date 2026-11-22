//! An ALSA PCM through libasound, loaded at run time: nothing to link and
//! no headers to build with. Its `default` PCM is whatever the system
//! made it: PipeWire's or PulseAudio's ALSA plugin, or the card.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::OnceLock;

use libloading::Library;

use super::{RATE, Writer, buffer};

const PLAYBACK: c_int = 0;
#[cfg(target_endian = "little")]
const S16: c_int = 2;
#[cfg(target_endian = "big")]
const S16: c_int = 3;
const RW_INTERLEAVED: c_int = 3;
const EPIPE: c_int = 32;

type Open = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetParams = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type WriteI = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type Recover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type Delay = unsafe extern "C" fn(*mut c_void, *mut c_long) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
type StrError = unsafe extern "C" fn(c_int) -> *const c_char;
/// ALSA calls it with a format and its arguments (`...`); this one reads
/// none of them.
type ErrorHandler = unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char);
type SetErrorHandler = unsafe extern "C" fn(Option<ErrorHandler>) -> c_int;

struct Lib {
    open: Open,
    set_params: SetParams,
    writei: WriteI,
    recover: Recover,
    delay: Delay,
    close: Close,
    /// snd_pcm_drain, of the same type.
    drain: Close,
    strerror: StrError,
}

unsafe extern "C" fn quiet(_: *const c_char, _: c_int, _: *const c_char, _: c_int, _: *const c_char) {}

/// The library, loaded once and never unloaded (its plugins may start
/// threads of their own).
fn lib() -> Result<&'static Lib, String> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let l = Library::new("libasound.so.2").map_err(|e| format!("libasound.so.2 is not installed ({e})"))?;
        let lib = Lib {
            open: *l.get::<Open>(b"snd_pcm_open\0").map_err(|e| e.to_string())?,
            set_params: *l.get::<SetParams>(b"snd_pcm_set_params\0").map_err(|e| e.to_string())?,
            writei: *l.get::<WriteI>(b"snd_pcm_writei\0").map_err(|e| e.to_string())?,
            recover: *l.get::<Recover>(b"snd_pcm_recover\0").map_err(|e| e.to_string())?,
            delay: *l.get::<Delay>(b"snd_pcm_delay\0").map_err(|e| e.to_string())?,
            close: *l.get::<Close>(b"snd_pcm_close\0").map_err(|e| e.to_string())?,
            drain: *l.get::<Close>(b"snd_pcm_drain\0").map_err(|e| e.to_string())?,
            strerror: *l.get::<StrError>(b"snd_strerror\0").map_err(|e| e.to_string())?,
        };
        // ALSA's own messages (a missing device, a plugin with no server)
        // would go to standard error at every try; the reason is reported
        // once instead.
        if std::env::var_os("TERSE_AUDIO_DEBUG").is_none()
            && let Ok(set) = l.get::<SetErrorHandler>(b"snd_lib_error_set_handler\0")
        {
            set(Some(quiet));
        }
        std::mem::forget(l);
        Ok(lib)
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn error(lib: &Lib, code: c_int) -> String {
    unsafe {
        let s = (lib.strerror)(code);
        if s.is_null() { format!("error {code}") } else { CStr::from_ptr(s).to_string_lossy().into_owned() }
    }
}

pub struct Alsa {
    lib: &'static Lib,
    pcm: *mut c_void,
    underruns: u64,
}

// The PCM is used by one thread at a time (the output's).
unsafe impl Send for Alsa {}

/// PCM `name`, 48,000 Hz stereo, 16 bits, about three frames of buffer
/// (ALSA starts it once full), and a name for it.
pub fn open(name: &str) -> Result<(Alsa, String), String> {
    let lib = lib()?;
    let c = CString::new(name).map_err(|e| e.to_string())?;
    let mut pcm = std::ptr::null_mut();
    let r = unsafe { (lib.open)(&mut pcm, c.as_ptr(), PLAYBACK, 0) };
    if r < 0 {
        return Err(format!("\"{name}\": {}", error(lib, r)));
    }
    let a = Alsa { lib, pcm, underruns: 0 };
    let latency_us = (buffer() as u64 * 1_000_000 / RATE as u64) as c_uint;
    let r = unsafe { (lib.set_params)(pcm, S16, RW_INTERLEAVED, 2, RATE, 1, latency_us) };
    if r < 0 {
        return Err(format!("\"{name}\": {}", error(lib, r)));
    }
    Ok((a, format!("ALSA PCM \"{name}\" (libasound)")))
}

impl Writer for Alsa {
    fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        let mut rest = samples;
        while !rest.is_empty() {
            let n = unsafe { (self.lib.writei)(self.pcm, rest.as_ptr().cast(), (rest.len() / 2) as c_ulong) };
            if n < 0 {
                let e = n as c_int;
                if e == -EPIPE {
                    self.underruns += 1;
                }
                // An underrun (EPIPE) or a suspend (ESTRPIPE): prepare it
                // and go on; anything else ends the output.
                let r = unsafe { (self.lib.recover)(self.pcm, e, 1) };
                if r < 0 {
                    return Err(error(self.lib, e));
                }
                continue;
            }
            rest = &rest[(n as usize * 2).min(rest.len())..];
        }
        Ok(())
    }

    fn delay_us(&mut self) -> Option<u64> {
        let mut d: c_long = 0;
        let r = unsafe { (self.lib.delay)(self.pcm, &mut d) };
        (r >= 0).then(|| d.max(0) as u64 * 1_000_000 / RATE as u64)
    }

    fn drain(&mut self) {
        unsafe { (self.lib.drain)(self.pcm) };
    }

    fn underruns(&self) -> Option<u64> {
        Some(self.underruns)
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        unsafe { (self.lib.close)(self.pcm) };
    }
}
