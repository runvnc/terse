use host::bus::Bus;
use host::frames::{Machine, Status};
use host::image::load;
use std::error::Error;
use std::process::exit;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let file = std::fs::read(&args[1])?;
    let frames: u32 = args[2].parse()?;
    let (cpu, devices) = load(&file)?;
    let mut bus = (!devices.is_empty())
        .then(|| Bus::start(&args[3], &cpu, &file, &devices));
    let mut m = Machine::new(cpu);
    while m.running() && m.frame < frames {
        if let Some(bus) = &mut bus {
            m.budget -= bus.begin(m.frame);
        }
        m.run_frame(|cpu, budget, slot, function| {
            let Some(bus) = &mut bus else {
                return (u32::MAX, 4);
            };
            bus.call(cpu, budget, slot, function)
        });
        if let Some(bus) = &mut bus {
            bus.end(m.frame, !m.running());
        }
    }
    let (grants, terminal) = match bus {
        Some(mut bus) => {
            let terminal = bus.state(2);
            let grants = std::mem::take(&mut bus.grants);
            bus.close();
            (grants, Some(terminal))
        }
        None => (Vec::new(), None),
    };
    let id = blake3::hash(&file);
    let terminal = terminal.as_deref();
    let state = m.state(id.as_bytes(), &grants, terminal);
    println!("{}", blake3::hash(&state));
    match m.status {
        Status::Faulted(fault) => {
            eprintln!("{fault}");
            exit(1)
        }
        Status::Exited(code) => exit(code as i32),
        Status::Running => Ok(()),
    }
}
