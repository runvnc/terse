//! Device 8 (SPEC 12): the calls, by hand in assembly (skipped without the
//! RISC-V tools); and the system with the checked-in shell (programs/shell)
//! and hand-made machines: frame order, routing, latching, a machine that
//! cannot use device 8.

mod common;

use std::collections::HashMap;

use common::{assemble, program};
use terse_core::machines::state;
use terse_core::system::CONTROLLERS;
use terse_core::{
    Controller, DeviceReq, Driver, Event, FrameInput, FrameResult, Image, Machine, ProgramId, RAM_BASE, StartFailure,
    System, Who, buttons,
};

const SHELL: &[u8] = include_bytes!("../../../programs/shell/shell.terse");
const HELLO: &[u8] = include_bytes!("../../../programs/hello/hello.terse");
const MACHINES: DeviceReq = DeviceReq { slot: 8, version: 0 };

fn words(code: &[u32]) -> Vec<u8> {
    code.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// A machine that lists device 8, calls `builtin` (806h) with a buffer at
/// the start of RAM, stores the result at RAM + 32 and waits forever.
fn probe() -> Image {
    let code = [
        0x0000_18B7, // lui a7, 1
        0x8068_8893, // addi a7, a7, -2042: 806h
        0x0000_0513, // li a0, 0
        0x0010_05B7, // lui a1, 0x100: RAM
        0x0000_0073, // ecall
        0x02A5_A023, // sw a0, 32(a1)
        0x1000_0893, // li a7, 0x100
        0x0000_0073, // ecall
        0xFF9F_F06F, // j -8
    ];
    program(&words(&code), 1, &[MACHINES])
}

/// A machine that faults at once (a load from address 0).
fn crash() -> Image {
    program(&words(&[0x0000_2503]), 1, &[])
}

/// Records what the system gives each machine.
#[derive(Default)]
struct Rec {
    /// The images that can be had, by ID.
    images: HashMap<[u8; 32], Image>,
    /// The built-in programs and published ones, for `want_program`.
    programs: Vec<(ProgramId, Image)>,
    wanted: Vec<(u32, [u8; 32])>,
    removed: Vec<u32>,
    /// (system frame, who, events) of every machine frame run.
    log: Vec<(u32, Who, Vec<Event>)>,
    frame: u32,
}

impl Driver for Rec {
    fn frame(&mut self, who: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        self.log.push((self.frame, who, events.clone()));
        machine.run_frame(&FrameInput { events: &events, controllers, ..Default::default() })
    }

    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        self.wanted.push((serial, image));
    }

    /// The program's image ID, if it is one this host knows (a built-in
    /// by title among `builtins`), else an ID that cannot be had.
    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]) {
        let id = self
            .programs
            .iter()
            .find(|(p, i)| p == program && (!p.is_builtin() || builtins.contains(&i.id)))
            .map_or([0xEE; 32], |(_, i)| i.id);
        self.wanted.push((serial, id));
    }

    fn removed(&mut self, serial: u32) {
        self.removed.push(serial);
    }
}

struct Run {
    system: System,
    rec: Rec,
}

impl Run {
    /// The checked-in shell with these programs as its built-ins.
    fn new(programs: &[Image]) -> Self {
        let mut rec = Rec::default();
        for p in programs {
            rec.images.insert(p.id, p.clone());
            rec.programs.push((ProgramId { publisher: [0; 32], name: p.title.clone() }, p.clone()));
        }
        let ids = programs.iter().map(|p| p.id).collect();
        Run { system: System::new(&Image::parse(SHELL).unwrap(), true, ids), rec }
    }

    /// One frame with `events`; images asked for in the previous frame
    /// are delivered first (an unknown ID fails).
    fn frame(&mut self, events: &[Event]) {
        for (serial, id) in std::mem::take(&mut self.rec.wanted) {
            let image = self.rec.images.get(&id).cloned().ok_or(StartFailure::not_found("not found"));
            self.system.deliver_image(serial, image);
        }
        self.rec.frame = self.system.shell().frame();
        self.system.frame(events, [Controller::default(); CONTROLLERS], &mut self.rec);
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame(&[]);
        }
    }

    fn press(&mut self, key: u8) {
        self.frame(&[Event::key_down(key, 0), Event::key_up(key, 0)]);
    }

    /// The events machine `handle` got in the last frame (None if it ran none).
    fn got(&self, handle: u8) -> Option<&Vec<Event>> {
        let frame = self.system.shell().frame() - 1;
        self.rec.log.iter().find(|(f, w, _)| *f == frame && w.handle == handle).map(|(_, _, e)| e)
    }
}

const ENTER: u8 = 0x28;

#[test]
fn start_load_route_and_order() {
    let hello = Image::parse(HELLO).unwrap();
    let mut r = Run::new(&[hello.clone(), probe()]);
    r.frames(2);
    r.press(ENTER); // the shell starts hello: handle 1
    assert_eq!(r.system.state(1), state::LOADING);
    assert_eq!(r.rec.wanted, [(1, hello.id)]);
    assert!(r.got(1).is_none(), "a loading machine runs no frame");
    // The next frame: loaded at its start, the shell told first, then hello
    // runs its first frame after the shell.
    r.frame(&[Event::text('a')]);
    assert_eq!(r.system.state(1), state::RUNNING);
    assert_eq!(r.got(0).unwrap()[0], Event::machine_changed(1, state::RUNNING));
    let frame = r.system.shell().frame() - 1;
    let order: Vec<u8> = r.rec.log.iter().filter(|(f, ..)| *f == frame).map(|(_, w, _)| w.handle).collect();
    assert_eq!(order, [0, 1]);
    // Typed text goes to the focused machine only (the shell focused hello
    // when it started it; the routing of this frame used that focus).
    assert_eq!(r.system.focus(), 1);
    assert_eq!(r.got(1).unwrap(), &[Event::text('a')]);
    assert!(!r.got(0).unwrap().contains(&Event::text('a')));
    // The home key: the focus is the shell's, and only the shell is told.
    r.frame(&[Event::home(), Event::text('b')]);
    assert_eq!(r.system.focus(), 0);
    assert_eq!(r.got(0).unwrap(), &[Event::home(), Event::text('b')]);
    assert!(r.got(1).unwrap().is_empty());
    assert!(r.rec.log.iter().filter(|(_, w, _)| w.handle != 0).all(|(_, _, e)| e.iter().all(|e| e.kind != Event::HOME)));
}

#[test]
fn pointer_is_mapped_into_the_picture() {
    let hello = Image::parse(HELLO).unwrap();
    let mut r = Run::new(&[hello]);
    r.frames(2);
    r.press(ENTER);
    r.frames(2);
    // One machine: at (17, 23), 605 x 454; the picture 605 x 453.
    let rect = r.system.shell().machines().unwrap().slot(1).rect;
    assert_eq!((rect.x, rect.y, rect.w, rect.h), (17, 23, 605, 454));
    r.frame(&[Event::pointer(17 + 302, 23 + 226, buttons::PRIMARY), Event::pointer(5, 5, 0)]);
    assert_eq!(r.got(1).unwrap(), &[Event::pointer(320, 240, buttons::PRIMARY)]);
    let shell = r.got(0).unwrap();
    assert_eq!(shell, &[Event::machine_pressed(1, buttons::PRIMARY), Event::pointer(5, 5, 0)]);
    // The corners of the picture; just outside it goes to the shell.
    r.frame(&[Event::pointer(17, 23, 0), Event::pointer(621, 475, 0), Event::pointer(622, 300, 0)]);
    assert_eq!(r.got(1).unwrap(), &[Event::pointer(0, 0, 0), Event::pointer(639, 479, 0)]);
    assert_eq!(r.got(0).unwrap(), &[Event::pointer(622, 300, 0)]);
}

#[test]
fn a_started_machine_cannot_use_device_8() {
    let mut r = Run::new(&[probe()]);
    r.frames(2);
    r.press(ENTER);
    r.frames(2);
    let m = r.system.child(1).unwrap().machine.as_ref().unwrap();
    assert!(m.machines().is_none());
    assert_eq!(&m.ram()[32..36], &[0xFF; 4], "builtin returned FFFF_FFFFh");
    assert_eq!(&m.ram()[..32], &[0; 32], "and wrote nothing");
    // The shell itself has it.
    assert_eq!(r.system.shell().machines().unwrap().builtins(), &[probe().id]);
}

#[test]
fn fault_and_failure_are_latched() {
    let mut r = Run::new(&[crash()]);
    r.rec.images.clear(); // the crash image cannot be had at first
    r.frames(2);
    r.press(ENTER);
    r.frame(&[]);
    assert_eq!(r.system.state(1), state::FAILED);
    assert_eq!(r.got(0).unwrap()[0], Event::machine_changed(1, state::FAILED));
    assert_eq!(r.system.focus(), 0, "a failed machine loses the focus");
    // X stops it; Enter starts it again, now that the image can be had.
    r.rec.images.insert(crash().id, crash());
    r.press(0x1B);
    assert_eq!(r.system.state(1), state::NONE);
    assert_eq!(r.rec.removed, [1]);
    r.press(0x52); // up, to the program
    r.press(ENTER);
    r.frame(&[]); // loaded; runs and faults in its first frame
    assert_eq!(r.system.state(1), state::RUNNING);
    r.frame(&[]);
    assert_eq!(r.system.state(1), state::FAULTED);
    assert_eq!(r.got(0).unwrap()[0], Event::machine_changed(1, state::FAULTED));
    let slot = r.system.shell().machines().unwrap().slot(1).clone();
    assert_eq!((slot.fault_kind, slot.fault_pc, slot.frames), (4, 0x0001_0000, 1));
    assert_eq!(r.system.child(1).unwrap().serial, 2, "a new machine under the same handle");
}

#[test]
fn eight_machines_at_most() {
    let hello = Image::parse(HELLO).unwrap();
    let mut r = Run::new(&[hello]);
    r.frames(2);
    for _ in 0..9 {
        r.press(ENTER);
        r.frame(&[Event::home()]);
        // The cursor is on the focused machine: up to the program.
        for _ in 0..8 {
            r.press(0x52);
        }
    }
    assert!((1..=8).all(|h| r.system.state(h) == state::RUNNING));
    assert_eq!(r.system.children().count(), 8, "the ninth start found no free handle");
    assert!(r.rec.removed.is_empty());
}

/// Device 8's calls, made by a shell written by hand.
#[test]
fn calls() {
    if !common::have_tools() {
        return;
    }
    let src = "
    li s0, 0x100000
    li a7, 0x806
    li a0, 0
    addi a1, s0, 256
    ecall
    sw a0, 0(s0)        # builtin 0: 1
    li a0, 2
    ecall
    sw a0, 4(s0)        # builtin 2 (past the end): 0
    li a0, 0
    li a1, 0
    ecall
    sw a0, 8(s0)        # builtin into ROM-less address 0: FFFFFFFE
    addi s1, s0, 12
    li s2, 9
1:  addi a0, s0, 256
    li a7, 0x800
    ecall
    sw a0, 0(s1)        # start x 9: 1 ... 8, 0
    addi s1, s1, 4
    addi s2, s2, -1
    bnez s2, 1b
    li a0, 0
    ecall
    sw a0, 48(s0)       # start with a bad buffer: FFFFFFFE
    li a7, 0x801
    li a0, 3
    ecall
    sw a0, 52(s0)       # stop 3: 1
    li a0, 3
    ecall
    sw a0, 56(s0)       # stop 3 again: 0
    li a0, 9
    ecall
    sw a0, 60(s0)       # stop 9: 0
    li a7, 0x802
    li a0, 1
    li a1, 600
    li a2, 400
    li a3, 100
    li a4, 100
    ecall
    sw a0, 64(s0)       # place 1: 1, clamped
    li a0, 3
    ecall
    sw a0, 68(s0)       # place 3 (stopped): 0
    li a7, 0x803
    li a0, 1
    li a1, 0
    ecall
    sw a0, 72(s0)       # run 1 0: 1 (it will be paused when loaded)
    li a0, 1
    li a1, 2
    ecall
    sw a0, 76(s0)       # run 1 2: 0
    li a7, 0x804
    li a0, 2
    ecall
    sw a0, 80(s0)       # focus 2: 1
    li a0, 3
    ecall
    sw a0, 84(s0)       # focus 3 (stopped): 0
    li a7, 0x805
    li a0, 1
    addi a1, s0, 128
    ecall
    sw a0, 88(s0)       # info 1: 1, the record at +128
    li a0, 9
    ecall
    sw a0, 92(s0)       # info 9: 0
    li a0, 1
    li a1, 0
    ecall
    sw a0, 96(s0)       # info into address 0: FFFFFFFE
    li a7, 0x80F
    ecall
    sw a0, 100(s0)      # 80Fh: FFFFFFFF
2:  li a7, 0x100
    ecall
    j 2b
";
    let rom = assemble(src);
    let image = program(&rom, 1, &[MACHINES]);
    let builtins = vec![[1; 32], [2; 32]];
    let mut m = Machine::new_shell(&image, builtins.clone());
    m.run_frame(&FrameInput::default());
    assert!(m.fault().is_none(), "{:?}", m.fault());
    let ram = m.ram();
    let word = |at: usize| u32::from_le_bytes(ram[at..at + 4].try_into().unwrap());
    assert_eq!((word(0), word(4), word(8)), (1, 0, 0xFFFF_FFFE));
    assert_eq!(&ram[256..288], &[1; 32]);
    let starts: Vec<u32> = (0..9).map(|i| word(12 + 4 * i)).collect();
    assert_eq!(starts, [1, 2, 3, 4, 5, 6, 7, 8, 0]);
    assert_eq!(word(48), 0xFFFF_FFFE);
    assert_eq!((word(52), word(56), word(60)), (1, 0, 0));
    assert_eq!((word(64), word(68)), (1, 0));
    assert_eq!((word(72), word(76)), (1, 0));
    assert_eq!((word(80), word(84)), (1, 0));
    assert_eq!((word(88), word(92), word(96), word(100)), (1, 0, 0xFFFF_FFFE, 0xFFFF_FFFF));
    // The record: still loading (the run flag cleared), the image, the
    // rectangle as clamped.
    let dev = m.machines().unwrap();
    assert_eq!(&ram[128..132], &[state::LOADING, 0, 0, 0]);
    assert!(!dev.slot(1).run);
    assert_eq!(&ram[144..176], &[1; 32]);
    let r = dev.slot(1).rect;
    assert_eq!((r.x, r.y, r.w, r.h), (600, 400, 40, 80));
    assert_eq!(dev.focus(), 2);
    assert_eq!(dev.slot(3).state, state::NONE);
    // The starts and the stop, in order, for the host.
    let mut m2 = m;
    let commands = m2.machines_mut().unwrap().take_commands();
    assert_eq!(commands.len(), 9);

    // The same image as a plain machine: no device 8.
    let mut plain = Machine::new(&image);
    plain.run_frame(&FrameInput::default());
    let ram = plain.ram();
    let word = |at: usize| u32::from_le_bytes(ram[at..at + 4].try_into().unwrap());
    assert!([0, 4, 12, 52, 64, 72, 80, 88, 100].iter().all(|&at| word(at) == 0xFFFF_FFFF));
    // The state section is there (with its flag) only for an image that
    // lists device 8, and differs with the grant.
    let granted = Machine::new_shell(&image, builtins).state();
    let absent = Machine::new(&image).state();
    assert_eq!(granted.len(), absent.len() + 1 + 8 * (57 + 81) + 1 + 2 * 32 + 1, "no request waits");
    assert_eq!(absent.last(), Some(&0));
}

/// Charges (SPEC 13): 4 per call, plus 1 per 4 bytes copied.
#[test]
fn charges() {
    if !common::have_tools() {
        return;
    }
    let used = |call: u32, a1: &str| {
        let src = format!(
            "    li s0, 0x100000\n    li a7, {call}\n    li a0, 1\n    {a1}\n    ecall\n1:  li a7, 0x100\n    ecall\n    j 1b\n"
        );
        let mut m = Machine::new_shell(&program(&assemble(&src), 1, &[MACHINES]), vec![[1; 32]; 2]);
        m.run_frame(&FrameInput::default()).used
    };
    let base = used(0x8FF, "addi a1, s0, 0");
    assert_eq!(used(0x805, "addi a1, s0, 0") - base, 12, "info copies 48 bytes");
    assert_eq!(used(0x806, "addi a1, s0, 0") - base, 8, "builtin copies 32");
    assert_eq!(used(0x803, "addi a1, s0, 0") - base, 0, "run copies nothing");
    let _ = RAM_BASE;
}

/// Records the programs the system asks for, delivers nothing by itself.
#[derive(Default)]
struct Programs {
    asked: Vec<(u32, ProgramId, Vec<[u8; 32]>)>,
}

impl Driver for Programs {
    fn frame(&mut self, _: Who, machine: &mut Machine, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        machine.run_frame(&FrameInput { events: &events, controllers, ..Default::default() })
    }

    fn want_image(&mut self, _: u32, _: [u8; 32]) {
        panic!("only programs are started here");
    }

    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]) {
        self.asked.push((serial, program.clone(), builtins.to_vec()));
    }

    fn removed(&mut self, _: u32) {}
}

/// `start_program` (807h) and `identity` (808h) (SPEC 12.2): the host is
/// asked for the program by its identity (a built-in with the zero key);
/// the handle loads until the image arrives, possibly frames later; a
/// start fails with its reason (1 not found, 3 needs a device the host
/// does not provide, never running); the started machine has the identity
/// for its life, and `identity` reports it.
#[test]
fn start_program_and_identity() {
    if !common::have_tools() {
        return;
    }
    let src = "
    li s0, 0x100000
    li a7, 0x807
    la a0, tool
    ecall
    sw a0, 0(s0)        # tool of feed 22..: 1
    la a0, hello
    ecall
    sw a0, 4(s0)        # the built-in hello: 2
    la a0, needs
    ecall
    sw a0, 8(s0)        # needs: 3
    la a0, absent
    ecall
    sw a0, 12(s0)       # absent: 4
    la a0, empty
    ecall
    sw a0, 16(s0)       # an empty name: FFFFFFFE
    la a0, long
    ecall
    sw a0, 20(s0)       # 49 bytes: FFFFFFFE
    li a0, 0
    ecall
    sw a0, 24(s0)       # nowhere: FFFFFFFE
    li a7, 0x808
    li a0, 1
    addi a1, s0, 128
    ecall
    sw a0, 28(s0)       # identity 1: 1
    li a0, 5
    addi a1, s0, 256
    ecall
    sw a0, 32(s0)       # identity 5 (not in use): 0
    li a0, 1
    li a1, 0
    ecall
    sw a0, 36(s0)       # identity into address 0: FFFFFFFE
1:  li a7, 0x100
    ecall
    j 1b
tool:   .fill 32, 1, 0x22
        .byte 4
        .ascii \"tool\"
hello:  .fill 32, 1, 0
        .byte 5
        .ascii \"hello\"
needs:  .fill 32, 1, 0x22
        .byte 5
        .ascii \"needs\"
absent: .fill 32, 1, 0x22
        .byte 6
        .ascii \"absent\"
empty:  .fill 32, 1, 0x22
        .byte 0
long:   .fill 32, 1, 0x22
        .byte 49
        .fill 49, 1, 0x61
";
    let shell = program(&assemble(src), 1, &[MACHINES]);
    let hello = Image::parse(HELLO).unwrap();
    let mut system = System::new(&shell, true, vec![hello.id]);
    let mut d = Programs::default();
    let frame = |system: &mut System, d: &mut Programs| system.frame(&[], [Controller::default(); CONTROLLERS], d);
    frame(&mut system, &mut d);
    let ram = system.shell().ram().to_vec();
    let word = |at: usize| u32::from_le_bytes(ram[at..at + 4].try_into().unwrap());
    assert_eq!((0..10).map(|i| word(4 * i)).collect::<Vec<_>>(), [1, 2, 3, 4, 0xFFFF_FFFE, 0xFFFF_FFFE, 0xFFFF_FFFE, 1, 0, 0xFFFF_FFFE]);
    let tool = ProgramId { publisher: [0x22; 32], name: b"tool".to_vec() };
    let mut record = vec![0x22; 32];
    record.push(4);
    record.extend(b"tool");
    record.resize(81, 0);
    assert_eq!(&ram[128..209], &record[..], "the identity record");
    assert_eq!(&ram[256..260], &[0; 4], "nothing written for a handle not in use");
    let asked: Vec<(u32, Vec<u8>, bool)> = d.asked.iter().map(|(s, p, b)| (*s, p.name.clone(), b == &[hello.id])).collect();
    assert_eq!(asked, [(1, b"tool".to_vec(), true), (2, b"hello".to_vec(), true), (3, b"needs".to_vec(), true), (4, b"absent".to_vec(), true)]);
    assert!(d.asked[1].1.is_builtin());
    for h in 1..=4 {
        assert_eq!(system.state(h), state::LOADING);
        assert_eq!(system.shell().machines().unwrap().slot(h).image, [0; 32], "no image ID while loading");
    }

    // The built-in arrives at once; the device check fails one image; the
    // host holds no "absent". The tool's image arrives three frames later.
    system.deliver_image(2, Ok(hello.clone()));
    let mut bytes = Image::build("needs", 1, &[DeviceReq { slot: 2, version: 0 }], &[0x6F, 0, 0, 0]);
    bytes[32] = 5; // device 5 (messages): not one this host provides
    system.deliver_image(3, terse_core::load_image(&bytes));
    system.deliver_image(4, Err(StartFailure::not_found("no such program")));
    frame(&mut system, &mut d);
    let dev = system.shell().machines().unwrap();
    assert_eq!((system.state(1), system.state(2), system.state(3), system.state(4)), (state::LOADING, state::RUNNING, state::FAILED, state::FAILED));
    assert_eq!(dev.slot(2).image, hello.id);
    assert_eq!(&dev.slot(3).info()[..4], &[state::FAILED, 0, 0, 3], "needs a device the host does not have");
    assert_eq!(dev.slot(3).image, *blake3::hash(&bytes).as_bytes());
    assert_eq!(&dev.slot(4).info()[..4], &[state::FAILED, 0, 0, 1], "not found");
    assert_eq!(dev.slot(4).image, [0; 32]);
    assert!(system.child(3).unwrap().machine.is_none(), "a failed start runs nothing");
    assert_eq!(system.child(2).unwrap().machine.as_ref().unwrap().identity().unwrap().publisher, [0; 32]);
    for _ in 0..3 {
        frame(&mut system, &mut d);
        assert_eq!(system.state(1), state::LOADING);
    }
    let tool_image = Image::parse(&Image::build("tool", 1, &[], &[0x6F, 0, 0, 0])).unwrap();
    system.deliver_image(1, Ok(tool_image.clone()));
    frame(&mut system, &mut d);
    assert_eq!(system.state(1), state::RUNNING);
    let child = system.child(1).unwrap();
    assert_eq!((child.image, child.identity.as_ref()), (tool_image.id, Some(&tool)));
    assert_eq!(child.machine.as_ref().unwrap().identity(), Some(&tool));
    assert_eq!(system.shell().machines().unwrap().slot(1).image, tool_image.id);
    // The identity is the shell's state (STATE.md 1.10).
    let state = system.shell().state();
    assert!(state.windows(81).any(|w| w == &record[..]));
}

/// A machine that, in every frame, asks the shell to open blob ABAB…
/// (`open`, 107h) and keeps the result at RAM + 256 + 4 × frame; in its
/// first frame it also asks a second time (RAM + 0), with a bad kind (RAM +
/// 4), with a bad name (RAM + 8), reads its argument (RAM + 12; the bytes
/// at RAM + 32).
fn opener() -> Vec<u8> {
    assemble(
        "
    li s0, 0x100000
    li s1, 0
    li a7, 0x107
    la a0, req
    ecall
    sw a0, 256(s0)
    la a0, req
    ecall
    sw a0, 0(s0)
    la a0, bad
    ecall
    sw a0, 4(s0)
    la a0, badname
    ecall
    sw a0, 8(s0)
    li a7, 0x108
    addi a0, s0, 32
    ecall
    sw a0, 12(s0)
1:  li a7, 0x100
    ecall
    addi s1, s1, 4
    add t0, s0, s1
    li a7, 0x107
    la a0, req
    ecall
    sw a0, 256(t0)
    j 1b
req: .byte 1
    .fill 32, 1, 0xAB
bad: .byte 9
    .fill 32, 1, 0
badname: .byte 2
    .fill 32, 1, 0
    .byte 2
    .ascii \" x\"
",
    )
}

/// A shell that starts the image whose ID ends its ROM as handle 1 and gives
/// it that same ID as its argument (RAM + 0, + 4; handle 2: RAM + 8), calls
/// `open` itself (RAM + 12), counts open-requested events (RAM + 20) and,
/// from its frame 5 on, reads machine 1's request in every frame
/// (`request`, 809h: the requests read at RAM + 16, the last at RAM + 128).
fn opening_shell(child: &[u8; 32]) -> Image {
    let mut rom = assemble(
        "
    li s0, 0x100000
    la s1, child
    li a7, 0x800
    mv a0, s1
    ecall
    sw a0, 0(s0)
    li a7, 0x80A
    li a0, 1
    mv a1, s1
    ecall
    sw a0, 4(s0)
    li a0, 2
    ecall
    sw a0, 8(s0)
    li a7, 0x107
    la a0, req
    ecall
    sw a0, 12(s0)
1:  li a7, 0x100
    ecall
2:  li a7, 0x101
    addi a0, s0, 32
    ecall
    beqz a0, 3f
    lbu t0, 32(s0)
    li t1, 0x0F
    bne t0, t1, 2b
    lw t2, 20(s0)
    addi t2, t2, 1
    sw t2, 20(s0)
    j 2b
3:  li a7, 0x102
    ecall
    li t0, 5
    bltu a0, t0, 1b
    li a7, 0x809
    li a0, 1
    addi a1, s0, 128
    ecall
    lw t2, 16(s0)
    add t2, t2, a0
    sw t2, 16(s0)
    j 1b
req: .byte 1
    .fill 32, 1, 0
child: .fill 32, 1, 0
",
    );
    let n = rom.len();
    rom[n - 32..].copy_from_slice(child);
    program(&rom, 1, &[MACHINES])
}

/// SPEC 4 `open` and `argument`, SPEC 12.2 `request` and `argument`: one
/// request waits at a time until the shell reads it; the shell is told
/// once per request; the shell's own `open` and a machine run alone get 0;
/// the argument is the child's state (STATE.md 1.11).
#[test]
fn open_requests_and_arguments() {
    if !common::have_tools() {
        return;
    }
    let child = program(&opener(), 1, &[]);
    let mut rec = Rec::default();
    rec.images.insert(child.id, child.clone());
    let mut r = Run { system: System::new(&opening_shell(&child.id), true, Vec::new()), rec };
    r.frames(12);
    let word = |ram: &[u8], at: usize| u32::from_le_bytes(ram[at..at + 4].try_into().unwrap());
    let shell = r.system.shell().ram().to_vec();
    assert_eq!((word(&shell, 0), word(&shell, 4), word(&shell, 8), word(&shell, 12)), (1, 1, 0, 0));
    let c = r.system.child(1).unwrap().machine.as_ref().unwrap();
    let ram = c.ram();
    assert_eq!((word(ram, 0), word(ram, 4), word(ram, 8)), (0, 0xFFFF_FFFE, 0xFFFF_FFFE));
    assert_eq!(word(ram, 12), 1);
    assert_eq!(&ram[32..64], &child.id);
    // The child's frames 0-10 (system frames 1-11): the first request
    // waits until the shell's frame 5 reads it (the child's frame 4 runs
    // after that); from then on each is read in the frame after it.
    let results: Vec<u32> = (0..11).map(|f| word(ram, 256 + 4 * f)).collect();
    assert_eq!(results, [1, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1]);
    // Requests read in shell frames 5-11; one event per request made
    // (the one made in the last frame is announced in the next).
    assert_eq!((word(&shell, 16), word(&shell, 20)), (7, 7));
    assert_eq!(shell[128], 1);
    assert_eq!(&shell[129..161], &[0xAB; 32]);
    assert!(shell[161..210].iter().all(|&b| b == 0));
    // The argument ends the child's state.
    let state = c.state();
    assert_eq!(&state[state.len() - 32..], &child.id);
    // Alone: no shell to pass a request to, no argument.
    let mut alone = Machine::new(&child);
    alone.run_frame(&FrameInput::default());
    let ram = alone.ram();
    assert_eq!((word(ram, 256), word(ram, 12)), (0, 0));
    assert_eq!(alone.state().len() + 32, c.state().len());
}
