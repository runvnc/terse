//! `terse-dev-content`: the content device (SPEC 9, slot 4) as a driver
//! (DEVICES.md): per machine, terse-core's `Content` for the state; one
//! terse-net content service for every machine (one node, one store), run
//! as `terse run` and terse-host run it (every request completed at the
//! start of the machine's next frame; a machine's named records are its
//! identity's, else its image's). It is also the host's services for the
//! core (DEVICES.md 9: images and programs to start, snapshots), and
//! publishes the consent question it waits on (DEVICES.md 7.2) for the
//! core to show and answer. Options: those of `terse run` for content
//! (`--store`, `--config`, `--data`, `--relay`, `--consent allow|deny|ask`,
//! `--profile reading`, `--no-feed-relays`, `--no-program-download`).
//!
//! The device reads and writes guest memory it has no buffer for (a put's
//! data, a completion's delivered bytes): it keeps a shadow of RAM, fills
//! the parts it needs with the core's memory (READ, or the memory file),
//! lets terse-core's `Content` work on the shadow, and writes back what it
//! touched.

use std::collections::HashMap;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, Mem, Reply, Session};
use terse_bus::inproc::ContentOptions;
use terse_bus::proto::{CallEntry, DeviceInfo, Open, PosEvent, SlotInfo, access, position, property, service, u32_at};
use terse_bus::services::{self, fetched};
use terse_core::content::{ADDR, BLOCK_LEN, Content, LENGTH, SIZE};
use terse_core::machine::copy_charge;
use terse_core::system::System;
use terse_core::{Completion, ERR_UNKNOWN, Event, Outcome, RAM_BASE, Snapshot};
use terse_net::{ContentService, Owner, Snapshots};

/// One machine's content device.
struct Inst {
    content: Content,
    owner: Owner,
    rom: Vec<u8>,
    shadow: Vec<u8>,
    /// Completions for its next frame.
    completions: Vec<Completion>,
    feeds_updated: bool,
    /// It has begun a frame (a machine that has not is not told of feeds
    /// updated before it started, as terse-net's headless hosts do).
    begun: bool,
}

struct ContentDriver {
    options: ContentOptions,
    service: Option<ContentService>,
    replay: bool,
    inst: HashMap<u32, Inst>,
    /// Snapshot results the service gave while waiting for something else.
    saved: Vec<(u8, bool)>,
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

impl Inst {
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

impl ContentDriver {
    /// Waits for everything submitted, giving each machine its
    /// completions; returns the image fetches' answers (by tag) among them.
    fn wait(&mut self) -> Vec<(u32, Completion)> {
        let Some(service) = &mut self.service else { return Vec::new() };
        let mut fetches = Vec::new();
        for (tag, c) in service.wait_finished_tagged() {
            match self.inst.get_mut(&tag) {
                Some(i) if c.handle != 0 => i.completions.push(c),
                _ => fetches.push((tag, c)),
            }
        }
        let (saved, restored) = service.take_snapshot_results();
        self.saved.extend(saved);
        let _ = restored;
        fetches
    }

    fn owner(&self, instance: u32) -> Result<Owner, String> {
        self.inst.get(&instance).map(|i| i.owner.clone()).ok_or_else(|| format!("no instance {instance}"))
    }

    /// An image fetch's answer for `tag`.
    fn fetched(&mut self, tag: u32) -> Vec<u8> {
        let got = self.wait().into_iter().find(|(t, c)| *t == tag && c.handle == 0).map(|(_, c)| c.outcome);
        match got {
            Some(Outcome::Got { data, .. }) => [&[fetched::GOT][..], &data].concat(),
            Some(Outcome::NotFound) => vec![fetched::NOT_FOUND],
            _ => vec![fetched::FAILED],
        }
    }
}

impl Device for ContentDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        self.replay = session.replay();
        if !self.replay {
            match self.options.start() {
                Ok(s) => self.service = Some(s),
                Err(e) => eprintln!("terse-dev-content: {e}; every request will fail"),
            }
        }
        let submit = CallEntry::plain(0).fixed(0, BLOCK_LEN as u32, access::READ | access::WRITE).results(0, 0);
        let services = [service::FETCH_IMAGE, service::FETCH_PROGRAM, service::SNAPSHOT_SAVE, service::SNAPSHOT_LOAD, service::SNAPSHOT_DELETE, service::SNAPSHOT_TABLE];
        DeviceInfo {
            name: "terse-dev-content (Rust)".into(),
            native: (0, 0),
            slots: vec![SlotInfo { slot: 4, version: 0, layers: Vec::new(), table: vec![submit] }],
            publishes: vec![property::PROMPT.into()],
            subscribes: vec![property::ANSWER.into()],
            services: services.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn open(&mut self, instance: u32, open: &Open, _: &Session) -> Result<(), String> {
        let owner = match &open.identity {
            Some((publisher, name)) => Owner::Program { publisher: *publisher, name: name.clone() },
            None => Owner::Image(open.image_id),
        };
        let inst = Inst {
            content: Content::default(),
            owner,
            rom: open.rom.clone(),
            shadow: vec![0; open.ram_len as usize],
            completions: Vec::new(),
            feeds_updated: false,
            begun: false,
        };
        self.inst.insert(instance, inst);
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.inst.remove(&instance);
        if let Some(s) = &mut self.service {
            s.cancel(instance);
        }
    }

    fn reset(&mut self, instance: u32) {
        if let Some(i) = self.inst.get_mut(&instance) {
            i.content = Content::default();
            i.completions.clear();
            i.feeds_updated = false;
        }
    }

    fn call(&mut self, instance: u32, c: &CallIn, mem: &mut dyn Mem) -> Reply {
        let Some(i) = self.inst.get_mut(&instance) else { return Reply { a0: ERR_UNKNOWN, ..Default::default() } };
        if c.function != 0 {
            return Reply { a0: ERR_UNKNOWN, a1: None, charge: 0 };
        }
        let block = c.args[0];
        let at = block.wrapping_sub(RAM_BASE) as usize;
        if c.buf.len() != BLOCK_LEN || at + BLOCK_LEN > i.shadow.len() {
            return Reply { a0: 0, a1: None, charge: 0 };
        }
        i.shadow[at..at + BLOCK_LEN].copy_from_slice(c.buf);
        // The data range a submit reads (SPEC 9.1): a put's or save's data,
        // a title, a find's query, a name. Only bytes in RAM need fetching
        // (ROM came with OPEN).
        let b = c.buf;
        let (length, addr, size) = (u32_at(b, LENGTH), u32_at(b, ADDR), u32_at(b, SIZE));
        let data = match b[0] {
            0x02 | 0x0E => Some((addr, size as usize)),
            0x07 | 0x0B | 0x0C => Some((addr, length as usize)),
            0x0A => Some((addr, size as usize)),
            _ => None,
        };
        let ranges: Vec<(u32, usize)> = data.into_iter().filter(|&(_, l)| l <= 1 << 20).collect();
        fetch(&mut i.shadow, mem, &ranges);
        let (handle, taken) = i.content.submit(block, &i.rom, &mut i.shadow);
        if i.shadow[at..at + BLOCK_LEN] != *c.buf {
            mem.write(block, &i.shadow[at..at + BLOCK_LEN]);
        }
        Reply { a0: handle, a1: None, charge: copy_charge(taken) }
    }

    fn begin(&mut self, instance: u32, b: &Begin, mem: &mut dyn Mem) -> Inputs {
        // A system frame starts with instance 0's (the shell's, or the
        // single machine's): feeds updated since the last one are
        // announced to every machine that ran in it, in this frame (SPEC
        // 5), as terse-net's headless hosts do after each system frame.
        // Before any machine has run, nothing is taken.
        if (instance == 0 || !self.inst.contains_key(&0))
            && self.inst.values().any(|i| i.begun)
            && let Some(s) = &mut self.service
            && s.feeds_updated()
        {
            for i in self.inst.values_mut().filter(|i| i.begun) {
                i.feeds_updated = true;
            }
        }
        let Some(i) = self.inst.get_mut(&instance) else { return Inputs::default() };
        i.begun = true;
        let (mut completions, feeds_updated) = match b.replay {
            Some(r) if !r.is_empty() => decode(r).unwrap_or_default(),
            Some(_) => (Vec::new(), false),
            None => (std::mem::take(&mut i.completions), std::mem::take(&mut i.feeds_updated)),
        };
        let record = if completions.is_empty() && !feeds_updated { Vec::new() } else { encode(&completions, feeds_updated) };
        let mut out = Inputs { record, ..Default::default() };
        if feeds_updated {
            out.events.push(PosEvent { event: Event::feeds_updated(), position: position::AFTER_INPUT, flags: 0, index: 0 });
        }
        let mut all = i.content.take_orphans();
        all.append(&mut completions);
        if all.is_empty() {
            return out;
        }
        let ranges = i.pending_ranges();
        fetch(&mut i.shadow, mem, &ranges);
        let before = snapshot(&i.shadow, &ranges);
        let mut sys = System::default();
        let (_, _, delivered) = i.content.complete(&all, &mut i.shadow, &mut sys);
        write_back(&i.shadow, &before, mem, &ranges);
        out.charge = copy_charge(delivered);
        for &e in sys.queue() {
            out.events.push(PosEvent { event: e, position: position::COMPLETION, flags: 0, index: 0 });
        }
        out
    }

    fn end(&mut self, instance: u32, _: u32, _: bool) {
        let Some(i) = self.inst.get_mut(&instance) else { return };
        let requests = i.content.take_requests();
        let owner = i.owner.clone();
        if let Some(service) = &mut self.service {
            service.submit_owned(instance, owner, requests);
            // (Anything else that finished is a closed machine's.)
            self.wait();
        }
    }

    fn state(&self, instance: u32, _: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(i) = self.inst.get(&instance) {
            i.content.write_state(&mut out);
        }
        out
    }

    fn restore(&mut self, instance: u32, _: u8, bytes: &[u8]) -> Option<usize> {
        let i = self.inst.get_mut(&instance)?;
        let mut c = Content::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        c.restore(&mut r, i.shadow.len()).ok()?;
        let n = r.position();
        let mut again = Vec::new();
        c.write_state(&mut again);
        (again == bytes[..n]).then_some(())?;
        i.content = c;
        Some(n)
    }

    fn get(&mut self, _: u32, name: &str) -> Option<Vec<u8>> {
        (name == property::PROMPT).then(|| services::prompt(self.service.as_ref().and_then(|s| Some((s.prompt()?, s.prompts_waiting())))))
    }

    fn set(&mut self, _: u32, name: &str, value: &[u8]) {
        if name == property::ANSWER
            && let Some((tag, handle, allow)) = services::read_answer(value)
            && let Some(s) = &mut self.service
            && s.prompt().is_some_and(|p| p.tag == tag && p.handle == handle)
        {
            s.answer(allow);
        }
    }

    fn service(&mut self, instance: u32, name: &str, request: &[u8]) -> Result<Vec<u8>, String> {
        if self.service.is_none() {
            return Err("no content service (replay, or it could not start)".into());
        }
        match name {
            service::FETCH_IMAGE => {
                let (tag, id) = services::read_fetch_image(request)?;
                self.service.as_mut().unwrap().fetch(tag, id);
                Ok(self.fetched(tag))
            }
            service::FETCH_PROGRAM => {
                let (tag, program, builtins) = services::read_fetch_program(request)?;
                self.service.as_mut().unwrap().fetch_program(tag, &program, &builtins);
                Ok(self.fetched(tag))
            }
            service::SNAPSHOT_SAVE => {
                let slot = *request.first().ok_or("no slot")?;
                let snapshot = Snapshot::decode(&request[1..]).map_err(|e| e.to_string())?;
                let owner = self.owner(instance)?;
                self.service.as_mut().unwrap().save_snapshot(owner, slot, &snapshot);
                self.wait();
                let stored = self.saved.iter().rposition(|s| s.0 == slot).map(|k| self.saved.remove(k).1).unwrap_or(false);
                Ok(vec![stored as u8])
            }
            service::SNAPSHOT_LOAD => {
                let mut r = terse_bus::proto::Reader::new(request);
                let (tag, slot) = (r.u32()?, r.u8()?);
                let owner = self.owner(instance)?;
                let service = self.service.as_mut().unwrap();
                service.restore_snapshot(tag, owner, slot);
                let mut done = Vec::new();
                for (t, c) in service.wait_finished_tagged() {
                    done.push((t, c));
                }
                let (saved, restored) = service.take_snapshot_results();
                self.saved.extend(saved);
                for (t, c) in done {
                    if let Some(i) = self.inst.get_mut(&t) {
                        i.completions.push(c);
                    }
                }
                let result = restored.into_iter().find(|r| r.0 == tag).map(|r| r.1).ok_or("no answer")?;
                Ok(services::restored(&result))
            }
            service::SNAPSHOT_DELETE => {
                let slot = *request.first().ok_or("no slot")?;
                let owner = self.owner(instance)?;
                self.service.as_mut().unwrap().delete_snapshot(owner, slot);
                self.wait();
                Ok(Vec::new())
            }
            service::SNAPSHOT_TABLE => {
                let owner = self.owner(instance)?;
                let data = self.options.data.clone().or_else(terse_net::default_data_dir).ok_or("no data directory")?;
                Ok(services::table(&Snapshots::in_data_dir(&data).table(&owner)))
            }
            _ => Err(format!("no service {name}")),
        }
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
    let mut d = ContentDriver { options, service: None, replay: false, inst: HashMap::new(), saved: Vec::new() };
    terse_bus::driver::run(&mut d, "terse-dev-content")
}
