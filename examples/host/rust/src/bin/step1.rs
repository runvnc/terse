use host::image::load;
use std::error::Error;
use std::process::exit;

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
            Ok((_, true)) if cpu.x[17] == 0x106 => {
                exit((cpu.x[10] & 0xff) as i32)
            }
            Ok((_, true)) => cpu.x[10] = u32::MAX,
            Ok(_) => {}
        }
    }
}
