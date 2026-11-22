#![no_std]
#![no_main]

use terse::*;

entry!(main);

fn main() {
    term_color(15, 4);
    term_puts(" Hello from hello! ");
    term_color(7, 0);
    term_puts("\n\nPress Esc to exit.\n");

    loop {
        wait_frame();
        while let Some(e) = next_event() {
            if e.kind == EV_KEY_DOWN && e.key() == KEY_ESCAPE {
                exit(0);
            }
        }
    }
}
