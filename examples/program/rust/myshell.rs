#![no_std]
#![no_main]

use terse::*;

entry!(main);

const BUILT_IN: [u8; 32] = [0; 32];

fn main() {
    let hello = mach_start_named(&BUILT_IN, "hello");
    mach_place(hello, 0, 0, 640, 480);
    mach_focus(hello);
    loop {
        wait_frame();
    }
}
