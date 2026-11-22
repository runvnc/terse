//! A PulseAudio server (PulseAudio itself, or PipeWire's pipewire-pulse)
//! through libpulse's simple API, loaded at run time: nothing to link and
//! no headers to build with.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::OnceLock;

use libloading::Library;

use super::{RATE, Writer, buffer, start};

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const PLAYBACK: c_int = 1;
#[cfg(target_endian = "little")]
const S16NE: c_int = 3;
#[cfg(target_endian = "big")]
const S16NE: c_int = 4;

type New = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const SampleSpec,
    *const c_void,
    *const BufferAttr,
    *mut c_int,
) -> *mut c_void;
type Write = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type Latency = unsafe extern "C" fn(*mut c_void, *mut c_int) -> u64;
type Free = unsafe extern "C" fn(*mut c_void);
type Drain = unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int;
type StrError = unsafe extern "C" fn(c_int) -> *const c_char;

struct Lib {
    new: New,
    write: Write,
    latency: Latency,
    free: Free,
    drain: Drain,
    strerror: Option<StrError>,
}

/// The library, loaded once and never unloaded (its threads and the
/// server's plugins may outlive a stream).
fn lib() -> Result<&'static Lib, String> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let simple = Library::new("libpulse-simple.so.0").map_err(|e| format!("libpulse-simple.so.0 is not installed ({e})"))?;
        let pulse = Library::new("libpulse.so.0").ok();
        let lib = Lib {
            new: *simple.get::<New>(b"pa_simple_new\0").map_err(|e| e.to_string())?,
            write: *simple.get::<Write>(b"pa_simple_write\0").map_err(|e| e.to_string())?,
            latency: *simple.get::<Latency>(b"pa_simple_get_latency\0").map_err(|e| e.to_string())?,
            free: *simple.get::<Free>(b"pa_simple_free\0").map_err(|e| e.to_string())?,
            drain: *simple.get::<Drain>(b"pa_simple_drain\0").map_err(|e| e.to_string())?,
            strerror: pulse.as_ref().and_then(|p| p.get::<StrError>(b"pa_strerror\0").ok().map(|s| *s)),
        };
        std::mem::forget(simple);
        if let Some(p) = pulse {
            std::mem::forget(p);
        }
        Ok(lib)
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn error(lib: &Lib, code: c_int) -> String {
    match lib.strerror {
        Some(f) => unsafe {
            let s = f(code);
            if s.is_null() { format!("error {code}") } else { CStr::from_ptr(s).to_string_lossy().into_owned() }
        },
        None => format!("error {code}"),
    }
}

pub struct Pulse {
    lib: &'static Lib,
    s: *mut c_void,
}

// The stream is used by one thread at a time (the output's).
unsafe impl Send for Pulse {}

/// A playback stream on `server` (else the default one: $PULSE_SERVER,
/// the client configuration, $XDG_RUNTIME_DIR/pulse/native), and a name
/// for it.
pub fn open(server: Option<&str>) -> Result<(Pulse, String), String> {
    let lib = lib()?;
    let server_c = server.map(CString::new).transpose().map_err(|e| e.to_string())?;
    let spec = SampleSpec { format: S16NE, rate: RATE, channels: 2 };
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: (buffer() * 4) as u32,
        prebuf: (start() * 4) as u32,
        minreq: (buffer() * 4 / 12) as u32,
        fragsize: u32::MAX,
    };
    let mut err: c_int = 0;
    let s = unsafe {
        (lib.new)(
            server_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            c"Tersenet".as_ptr(),
            PLAYBACK,
            std::ptr::null(),
            c"sound".as_ptr(),
            &spec,
            std::ptr::null(),
            &attr,
            &mut err,
        )
    };
    if s.is_null() {
        return Err(error(lib, err));
    }
    let name = match server {
        Some(s) => format!("PulseAudio server {s} (libpulse-simple)"),
        None => "PulseAudio server: PipeWire or PulseAudio (libpulse-simple)".into(),
    };
    Ok((Pulse { lib, s }, name))
}

impl Writer for Pulse {
    fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        let mut err: c_int = 0;
        let r = unsafe { (self.lib.write)(self.s, samples.as_ptr().cast(), samples.len() * 2, &mut err) };
        if r < 0 { Err(error(self.lib, err)) } else { Ok(()) }
    }

    fn delay_us(&mut self) -> Option<u64> {
        let mut err: c_int = 0;
        let l = unsafe { (self.lib.latency)(self.s, &mut err) };
        (l != u64::MAX).then_some(l)
    }

    /// The simple API does not say (a recording of the sink does).
    fn drain(&mut self) {
        let mut err: c_int = 0;
        unsafe { (self.lib.drain)(self.s, &mut err) };
    }

    fn underruns(&self) -> Option<u64> {
        None
    }
}

impl Drop for Pulse {
    fn drop(&mut self) {
        unsafe { (self.lib.free)(self.s) }
    }
}
