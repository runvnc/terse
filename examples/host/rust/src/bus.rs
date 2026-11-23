use crate::cpu::{A0, A1, A5, Cpu, UNKNOWN_CALL};
use crate::frames::BASE_CHARGE;
use crate::image::{DEVICE_SIZE, Device};
use std::collections::HashMap;
use std::fmt::Display;
use std::io::{Read, Write, stderr};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, exit};
use std::{env, fs};

pub const TERMINAL: u8 = 2;
pub const FULL_TEXT: u8 = 10;

const HELLO: u8 = 0x01;
const OPEN: u8 = 0x03;
const CALL: u8 = 0x10;
const BEGIN: u8 = 0x30;
const END: u8 = 0x31;
const STATE: u8 = 0x40;
const SHUTDOWN: u8 = 0x60;
const REPLY: u8 = 0x80;
const PROTOCOL: u8 = 1;
const ADDRESS_SIZE: usize = 8;
const NO_SLOT: u8 = 0;

const NAME_AT: usize = 4;
const LENGTH_SIZE: usize = 2;
const NATIVE_SIZE: usize = 4;
const LAYER_SIZE: usize = 2;
const TOOK_AT: usize = 1;
const TOOK: u8 = 1;

const ENTRY_SIZE: usize = 64;
const FUNCTION_AT: usize = 0;
const FLAGS_AT: usize = 1;
const BUFFER_AT: usize = 2;
const LENGTH_AT: usize = 3;
const SCALE_AT: usize = 4;
const LEAST_AT: usize = 8;
const MOST_AT: usize = 12;
const ACCESS_AT: usize = 16;
const CHECKED_AT: usize = 17;
const CHECKED_MOST_AT: usize = 20;
const LENGTH_RESULT_AT: usize = 24;
const PLACE_RESULT_AT: usize = 28;

const DEFERRED: u8 = 2;
const BUFFERED: u8 = 4;
const READS: u8 = 1;
const WRITES: u8 = 2;
const IN_ROM: u8 = 4;
const NONE: u8 = 0xff;
const ANSWERED_CHARGE: i64 = 64;

const SETS_A1: u8 = 1;
const A0_AT: usize = 4;
const A1_AT: usize = 8;
const CHARGE_AT: usize = 12;
const WRITE_COUNT_AT: usize = 16;
const WRITES_AT: usize = 20;
const WRITE_LENGTH_AT: usize = 4;
const WRITE_DATA_AT: usize = 8;

const INPUTS_CHARGE_AT: usize = 4;
const SECTION_AT: usize = 8;

pub struct Bus {
    stream: UnixStream,
    driver: Child,
    dir: PathBuf,
    tables: HashMap<(u32, u32), Vec<u8>>,
    pub grants: Vec<u8>,
}

fn fail(error: impl Display) -> ! {
    eprintln!("bus: {error}");
    exit(3)
}

fn check<T, E: Display>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| fail(error))
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn words(bytes: &mut Vec<u8>, values: &[u32]) {
    bytes.extend(values.iter().flat_map(|v| v.to_le_bytes()));
}

impl Bus {
    pub fn start(
        command: &str,
        cpu: &Cpu,
        file: &[u8],
        devices: &[Device],
    ) -> Bus {
        let name = format!("terse-bus-{}", std::process::id());
        let dir = env::temp_dir().join(name);
        check(fs::create_dir_all(&dir));
        let path = dir.join("bus.sock");
        let listener = check(UnixListener::bind(&path));
        let driver = check(
            Command::new(command)
                .env("TERSE_BUS_SOCKET", &path)
                .stdout(stderr())
                .spawn(),
        );
        let (stream, _) = check(listener.accept());
        let session = dir.to_string_lossy().into_owned();
        let (tables, grants) = (HashMap::new(), Vec::new());
        let mut bus =
            Bus { stream, driver, dir, tables, grants };

        let ask: Vec<u8> = devices
            .iter()
            .map(|d| d.slot)
            .filter(|slot| [TERMINAL, FULL_TEXT].contains(slot))
            .collect();
        let mut hello = vec![HELLO, PROTOCOL, 0, 0];
        hello.extend([0, 0, 0, 0, ask.len() as u8]);
        hello.extend(&ask);
        hello.extend((session.len() as u16).to_le_bytes());
        hello.extend(session.as_bytes());
        hello.extend([0, 0]);
        let reply = bus.request(NO_SLOT, &hello);
        let mut rest = &reply[NAME_AT..];
        let mut take = |n: usize| rest.split_off(..n).unwrap();
        let length = take(LENGTH_SIZE);
        let length = u16::from_le_bytes([length[0], length[1]]);
        take(length as usize + NATIVE_SIZE);
        let mut versions = HashMap::new();
        for _ in 0..take(1)[0] {
            let (slot, version) = (take(1)[0], take(1)[0]);
            let layers = take(1)[0] as usize;
            take(LAYER_SIZE * layers);
            for _ in 0..take(1)[0] {
                let entry = take(ENTRY_SIZE).to_vec();
                let function = entry[FUNCTION_AT];
                let key = (slot as u32, function as u32);
                bus.tables.insert(key, entry);
            }
            versions.insert(slot, version);
        }

        let mut has = Vec::new();
        for d in devices {
            let (slot, version) = (d.slot, d.version);
            let granted = versions.get(&slot) == Some(&version);
            if !granted && !d.optional {
                fail(format!("no driver for device {slot}"));
            }
            if d.optional {
                bus.grants.push(granted as u8);
            }
            if granted {
                has.extend([d.slot, d.version]);
            }
        }
        let mut open = vec![OPEN, 0, 0, 0];
        open.extend(blake3::hash(file).as_bytes());
        let (rom, ram) = (cpu.rom.len(), cpu.ram.len());
        words(&mut open, &[rom as u32, ram as u32]);
        open.extend([0, 0, (has.len() / DEVICE_SIZE) as u8]);
        open.extend(&has);
        open.push(0);
        open.extend(&cpu.rom);
        if bus.request(NO_SLOT, &open)[TOOK_AT] != TOOK {
            fail("the driver will not run the program");
        }
        bus
    }

    fn send(&mut self, slot: u8, body: &[u8]) {
        let length = (ADDRESS_SIZE + body.len()) as u32;
        let mut message = length.to_le_bytes().to_vec();
        message.extend([0, 0, 0, 0, slot, 0, 0, 0]);
        message.extend(body);
        check(self.stream.write_all(&message));
    }

    fn request(&mut self, slot: u8, body: &[u8]) -> Vec<u8> {
        self.send(slot, body);
        let mut length = [0; 4];
        check(self.stream.read_exact(&mut length));
        let mut reply = vec![0; word(&length, 0) as usize];
        check(self.stream.read_exact(&mut reply));
        let reply = reply.split_off(ADDRESS_SIZE);
        if reply.first() != Some(&(body[0] | REPLY)) {
            let text = String::from_utf8_lossy(&reply);
            fail(format!("the driver said {text:?}"));
        }
        reply
    }

    pub fn call(
        &mut self,
        cpu: &mut Cpu,
        budget: i64,
        slot: u32,
        function: u32,
    ) -> (u32, i64) {
        let key = (slot, function);
        let Some(entry) = self.tables.get(&key).cloned() else {
            return (UNKNOWN_CALL, BASE_CHARGE);
        };
        let field = |at| word(&entry, at);
        let flags = entry[FLAGS_AT];
        let access = entry[ACCESS_AT];
        let mut charge = ANSWERED_CHARGE;
        if flags & DEFERRED != 0 {
            charge = BASE_CHARGE;
        }
        let args = cpu.x[A0..=A5].to_vec();
        let addr = args[entry[BUFFER_AT] as usize];
        let mut size = 0;
        if flags & BUFFERED != 0 {
            size = field(SCALE_AT) as u64;
            if entry[LENGTH_AT] != NONE {
                size *= args[entry[LENGTH_AT] as usize] as u64;
            }
            let least = field(LEAST_AT) as u64;
            if size < least || size > field(MOST_AT) as u64 {
                return (field(LENGTH_RESULT_AT), charge);
            }
            let ram_only =
                access & IN_ROM == 0 || access & WRITES != 0;
            let place = cpu.memory(addr, size as u32, ram_only);
            if place.is_none() && size > 0 {
                return (field(PLACE_RESULT_AT), charge);
            }
        }
        let checked = entry[CHECKED_AT];
        let most = field(CHECKED_MOST_AT);
        if checked != NONE && args[checked as usize] > most {
            return (field(PLACE_RESULT_AT), charge);
        }

        let mut call = vec![CALL, function as u8, 0, 0];
        words(&mut call, &args);
        call.extend((budget - BASE_CHARGE).to_le_bytes());
        let reads = if access & READS == 0 { 0 } else { size };
        words(&mut call, &[reads as u32]);
        if reads > 0 {
            let buffer = cpu.memory(addr, reads as u32, false);
            call.extend(&*buffer.unwrap());
        }
        let reply = self.request(slot as u8, &call);
        if reply[FLAGS_AT] & SETS_A1 != 0 {
            cpu.x[A1] = word(&reply, A1_AT);
        }
        let mut at = WRITES_AT;
        for _ in 0..word(&reply, WRITE_COUNT_AT) {
            let write = &reply[at..];
            let to = word(write, 0);
            let length = word(write, WRITE_LENGTH_AT);
            let offset = to.wrapping_sub(addr) as u64;
            let outside = offset + length as u64 > size;
            if access & WRITES == 0 || outside {
                fail("a write outside the buffer");
            }
            let data = &write[WRITE_DATA_AT..];
            let target = cpu.memory(to, length, true).unwrap();
            target.copy_from_slice(&data[..target.len()]);
            at += WRITE_DATA_AT + length as usize;
        }
        let extra = word(&reply, CHARGE_AT) as i64;
        (word(&reply, A0_AT), charge + extra)
    }

    pub fn begin(&mut self, frame: u32) -> i64 {
        let mut begin = vec![BEGIN, 0, 0, 0];
        words(&mut begin, &[frame, 0, 0, 0]);
        let reply = self.request(NO_SLOT, &begin);
        word(&reply, INPUTS_CHARGE_AT) as i64
    }

    pub fn end(&mut self, frame: u32, stopped: bool) {
        let mut end = vec![END, stopped as u8, 0, 0];
        words(&mut end, &[frame]);
        self.send(NO_SLOT, &end);
    }

    pub fn state(&mut self, slot: u8) -> Vec<u8> {
        self.request(slot, &[STATE]).split_off(SECTION_AT)
    }

    pub fn close(mut self) {
        self.send(NO_SLOT, &[SHUTDOWN]);
        check(self.driver.wait());
        check(fs::remove_dir_all(&self.dir));
    }
}
