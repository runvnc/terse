#![no_std]
#![no_main]

use terse::*;

entry!(main);

fn main() {
    let mut buf = [0u8; 64];
    let mut x: i16 = 320;
    loop {
        while let Some(e) = next_event() {
            if e.kind != EV_KEY_DOWN {
                continue;
            }
            match e.key() {
                KEY_LEFT => x -= 10,
                KEY_RIGHT => x += 10,
                KEY_ESCAPE => exit(0),
                _ => {}
            }
        }
        let mut list = VecList::new(&mut buf);
        list.move_to(320 * VEC_UNIT, 400 * VEC_UNIT);
        list.line_to(x * VEC_UNIT, 80 * VEC_UNIT);
        list.submit();
        wait_frame();
    }
}
