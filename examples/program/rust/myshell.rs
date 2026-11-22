#![no_std]
#![no_main]

use terse::*;

entry!(main);

fn main() {
    let hello = mach_start_named(&[0; 32], "hello");
    mach_place(hello, 0, 0, 640, 480);
    mach_focus(hello);
    loop {
        wait_frame();
    }
}
