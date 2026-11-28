//! Tersenet in a web page (tersenet.org/try.html): the machine of
//! terse-core, compiled to WebAssembly, run by a small script (`try.js`)
//! that gives it the page's keys and pointer, runs a frame every 1/60 s and
//! draws the commands [`draw`] makes on a canvas.
//!
//! The module's interface is plain functions on its memory (no
//! wasm-bindgen): the script allocates with [`tw_alloc`], writes the
//! content bundle there, starts a system with [`tw_start`], and each frame
//! calls the input functions, [`tw_frame`] and [`tw_draw`]. The module
//! imports one function, `env.tw_random(ptr, len)`, which fills memory with
//! random bytes (`crypto.getRandomValues`).
//!
//! What runs is what a window runs: the built-in shell and its programs
//! (or one program alone), each machine with its 125,000 units a frame
//! (SPEC §frames), deterministic given its input. What the page leaves
//! out: the network (no relay, no feed key; the content device answers
//! from the blobs the page brought), sound, game controllers, snapshots,
//! the phosphor look of terse-host (the canvas draws plain lines with a
//! glow), and the host's own page typesetting: pages are laid out with
//! the reference layout (SPEC §page.reference) and drawn in the browser's
//! fonts.

pub mod bundle;
pub mod content;
pub mod draw;
pub mod host;
pub mod keys;

use std::cell::RefCell;
use std::collections::HashMap;

pub use host::{Host, Start};

#[derive(Default)]
struct State {
    host: Option<Host>,
    last: HashMap<u32, Vec<u8>>,
    ops: draw::Ops,
    error: String,
    copied: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// # Safety
/// `ptr` and `len` name memory the module gave out.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len) } }
}

/// Memory for `len` bytes, for the script to write into.
#[unsafe(no_mangle)]
pub extern "C" fn tw_alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len.max(1)];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Gives back memory from [`tw_alloc`].
///
/// # Safety
/// `ptr` and `len` are what [`tw_alloc`] gave and was asked for.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tw_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, len.max(1), len.max(1)) });
}

/// Starts a system from the bundle at `bundle`: the built-in shell if
/// `title` is empty, else the built-in program with that title alone.
/// Returns 1, or 0 with the reason in [`tw_error`].
///
/// # Safety
/// The pointers and lengths name memory the module gave out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tw_start(bundle: *const u8, bundle_len: usize, title: *const u8, title_len: usize) -> u32 {
    let (bundle, title) = unsafe { (bytes(bundle, bundle_len), bytes(title, title_len)) };
    let title = String::from_utf8_lossy(title);
    let start = if title.is_empty() { Start::Shell } else { Start::Alone(&title) };
    with(|s| {
        s.last.clear();
        match Host::new(bundle, start) {
            Ok(h) => {
                s.host = Some(h);
                1
            }
            Err(e) => {
                s.error = e;
                0
            }
        }
    })
}

/// The last error's text: its address; [`tw_error_len`] its length.
#[unsafe(no_mangle)]
pub extern "C" fn tw_error() -> *const u8 {
    with(|s| s.error.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn tw_error_len() -> usize {
    with(|s| s.error.len())
}

/// A key went down (1) or up (0): `code` is its `KeyboardEvent.code`,
/// `mods` the modifier bits (SPEC §events). Returns the key's HID usage, 0
/// if it has none (the key is ignored).
///
/// # Safety
/// `code` and `len` name memory the module gave out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tw_key(down: u32, code: *const u8, len: usize, mods: u32) -> u32 {
    let code = String::from_utf8_lossy(unsafe { bytes(code, len) }).into_owned();
    let Some(usage) = keys::hid_usage(&code) else { return 0 };
    with(|s| {
        if let Some(h) = &mut s.host {
            // F12 is the home key of a system (SPEC §machines.input), as in terse-host.
            if usage == 0x45 && h.system.is_system() {
                if down != 0 {
                    h.input.home();
                }
            } else {
                h.input.key(down != 0, usage, mods as u8);
            }
        }
    });
    usage as u32
}

/// Text typed: one Unicode scalar value.
#[unsafe(no_mangle)]
pub extern "C" fn tw_text(cp: u32) {
    if let Some(c) = char::from_u32(cp) {
        with(|s| s.host.as_mut().map(|h| h.input.text(c)));
    }
}

/// The paste key (`code`, with `mods`) was pressed with `text` (UTF-8) to
/// paste: its key down, the text, and its key up once the text has gone.
///
/// # Safety
/// The pointers and lengths name memory the module gave out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tw_paste(code: *const u8, code_len: usize, mods: u32, text: *const u8, text_len: usize) {
    let code = String::from_utf8_lossy(unsafe { bytes(code, code_len) }).into_owned();
    let text = String::from_utf8_lossy(unsafe { bytes(text, text_len) }).into_owned();
    let Some(usage) = keys::hid_usage(&code) else { return };
    with(|s| s.host.as_mut().map(|h| h.input.paste(usage, mods as u8, &text)));
}

/// The pointer is at (x, y) in the shell's screen units.
#[unsafe(no_mangle)]
pub extern "C" fn tw_pointer(x: u32, y: u32) {
    with(|s| s.host.as_mut().map(|h| h.input.pointer(x.min(639) as u16, y.min(479) as u16)));
}

/// The buttons now held (bit 0 primary, 1 secondary, 2 middle).
#[unsafe(no_mangle)]
pub extern "C" fn tw_buttons(buttons: u32) {
    with(|s| s.host.as_mut().map(|h| h.input.buttons(buttons as u8)));
}

/// Wheel travel in steps (positive: down, right).
#[unsafe(no_mangle)]
pub extern "C" fn tw_wheel(v: f64, h: f64) {
    with(|s| s.host.as_mut().map(|x| x.input.wheel(v, h)));
}

/// The home key, from the page's button.
#[unsafe(no_mangle)]
pub extern "C" fn tw_home() {
    with(|s| s.host.as_mut().map(|h| h.input.home()));
}

/// Runs the next frame at `unix_time` (seconds). Returns 1 while the
/// system runs, 0 once it has stopped.
#[unsafe(no_mangle)]
pub extern "C" fn tw_frame(unix_time: f64) -> u32 {
    with(|s| s.host.as_mut().map_or(0, |h| h.frame(unix_time as u32) as u32))
}

/// Budget units the machines used in the last frame, all together.
#[unsafe(no_mangle)]
pub extern "C" fn tw_used() -> f64 {
    with(|s| s.host.as_ref().map_or(0.0, |h| h.used.iter().map(|u| u.1).sum::<u64>() as f64))
}

/// Draws what the system shows now (module [`draw`]): the commands'
/// address; [`tw_draw_len`] their length.
#[unsafe(no_mangle)]
pub extern "C" fn tw_draw() -> *const u8 {
    with(|s| {
        let State { host, last, ops, .. } = s;
        match host {
            Some(h) => draw::frame(h, last, ops),
            None => ops.buf.clear(),
        }
        ops.buf.as_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn tw_draw_len() -> usize {
    with(|s| s.ops.buf.len())
}

/// Text a machine offered the clipboard since the last call (SPEC
/// §system `copy`): its length (0: none); the text is then at
/// [`tw_copied`] until the next call.
#[unsafe(no_mangle)]
pub extern "C" fn tw_take_copied() -> usize {
    with(|s| {
        let text = s.host.as_mut().and_then(|h| h.copied.take()).unwrap_or_default();
        s.copied = String::from_utf8_lossy(&text).into_owned();
        s.copied.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn tw_copied() -> *const u8 {
    with(|s| s.copied.as_ptr())
}
