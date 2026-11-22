#![no_std]
#![no_main]

use terse::*;

entry!(main);

fn near(a: i16, b: i16) -> bool {
    (a - b).abs() < 12
}

fn show_score(score: u32) {
    term_goto(0, 0);
    term_puts("Score ");
    term_put_dec(score);
}

fn main() {
    let mut buf = [0u8; 64];
    let (mut x, mut y, mut dx, mut dy) = (320i16, 240i16, 0, 0);
    let (mut goal_x, mut goal_y, mut score) = (160i16, 120i16, 0);
    show_score(score);
    loop {
        while let Some(e) = next_event() {
            if e.kind != EV_KEY_DOWN {
                continue;
            }
            (dx, dy) = match e.key() {
                KEY_LEFT => (-2, 0),
                KEY_RIGHT => (2, 0),
                KEY_UP => (0, -2),
                KEY_DOWN => (0, 2),
                KEY_ESCAPE => exit(0),
                _ => (dx, dy),
            };
        }
        x = (x + dx).rem_euclid(640);
        y = (y + dy).rem_euclid(480);
        if near(x, goal_x) && near(y, goal_y) {
            score += 1;
            show_score(score);
            goal_x = 40 + random_byte() as i16 * 2;
            goal_y = 40 + random_byte() as i16 * 3 / 2;
        }
        let mut list = VecList::new(&mut buf);
        list.move_to(x * VEC_UNIT, y * VEC_UNIT);
        list.disc(4 * VEC_UNIT as u16);
        list.move_to(goal_x * VEC_UNIT, goal_y * VEC_UNIT);
        list.circle(10 * VEC_UNIT as u16);
        list.submit();
        wait_frame();
    }
}
