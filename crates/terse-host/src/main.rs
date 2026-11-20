//! `terse-host`: runs a Terse image in a window, or renders a screenshot.

// On Windows a release build is a windowed program: opened from Explorer it
// gets no console window. Started from a console, it writes to that console
// ([`attach_parent_console`]).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    attach_parent_console();
    terse_host::main()
}

/// A windowed program on Windows starts with no standard error unless it
/// was redirected: when started from a console, write to that console, so
/// the relay line, errors and the usage are seen there.
#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn AttachConsole(process: u32) -> i32;
    }
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: both only query or attach this process's console.
    unsafe {
        if GetStdHandle(STD_ERROR_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
