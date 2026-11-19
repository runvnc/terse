//! `terse-dev-content`: the content device (SPEC 9, slot 4) as a driver
//! (DEVICES.md): terse-core's `Content` for the state and terse-net's
//! content service for the network, run as `terse run` runs them (every
//! request completed at the start of the next frame). Options: those of
//! `terse run` for content (`--store`, `--config`, `--data`, `--relay`,
//! `--consent`, `--profile reading`, `--no-feed-relays`,
//! `--no-program-download`).
//!
//! The device reads and writes guest memory it has no buffer for (a put's
//! data, a completion's delivered bytes): it keeps a shadow of RAM, fills
//! the parts it needs with the core's memory (READ, or the memory file),
//! lets terse-core's `Content` work on the shadow, and writes back what it
//! touched.

use terse_bus::driver::{Begin, CallIn, Device, Inputs, Mem, Reply, Session};
use terse_bus::inproc::ContentOptions;
use terse_bus::proto::{CallEntry, DeviceInfo, PosEvent, access, position, u32_at};
use terse_core::content::{ADDR, BLOCK_LEN, Content, LENGTH, SIZE};
use terse_core::machine::copy_charge;
use terse_core::system::System;
use terse_core::{Completion, ERR_UNKNOWN, Event, Outcome, RAM_BASE};
use terse_net::{ContentService, Owner};

struct ContentDriver {
    options: ContentOptions,
    content: Content,
    service: Option<ContentService>,
    completions: Vec<Completion>,
    feeds_updated: bool,
    image_id: [u8; 32],
    rom: Vec<u8>,
    shadow: Vec<u8>,
    replay: bool,
}

/// Guest ranges (address, length) to bring into the shadow.
fn fetch(shadow: &mut [u8], mem: &mut dyn Mem, ranges: &[(u32, usize)]) {
    for &(addr, len) in ranges {
        let off = addr.wrapping_sub(RAM_BASE) as usize;
        if len == 0 || off > shadow.len() || shadow.len() - off < len {
            continue;
        }
        if let Some(b) = mem.read(addr, len) {
            shadow[off..off + len].copy_from_slice(&b);
        }
    }
}

/// Writes back the bytes of `ranges` that differ from `before`.
fn write_back(shadow: &[u8], before: &[Vec<u8>], mem: &mut dyn Mem, ranges: &[(u32, usize)]) {
    for (&(addr, len), old) in ranges.iter().zip(before) {
        let off = addr.wrapping_sub(RAM_BASE) as usize;
        if len == 0 || off > shadow.len() || shadow.len() - off < len {
            continue;
        }
        let now = &shadow[off..off + len];
        if now != &old[..] {
            mem.write(addr, now);
        }
    }
}

fn snapshot(shadow: &[u8], ranges: &[(u32, usize)]) -> Vec<Vec<u8>> {
    ranges
        .iter()
        .map(|&(addr, len)| {
            let off = addr.wrapping_sub(RAM_BASE) as usize;
            if len == 0 || off > shadow.len() || shadow.len() - off < len { Vec::new() } else { shadow[off..off + len].to_vec() }
        })
        .collect()
}

/// The completions record: count, then each as STATE.md 5 has it; then
/// the feeds-updated flag.
fn encode(completions: &[Completion], feeds_updated: bool) -> Vec<u8> {
    let mut out = (completions.len() as u32).to_le_bytes().to_vec();
    for c in completions {
        terse_core::record::encode_completion(c, &mut out);
    }
    out.push(feeds_updated as u8);
    out
}

fn decode(b: &[u8]) -> Option<(Vec<Completion>, bool)> {
    let mut r = terse_bus::proto::Reader::new(b);
    let n = r.u32().ok()?;
    let mut v = Vec::new();
    for _ in 0..n {
        let handle = r.u8().ok()?;
        let outcome = match r.u8().ok()? {
            k @ (0x01 | 0x03) => {
                let size = r.u32().ok()?;
                let data = r.blob().ok()?.to_vec();
                if k == 1 { Outcome::Got { size, data } } else { Outcome::Records { total: size, data } }
            }
            0x02 => Outcome::Put { id: r.take(32).ok()?.try_into().ok()? },
            0x04 => Outcome::Done,
            0x05 => Outcome::Published { seq: r.u32().ok()? },
            0x06 => Outcome::Identity { key: r.take(32).ok()?.try_into().ok()? },
            0x07 => Outcome::NotFound,
            0x08 => Outcome::Failed,
            0x09 => Outcome::Refused,
            0x0A => Outcome::NoRoom,
            0x0B => Outcome::BadRequest,
            _ => return None,
        };
        v.push(Completion { handle, outcome });
    }
    Some((v, r.u8().ok()? != 0))
}

impl ContentDriver {
    /// Every guest range a completion may write: each pending request's
    /// block and its data range (from the state section).
    fn pending_ranges(&self) -> Vec<(u32, usize)> {
        let mut s = Vec::new();
        self.content.write_state(&mut s);
        let mut v = Vec::new();
        for rec in s.chunks_exact(53) {
            if rec[0] == 0 {
                continue;
            }
            v.push((u32_at(rec, 1), BLOCK_LEN));
            let (length, addr) = (u32_at(rec, 41), u32_at(rec, 45));
            if addr != 0 && length > 0 {
                v.push((addr, length as usize));
            }
        }
        v
    }
}

impl Device for ContentDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        self.replay = session.replay();
        self.image_id = session.hello.image_id;
        self.rom = session.hello.rom.clone();
        self.shadow = vec![0; session.hello.ram_len as usize];
        if !self.replay {
            match self.options.start() {
                Ok(s) => self.service = Some(s),
                Err(e) => eprintln!("terse-dev-content: {e}; every request will fail"),
            }
        }
        let submit = CallEntry::plain(0).fixed(0, BLOCK_LEN as u32, access::READ | access::WRITE).results(0, 0);
        DeviceInfo { version: 0, name: "terse-dev-content (Rust)".into(), native: (0, 0), layers: Vec::new(), table: vec![submit] }
    }

    fn reset(&mut self) {
        self.content = Content::default();
        self.completions.clear();
        self.feeds_updated = false;
    }

    fn call(&mut self, c: &CallIn, mem: &mut dyn Mem) -> Reply {
        if c.function != 0 {
            return Reply { a0: ERR_UNKNOWN, a1: None, charge: 0 };
        }
        let block = c.args[0];
        let at = block.wrapping_sub(RAM_BASE) as usize;
        if c.buf.len() != BLOCK_LEN || at + BLOCK_LEN > self.shadow.len() {
            return Reply { a0: 0, a1: None, charge: 0 };
        }
        self.shadow[at..at + BLOCK_LEN].copy_from_slice(c.buf);
        // The data range a submit reads (SPEC 9.1): a put's or save's data,
        // a title, a find's query, a name. Only bytes in RAM need fetching
        // (ROM came with HELLO).
        let b = c.buf;
        let (length, addr, size) = (u32_at(b, LENGTH), u32_at(b, ADDR), u32_at(b, SIZE));
        let data = match b[0] {
            0x02 | 0x0E => Some((addr, size as usize)),
            0x07 | 0x0B | 0x0C => Some((addr, length as usize)),
            0x0A => Some((addr, size as usize)),
            _ => None,
        };
        let ranges: Vec<(u32, usize)> = data.into_iter().filter(|&(_, l)| l <= 1 << 20).collect();
        fetch(&mut self.shadow, mem, &ranges);
        let (handle, taken) = self.content.submit(block, &self.rom, &mut self.shadow);
        if self.shadow[at..at + BLOCK_LEN] != *c.buf {
            mem.write(block, &self.shadow[at..at + BLOCK_LEN]);
        }
        Reply { a0: handle, a1: None, charge: copy_charge(taken) }
    }

    fn begin(&mut self, b: &Begin, mem: &mut dyn Mem) -> Inputs {
        let (mut completions, feeds_updated) = match b.replay {
            Some(r) if !r.is_empty() => decode(r).unwrap_or_default(),
            Some(_) => (Vec::new(), false),
            None => (std::mem::take(&mut self.completions), std::mem::take(&mut self.feeds_updated)),
        };
        let record = if completions.is_empty() && !feeds_updated { Vec::new() } else { encode(&completions, feeds_updated) };
        let mut out = Inputs { record, ..Default::default() };
        if feeds_updated {
            out.events.push(PosEvent { event: Event::feeds_updated(), position: position::AFTER_INPUT, flags: 0, index: 0 });
        }
        let mut all = self.content.take_orphans();
        all.append(&mut completions);
        if all.is_empty() {
            return out;
        }
        let ranges = self.pending_ranges();
        fetch(&mut self.shadow, mem, &ranges);
        let before = snapshot(&self.shadow, &ranges);
        let mut sys = System::default();
        let (_, _, delivered) = self.content.complete(&all, &mut self.shadow, &mut sys);
        write_back(&self.shadow, &before, mem, &ranges);
        out.charge = copy_charge(delivered);
        for &e in sys.queue() {
            out.events.push(PosEvent { event: e, position: position::COMPLETION, flags: 0, index: 0 });
        }
        out
    }

    fn end(&mut self, _: u32, _: bool) {
        let requests = self.content.take_requests();
        if let Some(service) = &mut self.service {
            service.submit_owned(0, Owner::Image(self.image_id), requests);
            self.completions = service.wait_finished();
            self.feeds_updated = service.feeds_updated();
        }
    }

    fn state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.content.write_state(&mut out);
        out
    }

    fn restore(&mut self, bytes: &[u8]) -> bool {
        let mut c = Content::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        if c.restore(&mut r, self.shadow.len()).is_err() || r.remaining() != 0 {
            return false;
        }
        let mut again = Vec::new();
        c.write_state(&mut again);
        if again != bytes {
            return false;
        }
        self.content = c;
        true
    }
}

fn main() -> std::process::ExitCode {
    let mut options = ContentOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or(format!("{a} needs a value"));
        match options.take(&a, &mut value) {
            Ok(true) => {}
            Ok(false) => {
                eprintln!("terse-dev-content: unknown option {a}");
                return std::process::ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("terse-dev-content: {e}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    let mut d = ContentDriver {
        options,
        content: Content::default(),
        service: None,
        completions: Vec::new(),
        feeds_updated: false,
        image_id: [0; 32],
        rom: Vec::new(),
        shadow: Vec::new(),
        replay: false,
    };
    terse_bus::driver::run(&mut d, "terse-dev-content")
}
