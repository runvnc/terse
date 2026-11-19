//! `terse-dev-crash`: a terminal (slot 2) for testing the core's handling
//! of failed drivers (DEVICES.md 10). It answers with the terminal's table
//! (its deferred calls by the table's rules, its state not kept), but with
//! `TERSE_CRASH=exit` it exits in the middle of the `TERSE_CRASH_AT`th
//! answered call or BEGIN, with `TERSE_CRASH=hang` it stops answering
//! there, and with `TERSE_CRASH=slow` it waits two seconds and then becomes
//! the terminal driver named by `TERSE_CRASH_THEN`.

use std::collections::HashSet;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, Mem, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, Open, SlotInfo, access, rule};
use terse_core::machine::charge;
use terse_core::terminal::{CELL_BYTES, CELLS};
use terse_core::{COLS, ROWS, Terminal};

fn table() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 0, charge::MAX_WRITE, access::READ | access::ROM).batch(rule::ARGUMENT, 1, 0, 0, 0).per_byte(charge::WRITE_BYTE as u16),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0).charge_when(0, 1, 3, charge::SCROLL_CLEAR),
        e(2).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(3).sets_a1(),
        e(4).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(5).sets_a1(),
        e(6).buffer(1, 2, 4, 0, (CELLS * CELL_BYTES) as u32, access::WRITE).check(0, CELLS as u32 - 1),
        e(7).buffer(1, 2, 4, 0, (CELLS * CELL_BYTES) as u32, access::READ | access::ROM).check(0, CELLS as u32 - 1).batch(rule::ARGUMENT, 2, 0, 0, 0).per4(),
        e(8).buffer(1, 2, 3, 0, 768, access::READ | access::ROM).check(0, 255).batch(rule::ARGUMENT, 2, 0, 0, 0).per4(),
        e(9).sets_a1().batch(rule::CONSTANT, 0, COLS as u32, ROWS as u32, 0),
    ]
}

struct Crash {
    mode: String,
    at: u64,
    seen: u64,
    table: Vec<CallEntry>,
    inst: HashSet<u32>,
}

impl Crash {
    fn tick(&mut self) {
        self.seen += 1;
        if self.seen < self.at {
            return;
        }
        match self.mode.as_str() {
            "exit" => std::process::exit(9),
            "hang" => loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            },
            _ => {}
        }
    }
}

impl Device for Crash {
    fn info(&mut self, _: &Session) -> DeviceInfo {
        DeviceInfo { name: "terse-dev-crash".into(), slots: vec![SlotInfo { slot: 2, version: 0, layers: Vec::new(), table: table() }], ..Default::default() }
    }

    fn open(&mut self, instance: u32, _: &Open, _: &Session) -> Result<(), String> {
        self.inst.insert(instance);
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.inst.remove(&instance);
    }

    fn reset(&mut self, _: u32) {}

    fn call(&mut self, _: u32, c: &CallIn, _: &mut dyn Mem) -> Reply {
        if !c.batched {
            self.tick();
        }
        match self.table.iter().find(|e| e.function == c.function && e.batchable()) {
            Some(e) => {
                let (a0, a1, charge) = e.batch_outcome(&c.args, c.buf.len() as u32);
                Reply { a0, a1, charge }
            }
            None => Reply { a0: 0, a1: Some(0), charge: 0 },
        }
    }

    fn begin(&mut self, _: u32, _: &Begin, _: &mut dyn Mem) -> Inputs {
        self.tick();
        Inputs::default()
    }

    fn end(&mut self, _: u32, _: u32, _: bool) {}

    fn state(&self, _: u32, _: u8) -> Vec<u8> {
        let mut out = Vec::new();
        Terminal::default().write_state(&mut out);
        out
    }

    fn restore(&mut self, _: u32, _: u8, _: &[u8]) -> Option<usize> {
        None
    }
}

fn main() -> std::process::ExitCode {
    let mode = std::env::var("TERSE_CRASH").unwrap_or_default();
    if mode == "slow" {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if let Ok(then) = std::env::var("TERSE_CRASH_THEN") {
            use std::os::unix::process::CommandExt;
            let e = std::process::Command::new(then).exec();
            eprintln!("terse-dev-crash: {e}");
            return std::process::ExitCode::from(1);
        }
    }
    let at = std::env::var("TERSE_CRASH_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
    terse_bus::driver::run(&mut Crash { mode, at, seen: 0, table: table(), inst: HashSet::new() }, "terse-dev-crash")
}
