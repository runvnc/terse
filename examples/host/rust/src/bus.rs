use crate::cpu::Cpu;
use crate::image::Device;
use std::collections::HashMap;
use std::fmt::Display;
use std::io::{Read, Write, stderr};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, exit};
use std::{env, fs};

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
            .filter(|&slot| slot == 2 || slot == 10)
            .collect();
        let mut hello = vec![0x01, 1, 0, 0, 0, 0, 0, 0];
        hello.push(ask.len() as u8);
        hello.extend(&ask);
        hello.extend((session.len() as u16).to_le_bytes());
        hello.extend(session.as_bytes());
        hello.extend([0, 0]);
        let reply = bus.request(0, 0x81, &hello);
        let name_len = u16::from_le_bytes([reply[4], reply[5]]);
        let mut at = 10 + name_len as usize;
        let mut versions = HashMap::new();
        for _ in 0..reply[at] {
            let slot = reply[at + 1];
            versions.insert(slot, reply[at + 2]);
            at += 4 + 2 * reply[at + 3] as usize;
            for _ in 0..reply[at] {
                let entry = reply[at + 1..at + 65].to_vec();
                let key = (slot as u32, entry[0] as u32);
                bus.tables.insert(key, entry);
                at += 64;
            }
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
        let mut open = vec![0x03, 0, 0, 0];
        open.extend(blake3::hash(file).as_bytes());
        open.extend((cpu.rom.len() as u32).to_le_bytes());
        open.extend((cpu.ram.len() as u32).to_le_bytes());
        open.extend([0, 0, has.len() as u8 / 2]);
        open.extend(&has);
        open.push(0);
        open.extend(&cpu.rom);
        if bus.request(0, 0x83, &open)[1] != 1 {
            fail("the driver will not run the program");
        }
        bus
    }

    fn send(&mut self, slot: u8, body: &[u8]) {
        let length = 8 + body.len() as u32;
        let mut message = length.to_le_bytes().to_vec();
        message.extend([0, 0, 0, 0, slot, 0, 0, 0]);
        message.extend(body);
        check(self.stream.write_all(&message));
    }

    fn request(&mut self, slot: u8, want: u8, body: &[u8])
    -> Vec<u8> {
        self.send(slot, body);
        let mut length = [0; 4];
        check(self.stream.read_exact(&mut length));
        let mut reply = vec![0; word(&length, 0) as usize];
        check(self.stream.read_exact(&mut reply));
        if reply.len() < 9 || reply[8] != want {
            let text = String::from_utf8_lossy(&reply[8..]);
            fail(format!("the driver said {text:?}"));
        }
        reply.split_off(8)
    }

    pub fn begin(&mut self, frame: u32) -> i64 {
        let mut begin = vec![0x30, 0, 0, 0];
        begin.extend(frame.to_le_bytes());
        begin.extend([0; 12]);
        word(&self.request(0, 0xb0, &begin), 4) as i64
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
            return (u32::MAX, 4);
        };
        let charge = if entry[1] & 2 != 0 { 4 } else { 64 };
        let args = cpu.x[10..16].to_vec();
        let addr = args[entry[2] as usize];
        let access = entry[16];
        let mut size = 0;
        if entry[1] & 4 != 0 {
            size = word(&entry, 4) as u64;
            if entry[3] != 0xff {
                size *= args[entry[3] as usize] as u64;
            }
            let least = word(&entry, 8) as u64;
            if size < least || size > word(&entry, 12) as u64 {
                return (word(&entry, 24), charge);
            }
            let ram_only = access & 4 == 0 || access & 2 != 0;
            let place = cpu.memory(addr, size as u32, ram_only);
            if place.is_none() && size > 0 {
                return (word(&entry, 28), charge);
            }
        }
        let checked = entry[17] as usize;
        if checked != 0xff && args[checked] > word(&entry, 20) {
            return (word(&entry, 28), charge);
        }

        let mut call = vec![0x10, function as u8, 0, 0];
        for a in &args {
            call.extend(a.to_le_bytes());
        }
        call.extend((budget - 4).to_le_bytes());
        let reads = if access & 1 == 0 { 0 } else { size };
        call.extend((reads as u32).to_le_bytes());
        if reads > 0 {
            let buffer = cpu.memory(addr, reads as u32, false);
            call.extend(&*buffer.unwrap());
        }
        let reply = self.request(slot as u8, 0x90, &call);
        if reply[1] & 1 != 0 {
            cpu.x[11] = word(&reply, 8);
        }
        let mut at = 20;
        for _ in 0..word(&reply, 16) {
            let to = word(&reply, at);
            let length = word(&reply, at + 4);
            let offset = to.wrapping_sub(addr) as u64;
            let outside = offset + length as u64 > size;
            if access & 2 == 0 || outside {
                fail("a write outside the buffer");
            }
            let bytes = &reply[at + 8..][..length as usize];
            let target = cpu.memory(to, length, true).unwrap();
            target.copy_from_slice(bytes);
            at += 8 + length as usize;
        }
        (word(&reply, 4), charge + word(&reply, 12) as i64)
    }

    pub fn end(&mut self, frame: u32, stopped: bool) {
        let mut end = vec![0x31, stopped as u8, 0, 0];
        end.extend(frame.to_le_bytes());
        self.send(0, &end);
    }

    pub fn state(&mut self, slot: u8) -> Vec<u8> {
        self.request(slot, 0xc0, &[0x40]).split_off(8)
    }

    pub fn close(mut self) {
        self.send(0, &[0x60]);
        check(self.driver.wait());
        check(fs::remove_dir_all(&self.dir));
    }
}
