//! A system of machines (SPEC §machines): the shell, which has device 8, and the
//! machines it starts. The system decides the order of frames, routes the
//! host's input and keeps device 8's records up to date; a [`Driver`]
//! (headless or windowed) supplies everything else a machine's frame needs
//! (content completions, page events, random bytes) and fetches images.
//!
//! Without device 8 (a plain image, or a shell image not run as the shell)
//! the system is a single machine: every event goes to it unchanged and the
//! home key is dropped.

use crate::devices::{Devices, InProcess};
use crate::image::Image;
use crate::machine::{FrameResult, Machine, Stopped};
use crate::machines::{Command, MAX_MACHINES, ProgramId, StartFailure, failure, map_into, picture, state};
use crate::snapshot::{SLOTS, Snapshot, SnapshotInfo};
use crate::system::{CONTROLLERS, Controller, Event};

/// Which machine a driver is asked to run: handle 0 is the shell; `serial`
/// tells apart machines that had the same handle (the shell's is 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Who {
    pub handle: u8,
    pub serial: u32,
}

/// What a host does for the machines of a system (with devices `D`).
pub trait Driver<D: Devices = InProcess> {
    /// Runs one frame of `machine` with the input `events` the system
    /// routed to it (the driver adds the device events it produces itself:
    /// content completions, page events, feeds updated) and `controllers`.
    fn frame(&mut self, who: Who, machine: &mut Machine<D>, events: Vec<Event>, controllers: [Controller; CONTROLLERS])
    -> FrameResult;
    /// The shell started machine `serial`: fetch the image with this ID
    /// and hand it to [`System::deliver_image`].
    fn want_image(&mut self, serial: u32, image: [u8; 32]);
    /// The shell started machine `serial` as a program (`start_program`):
    /// find the current version the host holds (a built-in by its title,
    /// among `builtins`, for the all-zero key), fetch it and hand it to
    /// [`System::deliver_image`].
    fn want_program(&mut self, serial: u32, program: &ProgramId, builtins: &[[u8; 32]]);
    /// Machine `serial` ceased to exist (the shell stopped it).
    fn removed(&mut self, serial: u32);
    /// Store `snapshot` in slot `slot` of the snapshots of `shell` (SPEC
    /// §machines.snapshots), durably. Returns the result if it is known at once; else the
    /// host reports it later with [`System::snapshot_done`]. The default
    /// keeps no snapshots.
    fn save_snapshot(&mut self, shell: &Machine<D>, slot: u8, snapshot: Snapshot) -> Option<bool> {
        let _ = (shell, slot, snapshot);
        Some(false)
    }
    /// The shell asked to restore the snapshot in `slot` as machine
    /// `serial`: read it, get its image (store, else network) and hand both
    /// to [`System::deliver_snapshot`], or return them at once.
    fn want_snapshot(&mut self, serial: u32, shell: &Machine<D>, slot: u8) -> Option<Result<(Image, Snapshot), StartFailure>> {
        let _ = (serial, shell);
        Some(Err(StartFailure { reason: failure::SNAPSHOT, image: None, message: format!("no snapshot in slot {slot}") }))
    }
    /// Remove the snapshot in `slot` of `shell`'s snapshots.
    fn delete_snapshot(&mut self, shell: &Machine<D>, slot: u8) {
        let _ = (shell, slot);
    }
}

/// A machine the shell started.
pub struct Child<D: Devices = InProcess> {
    pub serial: u32,
    /// The image ID given to `start`; for `start_program`, that of the
    /// version the host delivered (zero until then).
    pub image: [u8; 32],
    /// The identity it was started with (`start_program`).
    pub identity: Option<ProgramId>,
    /// The machine, once its image has been loaded.
    pub machine: Option<Machine<D>>,
    /// The image's title, once loaded.
    pub title: String,
    /// Why the start failed, if it did.
    pub error: Option<String>,
    /// The argument the shell gave it before its first frame (SPEC §machines.calls).
    pub argument: Option<[u8; 32]>,
    /// The image, delivered but not yet latched (SPEC §machines.frames).
    delivered: Option<Result<Image, StartFailure>>,
    /// The snapshot slot it is restored from (`restore`), and the snapshot
    /// once delivered with its image.
    pub restore: Option<u8>,
    snapshot: Option<Snapshot>,
    /// Units used in its last frame.
    last_used: u32,
}

/// What one system frame did.
#[derive(Clone, Debug, Default)]
pub struct SystemFrame {
    /// The shell's frame, unless the shell had already stopped.
    pub shell: Option<FrameResult>,
    /// The children that ran, by handle.
    pub children: Vec<(u8, FrameResult)>,
}

pub struct System<D: Devices = InProcess> {
    shell: Machine<D>,
    children: [Option<Child<D>>; MAX_MACHINES],
    next_serial: u32,
    /// Buttons of the last pointer event (to tell presses).
    buttons: u8,
    /// Snapshots handed to the driver and not yet reported: slot and what
    /// `snapshot_info` will tell once stored.
    saving: Vec<(u8, SnapshotInfo)>,
    /// Snapshot results to report at the next latch: slot, stored.
    saved: Vec<(u8, bool)>,
    /// The eye the host draws the 3D scene from (SPEC §vector3d.space), for routing
    /// the pointer over panels.
    eye: (f32, f32),
}

/// The shell's 3D scene as the host shows it after a frame (SPEC §machines.screens):
/// which Screens are in effect, the view of the whole scene, the panels as
/// seen from the eye (far to near; not the machine a finished view shows
/// flat) and whether the view is moving.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub view: crate::vector3::SceneView,
    pub panels: Vec<crate::vector3::Panel>,
    pub moving: bool,
    /// The machine a finished view shows flat (0: none).
    pub faced: u8,
    /// The view's progress, 0 to 4096, eased by the host.
    pub progress: u16,
}

const NO_CONTROLLERS: [Controller; CONTROLLERS] = [Controller { connected: false, buttons: 0, left: (0, 0), right: (0, 0) }; CONTROLLERS];

impl System {
    /// A system whose shell runs `image`. With `shell`, the image is the
    /// shell: it gets device 8 if it lists it, and `builtins` are the IDs of
    /// the host's built-in programs. Without, it is a single machine.
    pub fn new(image: &Image, shell: bool, builtins: Vec<[u8; 32]>) -> Self {
        let shell = if shell { Machine::new_shell(image, builtins) } else { Machine::new(image) };
        System::with_machine(shell)
    }
}

impl<D: Devices> System<D> {
    /// A system whose shell (or single machine) already exists: a restored
    /// one (`terse run --load-state`), or one with devices elsewhere (a
    /// shell is one granted device 8, [`Machine::grant_machines`]).
    pub fn with_machine(machine: Machine<D>) -> Self {
        System {
            shell: machine,
            children: Default::default(),
            next_serial: 1,
            buttons: 0,
            saving: Vec::new(),
            saved: Vec::new(),
            eye: (0.0, 0.0),
        }
    }

    /// The shell's snapshot slots as the host has them when the system
    /// starts (SPEC §machines.snapshots).
    pub fn set_snapshot_table(&mut self, table: [Option<SnapshotInfo>; SLOTS]) {
        if let Some(dev) = self.shell.machines_mut() {
            for (slot, info) in table.into_iter().enumerate() {
                dev.set_snapshot(slot as u8, info);
            }
        }
    }

    /// The host stored the snapshot it was given for `slot` (or could not):
    /// reported to the shell at the start of the next frame (SPEC §machines.frames).
    pub fn snapshot_done(&mut self, slot: u8, stored: bool) {
        self.saved.push((slot, stored));
    }

    /// The snapshot and image for restored machine `serial` arrived, or
    /// could not be had; taken into account at the start of the next frame.
    pub fn deliver_snapshot(&mut self, serial: u32, delivered: Result<(Image, Snapshot), StartFailure>) {
        if let Some(c) = self.children.iter_mut().flatten().find(|c| c.serial == serial && c.machine.is_none()) {
            c.delivered = Some(delivered.map(|(image, snapshot)| {
                c.snapshot = Some(snapshot);
                image
            }));
        }
    }

    pub fn shell(&self) -> &Machine<D> {
        &self.shell
    }

    pub fn shell_mut(&mut self) -> &mut Machine<D> {
        &mut self.shell
    }

    /// The shell's identity (a built-in shell's, SPEC §machines.calls), whose named
    /// records it reads (SPEC §content.records).
    pub fn set_shell_identity(&mut self, identity: Option<ProgramId>) {
        self.shell.set_identity(identity);
    }

    /// Whether the shell has device 8: this is a system, not a single machine.
    pub fn is_system(&self) -> bool {
        self.shell.machines().is_some()
    }

    /// The machine with handle `h` (1-8), if the shell started one.
    pub fn child(&self, h: u8) -> Option<&Child<D>> {
        self.children.get(h.wrapping_sub(1) as usize)?.as_ref()
    }

    /// The machine with handle `h` (1-8), to change (tests and tools).
    pub fn child_mut(&mut self, h: u8) -> Option<&mut Child<D>> {
        self.children.get_mut(h.wrapping_sub(1) as usize)?.as_mut()
    }

    /// The children, with their handles, in increasing handle order.
    pub fn children(&self) -> impl Iterator<Item = (u8, &Child<D>)> {
        self.children.iter().enumerate().filter_map(|(i, c)| Some((i as u8 + 1, c.as_ref()?)))
    }

    /// The children, to change (a host's presentation of them).
    pub fn children_mut(&mut self) -> impl Iterator<Item = (u8, &mut Child<D>)> {
        self.children.iter_mut().enumerate().filter_map(|(i, c)| Some((i as u8 + 1, c.as_mut()?)))
    }

    /// The machine with this serial (0: the shell).
    pub fn machine_by_serial(&self, serial: u32) -> Option<(u8, &Machine<D>)> {
        if serial == 0 {
            return Some((0, &self.shell));
        }
        self.children().find(|(_, c)| c.serial == serial).and_then(|(h, c)| Some((h, c.machine.as_ref()?)))
    }

    /// Handle `h`'s state as device 8 records it (`state::NONE` if none).
    pub fn state(&self, h: u8) -> u8 {
        self.shell.machines().map_or(state::NONE, |m| m.slot(h).state)
    }

    /// The machine that has the focus (0: the shell).
    pub fn focus(&self) -> u8 {
        self.shell.machines().map_or(0, |m| m.focus())
    }

    /// The eye the host draws the 3D scene from (SPEC §vector3d.space): screen units
    /// from the reference eye. Panels are hit as seen from it (SPEC §machines.input).
    pub fn set_eye(&mut self, eye: (f32, f32)) {
        self.eye = eye;
    }

    /// The children's pictures as the host shows them flat: handle,
    /// rectangle of the picture in the shell's units, in drawing order
    /// (SPEC §machines.screens); the machine a finished view shows flat in its
    /// rectangle among them.
    pub fn pictures(&self) -> Vec<(u8, crate::page::Region)> {
        let Some(dev) = self.shell.machines() else { return Vec::new() };
        let faced = self.faced();
        self.children()
            .filter(|(h, c)| c.machine.is_some() && dev.slot(*h).has_picture())
            .filter_map(|(h, _)| {
                let rect = if h == faced { dev.view().rect } else { dev.slot(h).rect };
                Some((h, picture(rect)?))
            })
            .collect()
    }

    /// The frame counter of the shell's last frame (the view's progress is
    /// counted in it).
    fn shell_frame(&self) -> u32 {
        self.shell.frame().wrapping_sub(1)
    }

    /// The machine a finished view shows flat, if any.
    fn faced(&self) -> u8 {
        let Some(dev) = self.shell.machines() else { return 0 };
        let v = dev.view();
        if v.handle != 0 && v.toward && v.progress(self.shell_frame()) == crate::machines::VIEW_DONE && self.panel_mask() & 1 << (v.handle - 1) != 0 {
            v.handle
        } else {
            0
        }
    }

    /// The handles whose Screens in the shell's 3D list are in effect (SPEC
    /// §vector3d.list): bit h − 1 for handle h.
    pub fn panel_mask(&self) -> u8 {
        let Some(dev) = self.shell.machines() else { return 0 };
        dev.panel_mask(&self.shell.list3(), |h| self.child(h).is_some_and(|c| c.machine.is_some()))
    }

    /// The shell's 3D scene as the host shows it now, seen from `eye`
    /// (SPEC §machines.calls `face`, 12.3). `None` when no Screen is in effect: the
    /// shell's screen is drawn as a single machine's.
    pub fn scene(&self, eye: (f32, f32)) -> Option<Scene> {
        let mask = self.panel_mask();
        if mask == 0 {
            return None;
        }
        let dev = self.shell.machines()?;
        let list = self.shell.list3();
        let list = &list[..];
        let v = dev.view();
        let frame = self.shell_frame();
        let mut scene = Scene::default();
        scene.view.screens = mask;
        if v.handle != 0 && mask & 1 << (v.handle - 1) != 0 {
            let q = v.progress(frame);
            scene.progress = q;
            scene.moving = v.moving(frame);
            if let (Some(corners), Some(p)) = (crate::vector3::screen_corners(list, v.handle), picture(v.rect)) {
                let target = (p.x as f64, p.y as f64, p.w as f64, p.h as f64);
                scene.view.view = crate::vector3::view_transform(&corners, target, crate::vector3::ease(q));
            }
            if !scene.moving && v.toward {
                scene.faced = v.handle;
            }
        }
        crate::vector3::project_scene(list, eye, &scene.view, &mut Vec::new(), &mut Vec::new(), &mut scene.panels);
        let faced = scene.faced;
        scene.panels.retain(|p| faced == 0 || p.handle != faced);
        Some(scene)
    }

    /// The image for machine `serial` arrived, or could not be had; it is
    /// taken into account at the start of the next frame (SPEC §machines.frames).
    pub fn deliver_image(&mut self, serial: u32, image: Result<Image, StartFailure>) {
        if let Some(c) = self.children.iter_mut().flatten().find(|c| c.serial == serial && c.machine.is_none()) {
            c.delivered = Some(image);
        }
    }

    /// Whether the system has stopped: the shell exited or faulted.
    pub fn stopped(&self) -> Option<Stopped> {
        self.shell.stopped()
    }

    /// The samples of the system frame just run (SPEC §sound.presentation): every
    /// machine's whose frame ended since the last call (the shell's and
    /// its children's that ran), added and clamped to 16 bits into `out`, 800
    /// pairs, left and right interleaved. Returns whether any machine gave
    /// samples (`out` is all zero if none did).
    pub fn take_audio(&mut self, out: &mut [i16; 2 * crate::sound::FRAME_SAMPLES]) -> bool {
        let mut sum = [0i32; 2 * crate::sound::FRAME_SAMPLES];
        let mut any = false;
        let children = self.children.iter_mut().filter_map(|c| c.as_mut().and_then(|c| c.machine.as_mut()));
        for m in std::iter::once(&mut self.shell).chain(children) {
            if let Some(frame) = m.take_audio() {
                any = true;
                for (o, s) in sum.iter_mut().zip(frame) {
                    *o += s as i32;
                }
            }
        }
        for (o, s) in out.iter_mut().zip(sum) {
            *o = s.clamp(-32768, 32767) as i16;
        }
        any
    }

    /// Runs one frame of the system (SPEC §machines.frames): latches what changed,
    /// routes `events` (the host's input, in the shell's screen units), runs
    /// the shell, carries out its starts and stops, then runs each running
    /// child in increasing handle order. `controllers` go to the machine
    /// with the focus.
    pub fn frame(&mut self, events: &[Event], controllers: [Controller; CONTROLLERS], driver: &mut dyn Driver<D>) -> SystemFrame {
        let mut out = SystemFrame::default();
        if self.shell.stopped().is_some() {
            return out;
        }
        let mut shell_events = self.latch();
        let mut routed: [Vec<Event>; MAX_MACHINES] = Default::default();
        self.route(events, &mut shell_events, &mut routed);

        // The shell. The focus as routing left it (a home key included)
        // decides who reads the controllers this frame, the shell's and the
        // children's alike: a `focus` the shell makes now takes effect at
        // the next frame's routing (SPEC §machines.frames step 5).
        let focus = self.focus();
        let shell_controllers = if focus == 0 { controllers } else { NO_CONTROLLERS };
        out.shell = Some(driver.frame(Who { handle: 0, serial: 0 }, &mut self.shell, shell_events, shell_controllers));

        // Its starts and stops, in order; snapshots after the machines'
        // frames.
        let mut at_end = Vec::new();
        // A view whose machine is no longer a panel returns at once (SPEC
        // §machines.calls `face`).
        let list = self.shell.list3().into_owned();
        if let Some(dev) = self.shell.machines_mut() {
            dev.check_view(&list);
        }
        if let Some((builtins, commands)) = self.shell.machines_mut().map(|d| (d.builtins().to_vec(), d.take_commands())) {
            for command in commands {
                match command {
                    Command::Start { handle, .. } | Command::StartProgram { handle, .. } | Command::Restore { handle, .. } => {
                        let serial = self.next_serial;
                        self.next_serial += 1;
                        let i = handle as usize - 1;
                        if let Some(old) = self.children[i].take() {
                            driver.removed(old.serial);
                        }
                        let (image, identity, restore) = match &command {
                            Command::Start { image, .. } => (*image, None, None),
                            Command::StartProgram { program, .. } => ([0; 32], Some(program.clone()), None),
                            Command::Restore { slot, .. } => ([0; 32], None, Some(*slot)),
                            _ => unreachable!(),
                        };
                        self.children[i] = Some(Child {
                            serial,
                            image,
                            identity: identity.clone(),
                            machine: None,
                            title: String::new(),
                            error: None,
                            argument: None,
                            delivered: None,
                            restore,
                            snapshot: None,
                            last_used: 0,
                        });
                        match (&identity, restore) {
                            (_, Some(slot)) => {
                                if let Some(d) = driver.want_snapshot(serial, &self.shell, slot) {
                                    let child = self.children[i].as_mut().unwrap();
                                    child.delivered = Some(d.map(|(image, snapshot)| {
                                        child.snapshot = Some(snapshot);
                                        image
                                    }));
                                }
                            }
                            (None, None) => driver.want_image(serial, image),
                            (Some(program), None) => driver.want_program(serial, program, &builtins),
                        }
                    }
                    Command::Stop { handle } => {
                        if let Some(old) = self.children[handle as usize - 1].take() {
                            driver.removed(old.serial);
                        }
                    }
                    // A restored machine keeps the argument it had.
                    Command::Argument { handle, id } => {
                        if let Some(child) = self.children[handle as usize - 1].as_mut().filter(|c| c.restore.is_none()) {
                            child.argument = Some(id);
                        }
                    }
                    Command::Snapshot { .. } | Command::DeleteSnapshot { .. } => at_end.push(command),
                }
            }
            // Requests the shell read with `request` may be followed by
            // others.
            let dev = self.shell.machines().unwrap();
            for (i, slot) in self.children.iter_mut().enumerate() {
                let Some(machine) = slot.as_mut().and_then(|c| c.machine.as_mut()) else { continue };
                if machine.open_request().is_some_and(|r| r.1) && !dev.request_waiting(i as u8 + 1) {
                    machine.clear_open_request();
                }
            }
        }

        // The children.
        for i in 0..MAX_MACHINES {
            let h = i as u8 + 1;
            if self.state(h) != state::RUNNING {
                continue;
            }
            let Some(child) = self.children[i].as_mut() else { continue };
            let Some(machine) = child.machine.as_mut().filter(|m| m.stopped().is_none()) else { continue };
            let pads = if focus == h { controllers } else { NO_CONTROLLERS };
            let events = std::mem::take(&mut routed[i]);
            let result = driver.frame(Who { handle: h, serial: child.serial }, machine, events, pads);
            child.last_used = result.used.min(u32::MAX as u64) as u32;
            out.children.push((h, result));
        }

        // Snapshots and deletes, in the order the shell asked for them
        // (SPEC §machines.frames step 6).
        for command in at_end {
            match command {
                Command::Snapshot { handle, slot } => {
                    let Some(Ok(snapshot)) = self.children[handle as usize - 1].as_mut().and_then(|c| c.machine.as_mut()).map(Snapshot::try_of) else {
                        self.saved.push((slot, false));
                        continue;
                    };
                    self.saving.push((slot, snapshot.info()));
                    if let Some(stored) = driver.save_snapshot(&self.shell, slot, snapshot) {
                        self.saved.push((slot, stored));
                    }
                }
                Command::DeleteSnapshot { slot } => {
                    self.saving.retain(|s| s.0 != slot);
                    driver.delete_snapshot(&self.shell, slot);
                }
                _ => unreachable!(),
            }
        }
        out
    }

    /// Step 1 (SPEC §machines.frames): images delivered and machines that stopped
    /// become the handles' new states, and each record takes the machine's
    /// latest numbers. Returns the machine-changed events for the shell.
    fn latch(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let (Some(dev), like) = self.shell.machines_and_devices() else { return events };
        for (i, slot) in self.children.iter_mut().enumerate() {
            let h = i as u8 + 1;
            let Some(child) = slot.as_mut() else { continue };
            let (record_state, run) = (dev.slot(h).state, dev.slot(h).run);
            let mut new = None;
            match record_state {
                state::LOADING => match child.delivered.take() {
                    // A restore: the machine as it was (SPEC §machines.snapshots), with its
                    // image ID and identity.
                    Some(Ok(image)) if let Some(snapshot) = child.snapshot.take() => {
                        child.title = image.title_lossy();
                        child.image = image.id;
                        let s = dev.slot_mut(h);
                        s.image = image.id;
                        match snapshot.restore_with(like, child.serial, &image) {
                            Ok(mut machine) => {
                                machine.set_in_system(true);
                                child.identity = snapshot.identity.clone();
                                s.identity = snapshot.identity;
                                child.machine = Some(machine);
                                new = Some(if run { state::RUNNING } else { state::PAUSED });
                            }
                            Err(e) => {
                                child.error = Some(e.to_string());
                                // A device the machine had and this host
                                // lacks: as for `start` (SPEC §machines.calls).
                                s.failure = if matches!(e, crate::RestoreError::NeedsDevice(_)) { failure::NEEDS_DEVICE } else { failure::SNAPSHOT };
                                new = Some(state::FAILED);
                            }
                        }
                    }
                    Some(Ok(image)) => {
                        child.title = image.title_lossy();
                        child.image = image.id;
                        dev.slot_mut(h).image = image.id;
                        match like.new_machine(child.serial, &image, None, child.identity.as_ref()) {
                            Ok(mut machine) => {
                                machine.set_identity(child.identity.clone());
                                machine.set_in_system(true);
                                machine.set_argument(child.argument);
                                child.machine = Some(machine);
                                new = Some(if run { state::RUNNING } else { state::PAUSED });
                            }
                            Err(f) => {
                                child.error = Some(f.message);
                                dev.slot_mut(h).failure = f.reason;
                                new = Some(state::FAILED);
                            }
                        }
                    }
                    Some(Err(f)) => {
                        child.error = Some(f.message);
                        let s = dev.slot_mut(h);
                        s.failure = f.reason;
                        if let Some(id) = f.image {
                            child.image = id;
                            s.image = id;
                        }
                        new = Some(state::FAILED);
                    }
                    None => {}
                },
                state::RUNNING => {
                    let s = dev.slot_mut(h);
                    match child.machine.as_ref().and_then(|m| m.stopped()) {
                        Some(Stopped::Exit(code)) => {
                            s.exit_code = code;
                            new = Some(state::EXITED);
                        }
                        Some(Stopped::Fault(f)) => {
                            s.fault_kind = crate::machine::fault_number(&f);
                            s.fault_pc = f.pc;
                            new = Some(state::FAULTED);
                        }
                        None => {}
                    }
                }
                _ => {}
            }
            if let Some(state) = new {
                dev.set_state(h, state);
                events.push(Event::machine_changed(h, state));
            }
            if let Some(m) = &child.machine {
                let s = dev.slot_mut(h);
                s.units = child.last_used;
                s.frames = m.frame();
            }
        }
        // Open requests made in the last frame (SPEC §machines.frames), after the
        // changes of state.
        for (i, slot) in self.children.iter_mut().enumerate() {
            let Some(machine) = slot.as_mut().and_then(|c| c.machine.as_mut()) else { continue };
            if let Some(request) = machine.announce_open_request() {
                dev.set_request(i as u8 + 1, request);
                events.push(Event::open_requested(i as u8 + 1));
            }
        }
        // Snapshots stored (or not) since the last frame, and the slots
        // as they are now (SPEC §machines.frames).
        for (slot, stored) in std::mem::take(&mut self.saved) {
            if let Some(i) = self.saving.iter().position(|s| s.0 == slot) {
                let (_, info) = self.saving.remove(i);
                if stored {
                    dev.set_snapshot(slot, Some(info));
                }
            }
            events.push(Event::snapshot_done(slot, if stored { 1 } else { 3 }));
        }
        events
    }

    /// Step 2 (SPEC §machines.input): each input event goes to the shell or to one
    /// child, by the focus and the pictures as they are now.
    fn route(&mut self, events: &[Event], shell: &mut Vec<Event>, routed: &mut [Vec<Event>; MAX_MACHINES]) {
        let eye = self.eye;
        // A pointer over a machine's own Surface goes to the point of its
        // source rectangle (SPEC §vector3d.list Surface), the list as shown now.
        let own = |m: &Machine<D>, x: u16, y: u16| {
            let list = m.list3();
            if list.is_empty() { (x, y) } else { crate::vector3::surface_pointer(&list, eye, x, y) }
        };
        if !self.is_system() {
            for e in events.iter().filter(|e| e.kind != Event::HOME) {
                shell.push(match e.kind {
                    Event::POINTER => {
                        let (x, y, buttons) = e.pointer_parts();
                        let (x, y) = own(&self.shell, x, y);
                        Event::pointer(x, y, buttons)
                    }
                    _ => *e,
                });
            }
            return;
        }
        let pictures = self.pictures();
        let scene = self.scene(eye);
        for &e in events {
            match e.kind {
                Event::HOME => {
                    self.shell.machines_mut().unwrap().home();
                    shell.push(e);
                }
                Event::POINTER => {
                    let (x, y, buttons) = e.pointer_parts();
                    let pressed = buttons & !self.buttons != 0;
                    self.buttons = buttons;
                    // The topmost picture under the pointer: the highest
                    // handle; then the nearest panel (SPEC §machines.input).
                    let mut hit = pictures.iter().rev().find_map(|&(h, p)| Some((h, map_into(p, x, y)?)));
                    // The shell's own Surfaces are panels of its scene
                    // (handle 0) when it has one.
                    let mut on_shell = None;
                    if hit.is_none()
                        && let Some(scene) = &scene
                        && let Some((h, at)) = crate::vector3::panel_hit(&scene.panels, eye, x as f64 + 0.5, y as f64 + 0.5)
                    {
                        match at {
                            // Seen from behind: as if over no picture.
                            None => {}
                            // Over a moving scene: nobody.
                            Some(_) if scene.moving => continue,
                            Some(at) if h == 0 => on_shell = Some(at),
                            Some(at) => hit = Some((h, at)),
                        }
                    }
                    match hit {
                        None => {
                            let (x, y) = match (on_shell, &scene) {
                                (Some(at), _) => at,
                                (None, Some(_)) => (x, y),
                                (None, None) => own(&self.shell, x, y),
                            };
                            shell.push(Event::pointer(x, y, buttons));
                        }
                        Some((h, (cx, cy))) => {
                            if pressed {
                                shell.push(Event::machine_pressed(h, buttons));
                            }
                            if self.state(h) == state::RUNNING {
                                let machine = self.children[h as usize - 1].as_ref().and_then(|c| c.machine.as_ref());
                                let (cx, cy) = machine.map_or((cx, cy), |m| own(m, cx, cy));
                                routed[h as usize - 1].push(Event::pointer(cx, cy, buttons));
                            }
                        }
                    }
                }
                Event::KEY_DOWN | Event::KEY_UP | Event::TEXT | Event::WHEEL => match self.focus() {
                    0 => shell.push(e),
                    h if self.state(h) == state::RUNNING => routed[h as usize - 1].push(e),
                    _ => {}
                },
                _ => shell.push(e),
            }
        }
    }
}
