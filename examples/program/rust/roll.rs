#![no_std]
#![no_main]

use terse::*;

entry!(main);

const DICE: u32 = 240;
const DICE_SEED: u32 = DICE << 8;
const DICE_ROLL: u32 = DICE << 8 | 1;

fn main() {
    if device(DICE).is_none() {
        term_puts("No dice here: run me with example.dice.\n");
        loop {
            wait_frame();
        }
    }
    let mut seed = [0u8; 4];
    random(&mut seed);
    unsafe { call(DICE_SEED, u32::from_le_bytes(seed), 0, 0) };
    loop {
        for _ in 0..30 {
            wait_frame();
        }
        term_puts("You rolled ");
        term_put_dec(unsafe { call(DICE_ROLL, 6, 0, 0) });
        term_puts(".\n");
    }
}
