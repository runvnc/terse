use host::cpu::{A0, A7, UNKNOWN_CALL};
use host::image::load;
use std::error::Error;
use std::process::exit;

const EXIT: u32 = 0x106;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let file = std::fs::read(&args[1])?;
    let (mut cpu, _) = load(&file)?;
    loop {
        match cpu.step() {
            Err(fault) => {
                eprintln!("{fault}");
                exit(1)
            }
            Ok((_, true)) if cpu.x[A7] == EXIT => {
                exit(cpu.x[A0] as u8 as i32)
            }
            Ok((_, true)) => cpu.x[A0] = UNKNOWN_CALL,
            Ok(_) => {}
        }
    }
}
