use host::cpu::UNKNOWN_CALL;
use host::frames::{BASE_CHARGE, Machine, Status};
use host::image::load;
use std::error::Error;
use std::process::exit;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let file = std::fs::read(&args[1])?;
    let frames: u32 = args[2].parse()?;
    let (cpu, _) = load(&file)?;
    let mut m = Machine::new(cpu);
    while m.running() && m.frame < frames {
        m.run_frame(|_, _, _, _| (UNKNOWN_CALL, BASE_CHARGE));
    }
    match m.status {
        Status::Faulted(fault) => {
            eprintln!("{fault}");
            exit(1)
        }
        Status::Exited(code) => exit(code as i32),
        Status::Running => Ok(()),
    }
}
