use host::bus::{Bus, TERMINAL};
use host::frames::{Machine, Status};
use host::image::load;
use std::error::Error;
use std::process::exit;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let file = std::fs::read(&args[1])?;
    let frames: u32 = args[2].parse()?;
    let (cpu, devices) = load(&file)?;
    let mut bus = Bus::start(&args[3], &cpu, &file, &devices);
    let mut m = Machine::new(cpu);
    while m.running() && m.frame < frames {
        m.budget -= bus.begin(m.frame);
        m.run_frame(|cpu, budget, slot, function| {
            bus.call(cpu, budget, slot, function)
        });
        bus.end(m.frame, !m.running());
    }
    print!("{}", screen(&bus.state(TERMINAL)));
    bus.close();
    match m.status {
        Status::Faulted(fault) => {
            eprintln!("{fault}");
            exit(1)
        }
        Status::Exited(code) => exit(code as i32),
        Status::Running => Ok(()),
    }
}

const COLUMNS: usize = 80;
const ROWS: usize = 30;
const CELL_SIZE: usize = 4;
const CONTINUATION: u16 = 0xffff;

fn screen(grid: &[u8]) -> String {
    let code = |cell: usize| {
        let at = CELL_SIZE * cell;
        u16::from_le_bytes([grid[at], grid[at + 1]])
    };
    let mut text = String::new();
    for row in 0..ROWS {
        let mut line = String::new();
        for col in 0..COLUMNS {
            let cell = row * COLUMNS + col;
            let right_half =
                col > 0 && code(cell - 1) != CONTINUATION;
            match code(cell) {
                CONTINUATION if right_half => {}
                CONTINUATION => line.push(' '),
                c => line += &String::from_utf16_lossy(&[c]),
            }
        }
        text += line.trim_end_matches(' ');
        text += "\n";
    }
    text
}
