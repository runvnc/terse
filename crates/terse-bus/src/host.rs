//! The core host's system (DEVICES.md 2): terse-core's `System` of
//! machines (the shell and the machines it starts, SPEC §machines, or a single
//! machine) on the bus, run as terse-net's `HeadlessSystem` runs one with
//! the devices compiled in: the host's input is routed by the system; each
//! machine's devices give their own inputs (the page's events, content
//! completions, feeds updated) through its drivers; every image a shell
//! starts is fetched, by the content driver's services (DEVICES.md 9),
//! before the next frame, so a machine started in frame N is loaded at the
//! start of frame N + 1; snapshots are stored and read by those services.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use terse_core::machines::{ProgramId, StartFailure, failure};
use terse_core::system::{CONTROLLERS, Controller};
use terse_core::{Driver, Event, FrameInput, FrameResult, Grants, Image, Machine, Snapshot, System, SystemFrame, Who};
use terse_net::Prompt;

use crate::bus::{Bus, BusConfig, BusDevices, Wanted as Driven};
use crate::monitor::{Monitor, WatchConfig};
use crate::proto::{property, service};
use crate::services;

/// An image wanted in the frame just run: by ID, or a program.
enum Wanted {
    Image([u8; 32]),
    Program(ProgramId, Vec<[u8; 32]>),
}

/// What the system asks of the host, on the bus.
struct BusDriver {
    bus: Rc<RefCell<Bus>>,
    wanted: Vec<(u32, Wanted)>,
    /// Images to deliver at the start of the next frame.
    images: Vec<(u32, Result<Image, StartFailure>)>,
    /// The first error of a machine's devices (a driver that failed).
    error: Option<String>,
}

fn stopped_result(m: &Machine<BusDevices>) -> FrameResult {
    FrameResult { stopped: m.stopped(), used: 0, waited: false, events_dropped: 0, random_used: 0, completions_ignored: 0, calls: 0 }
}

impl Driver<BusDevices> for BusDriver {
    fn frame(&mut self, _who: Who, machine: &mut Machine<BusDevices>, events: Vec<Event>, controllers: [Controller; CONTROLLERS]) -> FrameResult {
        if self.error.is_some() {
            return stopped_result(machine);
        }
        let input = FrameInput { events: &events, controllers, ..Default::default() };
        match machine.try_run_frame(&input) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e);
                stopped_result(machine)
            }
        }
    }

    fn want_image(&mut self, serial: u32, image: [u8; 32]) {
        self.wanted.push((serial, Wanted::Image(image)));
    }

    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]) {
        self.wanted.push((serial, Wanted::Program(program.clone(), builtins.to_vec())));
    }

    fn removed(&mut self, serial: u32) {
        self.images.retain(|i| i.0 != serial);
        self.wanted.retain(|w| w.0 != serial);
    }

    fn save_snapshot(&mut self, shell: &Machine<BusDevices>, slot: u8, snapshot: Snapshot) -> Option<bool> {
        let request = [&[slot][..], &snapshot.encode()].concat();
        let answer = self.bus.borrow_mut().service(shell.devices().instance, service::SNAPSHOT_SAVE, &request);
        Some(answer.is_ok_and(|a| a.first() == Some(&1)))
    }

    fn want_snapshot(&mut self, serial: u32, shell: &Machine<BusDevices>, slot: u8) -> Option<Result<(Image, Snapshot), StartFailure>> {
        let request = [&serial.to_le_bytes()[..], &[slot]].concat();
        let answer = self.bus.borrow_mut().service(shell.devices().instance, service::SNAPSHOT_LOAD, &request);
        Some(match answer {
            Ok(a) => services::read_restored(&a),
            Err(e) => Err(StartFailure { reason: failure::SNAPSHOT, image: None, message: e }),
        })
    }

    fn delete_snapshot(&mut self, shell: &Machine<BusDevices>, slot: u8) {
        let _ = self.bus.borrow_mut().service(shell.devices().instance, service::SNAPSHOT_DELETE, &[slot]);
    }
}

/// Removes the session directory when dropped.
pub struct SessionDir(pub PathBuf);

impl Drop for SessionDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A system of machines on the bus. Dropped, it closes its machines
/// (their fields drop in order), then its drivers shut down (the last
/// reference to the bus), then the session's files go.
pub struct BusSystem {
    pub system: System<BusDevices>,
    driver: BusDriver,
    pub bus: Rc<RefCell<Bus>>,
    /// The shell's (or single machine's) image title.
    pub title: String,
    session: Option<SessionDir>,
    /// Keys whose release is the host's.
    swallowed: Vec<u8>,
    /// The watch over the drivers (PERFORMANCE.md R14, R15), if any.
    pub monitor: Option<Monitor>,
    /// The host's notices for the user (a slow device): text, and the
    /// monitor frame after which it goes.
    pub notice: Option<(Notice, u64)>,
    /// The device whose slowness stopped the shell or the single machine
    /// (the run then ends: `STATE.md` 6.4 status 4), and where its
    /// snapshot went.
    pub halted: Option<(u8, String)>,
}

/// A notice the host shows (R15): a heading and two lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub heading: String,
    pub what: String,
    pub then: String,
}

/// How long a notice stays (frames).
pub const NOTICE_FRAMES: u64 = 300;
/// How long a driver has to give its state for a snapshot before a stop.
const SNAPSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// The default drivers, programs next to `dir` (this program's): the
/// terminal (2), the vector displays (3 and 9, one process: SPEC §vector3d.calls),
/// content (4, with `content` as its options), sound (6) and the page (7), for the
/// slots in `slots`; `overrides` (slots, command line) replace the
/// default driver of those slots.
pub fn drivers(dir: &Path, slots: &[u8], content: &[String], overrides: &[(Vec<u8>, Vec<String>)]) -> Result<Vec<Driven>, String> {
    drivers_with(dir, slots, content, overrides, &[])
}

/// An installed driver chosen for a group (DEVICES.md 2.2).
#[derive(Clone, Debug)]
pub struct InstalledDriver {
    /// The group's slots (DEVICES.md 2.1's table).
    pub group: Vec<u8>,
    pub name: String,
    pub program: PathBuf,
    /// The slots and device versions of its manifest.
    pub versions: Vec<(u8, u8)>,
}

/// The installed drivers the user's drivers folder (`<config>/drivers`)
/// chooses, and notes for the user about choices that cannot be used.
/// Lines of the folder's `use` that cannot be read are the last item.
pub fn installed_drivers(config: &Path) -> (Vec<InstalledDriver>, Vec<crate::bus::Unused>, Vec<String>) {
    use terse_net::driverlist::{Source, resolve};
    let (sources, bad) = resolve(config);
    let (mut out, mut unused) = (Vec::new(), Vec::new());
    for (group, source) in sources {
        match source {
            Source::Installed { name, program, manifest } => out.push(InstalledDriver { group: group.to_vec(), name, program, versions: manifest.slots }),
            Source::Shipped { note: Some((name, why)), .. } => unused.push(crate::bus::Unused { name, slot: group[0], why }),
            Source::Shipped { .. } => {}
        }
    }
    (out, unused, bad)
}

/// [`drivers`], with the installed drivers that replace the default driver
/// of their group (DEVICES.md 2.2): each is started with the options the
/// default driver would get, and falls back to it.
pub fn drivers_with(dir: &Path, slots: &[u8], content: &[String], overrides: &[(Vec<u8>, Vec<String>)], installed: &[InstalledDriver]) -> Result<Vec<Driven>, String> {
    let exe = |name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)).display().to_string();
    let defaults: [(Vec<u8>, Vec<String>); 5] = DEFAULT_DRIVERS.map(|(s, name)| {
        let mut cmd = vec![exe(name)];
        if s.contains(&4) {
            cmd.extend_from_slice(content);
        }
        // The terminal and the page draw with the fonts the config
        // directory's `fonts` names (README "Fonts").
        if (s.contains(&2) || s.contains(&7))
            && let Some(i) = content.iter().position(|a| a == "--config")
            && let Some(dir) = content.get(i + 1)
        {
            cmd.extend(["--config".to_string(), dir.clone()]);
        }
        (s.to_vec(), cmd)
    });
    let mut out: Vec<Driven> = Vec::new();
    let mut covered: Vec<u8> = Vec::new();
    for (s, cmd) in overrides {
        // Full text (10) is the terminal's: the driver that replaces the
        // terminal's serves it too (if it says so in HELLO, it is granted).
        let mut s = s.clone();
        if s.contains(&2) && !s.contains(&FULL_TEXT) {
            s.push(FULL_TEXT);
        }
        let s: Vec<u8> = s.iter().copied().filter(|x| slots.contains(x)).collect();
        if !s.is_empty() {
            covered.extend(&s);
            out.push(Driven { slots: s, command: cmd.clone(), installed: None });
        }
    }
    for (group, cmd) in defaults {
        let s: Vec<u8> = group.iter().copied().filter(|x| slots.contains(x) && !covered.contains(x)).collect();
        if s.is_empty() {
            continue;
        }
        covered.extend(&s);
        match installed.iter().find(|i| i.group.first() == group.first()) {
            // Only a whole group goes to an installed driver: a --driver
            // that took part of it leaves the rest to the default one.
            Some(i) if s.len() == group.iter().filter(|x| slots.contains(x)).count() => {
                let mut command = vec![i.program.display().to_string()];
                command.extend_from_slice(&cmd[1..]);
                let shipped = Path::new(&cmd[0]).exists().then_some(cmd);
                out.push(Driven { slots: s, command, installed: Some(crate::bus::Installed { name: i.name.clone(), versions: i.versions.clone(), shipped }) });
            }
            _ => out.push(Driven { slots: s, command: cmd, installed: None }),
        }
    }
    if let Some(s) = slots.iter().find(|s| !covered.contains(s)) {
        return Err(format!("no driver for device {s} (SPEC §image)"));
    }
    Ok(out)
}

/// Device 10, full text: a capability the terminal's driver gives (SPEC
/// §terminal.text).
const FULL_TEXT: u8 = terse_core::text::FULL_TEXT;

/// The default drivers: the slots each serves and its program's name.
const DEFAULT_DRIVERS: [(&[u8], &str); 5] = [
    (&[2, FULL_TEXT], "terse-dev-terminal"),
    (&[3, terse_core::vector3::SLOT], "terse-dev-vector"),
    (&[4], "terse-dev-content"),
    (&[terse_core::sound::SLOT], "terse-dev-sound"),
    (&[7], "terse-dev-page"),
];

/// The slots whose drivers a single machine's run starts (SPEC §image): those
/// of [`needed_slots`], less the slots only optional devices use that are
/// withheld (`without`) or whose driver is not installed (no override and
/// no default driver in `dir`): such a device is absent. A slot the image
/// requires stays, so that a missing driver still stops the run. A shell
/// gets every slot, as before.
pub fn installed_slots(dir: &Path, image: &Image, shell: bool, without: &[u8], overrides: &[(Vec<u8>, Vec<String>)]) -> Vec<u8> {
    installed_slots_with(dir, image, shell, without, overrides, &[])
}

/// [`installed_slots`], counting the installed drivers too (DEVICES.md 2.2).
pub fn installed_slots_with(dir: &Path, image: &Image, shell: bool, without: &[u8], overrides: &[(Vec<u8>, Vec<String>)], installed: &[InstalledDriver]) -> Vec<u8> {
    let slots = needed_slots(image, shell);
    if shell {
        return slots;
    }
    let partner = |s: u8| match s {
        3 => Some(terse_core::vector3::SLOT),
        s if s == terse_core::vector3::SLOT => Some(3),
        _ => None,
    };
    // A slot is wanted if the image requires it, or requires the device it
    // shares a driver with, or lists it optional and does not withhold it.
    let wanted = |s: u8| {
        image.requires(s) || partner(s).is_some_and(|p| image.requires(p)) || (image.lists(s) && !without.contains(&s)) || partner(s).is_some_and(|p| image.lists(p) && !without.contains(&p))
    };
    let installed = |s: u8| {
        overrides.iter().any(|(o, _)| o.contains(&s))
            || installed.iter().any(|i| i.group.contains(&s))
            || DEFAULT_DRIVERS.iter().any(|(d, name)| d.contains(&s) && dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)).exists())
    };
    slots
        .into_iter()
        .filter(|&s| {
            let required = image.requires(s) || partner(s).is_some_and(|p| image.requires(p));
            required || (wanted(s) && installed(s))
        })
        .collect()
}

/// The slots whose drivers a run needs: every device but System and
/// Machines the image lists (slots that are no device left out) (device 9 brings device 3, whose style it
/// shares), or, for a shell, every device a machine it starts may list.
pub fn needed_slots(image: &Image, shell: bool) -> Vec<u8> {
    if shell {
        return vec![2, 3, 4, terse_core::sound::SLOT, 7, terse_core::vector3::SLOT, FULL_TEXT];
    }
    // Only slots that are devices (SPEC §image: 0, 5 and 11-255 are not, and
    // an optional one is never granted, whatever driver a --driver names).
    let device = |s: u8| terse_core::image::SUPPORTED_DEVICES.iter().any(|d| d.slot == s);
    let mut v: Vec<u8> = image.devices.iter().map(|d| d.slot).filter(|&s| s != 1 && s != terse_core::machines::SLOT && device(s)).collect();
    if v.contains(&terse_core::vector3::SLOT) && !v.contains(&3) {
        v.push(3);
    }
    if v.contains(&3) && !v.contains(&terse_core::vector3::SLOT) {
        v.push(terse_core::vector3::SLOT);
    }
    v.sort_unstable();
    v
}

/// What to run.
pub struct Start<'a> {
    pub config: BusConfig,
    pub drivers: Vec<Driven>,
    pub image: &'a Image,
    /// Run as the shell (device 8 granted if listed), with these built-in
    /// programs' IDs.
    pub shell: Option<Vec<[u8; 32]>>,
    pub identity: Option<ProgramId>,
    /// Restored from this snapshot (STATE.md 4).
    pub snapshot: Option<&'a Snapshot>,
    /// The machine's grants (SPEC §image), as a recording or snapshot gives
    /// them; `None`: every optional device a driver gives and `without`
    /// does not name.
    pub grants: Option<Grants>,
    /// Optional devices withheld (`--without`).
    pub without: Vec<u8>,
}

impl Start<'_> {
    /// Starts the drivers in a new session directory and the machine as
    /// instance 0.
    pub fn start(self) -> Result<BusSystem, String> {
        let session = SessionDir(crate::transport::session_dir()?);
        let mut config = self.config;
        config.session = session.0.clone();
        let bus = Rc::new(RefCell::new(Bus::start(&config, &self.drivers)?));
        let identity = self.snapshot.map_or(self.identity.clone(), |s| s.identity.clone());
        let grants = match (self.snapshot, self.grants) {
            (Some(s), _) => s.grants(self.image).map_err(|e| e.to_string())?,
            (None, Some(g)) => g,
            (None, None) => {
                let b = bus.borrow();
                self.image.grants(|d| b.provides(d) && !self.without.contains(&d.slot))
            }
        };
        let mut machine = Bus::open_instance(&bus, 0, self.image, Some(&grants), identity.as_ref()).map_err(|f| f.message)?;
        machine.set_identity(identity);
        if let Some(builtins) = self.shell {
            machine.grant_machines(builtins);
        }
        if let Some(s) = self.snapshot {
            machine.load_state(&s.state).map_err(|e| e.to_string())?;
        }
        let mut bs = BusSystem::new(bus, machine);
        bs.title = self.image.title_lossy();
        bs.session = Some(session);
        Ok(bs)
    }
}


impl BusSystem {
    /// The system whose shell (or single machine) is `machine`, opened on
    /// `bus` as instance 0. A shell gets its snapshot slots from the
    /// content driver.
    pub fn new(bus: Rc<RefCell<Bus>>, machine: Machine<BusDevices>) -> Self {
        let mut system = System::with_machine(machine);
        if system.is_system()
            && let Ok(t) = bus.borrow_mut().service(0, service::SNAPSHOT_TABLE, &[])
        {
            system.set_snapshot_table(services::read_table(&t));
        }
        let driver = BusDriver { bus: bus.clone(), wanted: Vec::new(), images: Vec::new(), error: None };
        BusSystem { system, driver, bus, title: String::new(), session: None, swallowed: Vec::new(), monitor: None, notice: None, halted: None }
    }

    /// Runs the system's next frame with the host's input `events` (in the
    /// shell's screen units), then fetches every image it wants.
    pub fn frame(&mut self, events: &[Event], controllers: [Controller; CONTROLLERS]) -> Result<SystemFrame, String> {
        for (serial, image) in self.driver.images.drain(..) {
            self.system.deliver_image(serial, image);
        }
        if let Some(m) = &mut self.monitor {
            m.begin(&mut self.bus.borrow_mut());
        }
        let result = self.system.frame(events, controllers, &mut self.driver);
        if let Some(e) = self.driver.error.take() {
            return Err(e);
        }
        if let Some(e) = &self.bus.borrow().failed {
            return Err(e.clone());
        }
        let wanted = std::mem::take(&mut self.driver.wanted);
        let mut bus = self.bus.borrow_mut();
        for (serial, want) in wanted {
            let image = match &want {
                _ if !bus.has_service(service::FETCH_IMAGE) => Err(StartFailure::not_found("no content device")),
                Wanted::Image(id) => match bus.service(0, service::FETCH_IMAGE, &services::fetch_image(serial, id)) {
                    Ok(a) => services::fetched_image(&a),
                    Err(e) => Err(StartFailure::not_found(e)),
                },
                Wanted::Program(program, builtins) => {
                    match bus.service(0, service::FETCH_PROGRAM, &services::fetch_program(serial, program, builtins)) {
                        Ok(a) => services::fetched_image(&a),
                        Err(e) => Err(StartFailure::not_found(e)),
                    }
                }
            };
            self.driver.images.push((serial, image));
        }
        if let Some(e) = &bus.failed {
            return Err(e.clone());
        }
        if let Some(m) = &mut self.monitor {
            m.emulated(&bus);
        }
        Ok(result)
    }

    /// Starts watching the drivers (R14, R15).
    pub fn watch(&mut self, cfg: &WatchConfig) {
        self.monitor = Some(Monitor::new(cfg, &self.bus.borrow()));
    }

    /// The frame's picture is presented (`present`: handing it to the
    /// display): the monitor judges the frame, and the host carries out
    /// what the rule asks. Returns lines for standard error.
    pub fn end_frame(&mut self, present: std::time::Duration) -> Result<Vec<String>, String> {
        let Some(m) = &mut self.monitor else { return Ok(Vec::new()) };
        let images: Vec<(u32, [u8; 32])> = std::iter::once((0, *self.system.shell().image_id()))
            .chain(self.system.children().filter_map(|(_, c)| c.machine.as_ref().map(|m| (m.devices().instance, *m.image_id()))))
            .collect();
        let actions = m.end(&self.bus.borrow(), present.as_nanos() as u64, |i| images.iter().find(|e| e.0 == i).map(|e| e.1));
        let frame = m.frame;
        if self.notice.as_ref().is_some_and(|(_, until)| *until < frame) {
            self.notice = None;
        }
        let mut out = Vec::new();
        for a in actions {
            out.extend(self.act(a)?);
        }
        Ok(out)
    }

    /// The driver serving `component`'s slot, with its description.
    fn driver_of(&self, c: terse_perflog::Component) -> Option<(usize, String)> {
        let terse_perflog::Component::Device(slot) = c else { return None };
        let bus = self.bus.borrow();
        let p = bus.proc_for(slot)?;
        let d = &bus.procs[p];
        let id = terse_perflog::Identity { name: d.info.name.clone(), version: d.info.version.clone(), build: d.info.build.clone() };
        Some((p, format!("device {slot} ({})", id.line())))
    }

    /// The machines that use driver `p`: handles (0: the shell or the
    /// single machine), still running or paused.
    fn users(&self, p: usize) -> Vec<u8> {
        let mut v = Vec::new();
        if self.system.shell().stopped().is_none() && self.system.shell().devices().procs().contains(&p) {
            v.push(0);
        }
        for (h, c) in self.system.children() {
            if let Some(m) = &c.machine
                && m.stopped().is_none()
                && m.devices().procs().contains(&p)
            {
                v.push(h);
            }
        }
        v
    }

    /// Takes the snapshot of machine `h` (0: the single machine; the
    /// shell is never snapshotted) within a short time limit, and keeps it:
    /// in the machine's automatic slot (SPEC §machines.snapshots) when a
    /// shell runs it, else in a file in the record's directory. Returns
    /// where it went (`slot<n>`, `file`, `none`) and a description.
    fn save(&mut self, h: u8) -> (String, String) {
        if h == 0 && self.system.is_system() {
            return ("none".into(), "the shell is never snapshotted".into());
        }
        let timeout = self.bus.borrow().config.reply_timeout;
        self.bus.borrow_mut().set_reply_timeout(SNAPSHOT_TIMEOUT.min(timeout));
        self.bus.borrow_mut().saving = true;
        let snapshot = match h {
            0 => terse_core::Snapshot::try_of(self.system.shell_mut()),
            h => match self.system.child_mut(h).and_then(|c| c.machine.as_mut()) {
                Some(m) => terse_core::Snapshot::try_of(m),
                None => Err("no machine".into()),
            },
        };
        self.bus.borrow_mut().set_reply_timeout(timeout);
        self.bus.borrow_mut().saving = false;
        let snapshot = match snapshot {
            Ok(s) => s,
            Err(e) => return ("none".into(), format!("no snapshot: {e}")),
        };
        if h == 0 {
            let Some(dir) = self.monitor.as_ref().and_then(|m| m.dir.clone()) else { return ("none".into(), "no record directory to keep it in".into()) };
            let name: String = snapshot.image_id()[..8].iter().map(|b| format!("{b:02x}")).collect();
            let path = dir.join(format!("stopped-{name}.tsnp"));
            let written = std::fs::create_dir_all(&dir).and_then(|_| {
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, snapshot.encode())?;
                std::fs::rename(&tmp, &path)
            });
            return match written {
                Ok(()) => ("file".into(), path.display().to_string()),
                Err(e) => ("none".into(), format!("{}: {e}", path.display())),
            };
        }
        let slot = terse_core::snapshot::auto_slot(h);
        let info = snapshot.info();
        match self.driver.save_snapshot(self.system.shell(), slot, snapshot) {
            Some(true) => {
                self.system.set_snapshot_info(slot, Some(info));
                (format!("slot{slot}"), format!("snapshot slot {slot}"))
            }
            _ => ("none".into(), format!("snapshot slot {slot} could not be stored")),
        }
    }

    /// Carries out what the rule asks (R15).
    fn act(&mut self, a: terse_perflog::Action) -> Result<Vec<String>, String> {
        use terse_perflog::{Action, Line};
        let mut out = Vec::new();
        let frame = self.monitor.as_ref().map_or(0, |m| m.frame.saturating_sub(1));
        let (c, stop, count) = match a {
            Action::Warn { component, count } => (component, false, count),
            Action::Stop { component, count } => (component, true, count),
        };
        let log = |s: &Self, l: Line| {
            if let Some(m) = &s.monitor {
                m.add(l);
            }
        };
        let Some((p, what)) = self.driver_of(c) else {
            // The host's own part: it cannot stop itself; it records and shows.
            log(self, Line::Warn { frame, component: c, count });
            out.push(format!("terse-bus: the host's own {c} made {count} frames late"));
            return Ok(out);
        };
        let users = self.users(p);
        if !stop {
            log(self, Line::Warn { frame, component: c, count });
            // Save at once (the owner's "save immediately"): the snapshot
            // a stop would otherwise have to take from a slow device.
            let mut saved = Vec::new();
            for &h in &users {
                let (_, d) = self.save(h);
                saved.push(if h == 0 { d } else { format!("machine {h}: {d}") });
            }
            self.notice = Some((
                Notice {
                    heading: "a slow device".into(),
                    what: format!("{what} is slow: {count} late frames."),
                    then: "Your work is saved; if it goes on, the machines that use it are stopped.".into(),
                },
                frame + NOTICE_FRAMES,
            ));
            out.push(format!("terse-bus: {what} is slow ({count} late frames): your work is saved ({}); if it goes on, the machines that use it are stopped", saved.join("; ")));
            return Ok(out);
        }
        let slot = match c {
            terse_perflog::Component::Device(s) => s,
            _ => 0,
        };
        for h in users {
            let (saved, d) = self.save(h);
            let image = match h {
                0 => Some(*self.system.shell().image_id()),
                h => self.system.child(h).and_then(|c| c.machine.as_ref()).map(|m| *m.image_id()),
            };
            log(self, Line::Stop { frame, component: c, machine: h as u32, image: terse_perflog::record::image_field(image.as_ref()), saved: saved.clone() });
            match h {
                0 => {
                    self.system.shell_mut().halt(slot);
                    self.halted = Some((slot, d.clone()));
                }
                h => {
                    if let Some(m) = self.system.child_mut(h).and_then(|c| c.machine.as_mut()) {
                        m.halt(slot);
                    }
                }
            }
            out.push(format!("terse-bus: {what} did not keep up ({count} late frames): machine {h} stopped; {d}"));
        }
        self.notice = Some((
            Notice { heading: "a device did not keep up".into(), what: format!("{what} did not keep up: {count} late frames."), then: "The machines that use it were saved and stopped.".into() },
            frame + NOTICE_FRAMES,
        ));
        Ok(out)
    }

    /// The consent question waiting (SPEC §content.feeds), how many wait, and the
    /// asking machine's title.
    pub fn prompt(&mut self) -> Result<Option<(Prompt, usize, String)>, String> {
        let Some(bytes) = self.system.shell().devices().property(property::PROMPT)? else { return Ok(None) };
        let Some((p, waiting)) = services::read_prompt(&bytes) else { return Ok(None) };
        let title = match p.tag {
            0 => self.title.clone(),
            tag => self.system.children().find(|(_, c)| c.serial == tag).map_or(String::new(), |(_, c)| c.title.clone()),
        };
        Ok(Some((p, waiting, title)))
    }

    /// The host's input for the next frame with the consent question in
    /// mind (SPEC §content.feeds): while one is asked the keyboard is the host's: Y
    /// allows, N or Esc refuses, and no key or text reaches a machine (nor
    /// the release of a key pressed then).
    pub fn input(&mut self, events: Vec<Event>) -> Result<Vec<Event>, String> {
        let prompt = self.prompt()?;
        let mut out = Vec::with_capacity(events.len());
        let mut asked = prompt.map(|p| p.0);
        for e in events {
            match e.kind {
                Event::KEY_DOWN if asked.is_some() => {
                    self.swallow(Some(e.a));
                    let allow = match e.a {
                        0x1C => Some(true),
                        0x11 | 0x29 => Some(false),
                        _ => None,
                    };
                    if let Some(allow) = allow {
                        let p = asked.take().unwrap();
                        self.answer(&p, allow)?;
                        // The keyboard stays the host's for this frame.
                        asked = Some(Prompt { tag: u32::MAX, ..p });
                    }
                }
                Event::TEXT if asked.is_some() => {}
                Event::KEY_UP if self.swallowed.contains(&e.a) => self.swallowed.retain(|&u| u != e.a),
                _ => out.push(e),
            }
        }
        Ok(out)
    }

    /// Notes that the release of key `usage` is the host's, as its press
    /// was (once, however often the press auto-repeats).
    pub fn swallow(&mut self, usage: Option<u8>) {
        if let Some(u) = usage.filter(|u| !self.swallowed.contains(u)) {
            self.swallowed.push(u);
        }
    }

    /// The user's answer to the question shown.
    pub fn answer(&mut self, p: &Prompt, allow: bool) -> Result<(), String> {
        self.system.shell_mut().devices_mut().set_property(property::ANSWER, &services::answer(p.tag, p.handle, allow))?;
        // Flushed with the next request; a socket driver gets it at once.
        self.bus.borrow_mut().flush()
    }

    /// The state hash of every machine: the shell's, then `(handle, hash)`
    /// of each child (`None`: no machine yet, or its start failed).
    pub fn hashes(&mut self) -> Result<([u8; 32], Vec<(u8, Option<[u8; 32]>)>), String> {
        let shell = self.system.shell_mut().try_state_hash()?;
        let mut out = Vec::new();
        for (h, c) in self.system.children_mut() {
            out.push((h, match &mut c.machine {
                Some(m) => Some(m.try_state_hash()?),
                None => None,
            }));
        }
        Ok((shell, out))
    }
}

/// The shell image's title when it is the built-in one.
pub fn shell_identity(image: &Image) -> ProgramId {
    ProgramId { publisher: [0; 32], name: image.title.clone() }
}
