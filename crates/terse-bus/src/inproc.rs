//! The same devices compiled into the core (terse-core's [`InProcess`]),
//! with terse-net's content service and the reference page layout as the
//! host's side, behind the same [`Devices`] trait as the bus: for comparing
//! the two with everything else equal (`terse-bus --in-process`).

use terse_core::devices::{CallOutcome, Devices, FrameInputs, GuestMem, InProcess, PosEvent};
use terse_core::page::PageInput;
use terse_core::{Completion, Event, FrameInput, Image};
use terse_net::{ContentService, Owner};

use crate::proto::{EVENT_PAIR, position};

pub struct InProcessHost {
    image_id: [u8; 32],
    pub devices: InProcess,
    page_input: PageInput,
    service: Option<ContentService>,
    completions: Vec<Completion>,
    feeds_updated: bool,
}

impl InProcessHost {
    pub fn new(image: &Image, service: Option<ContentService>) -> Self {
        InProcessHost {
            image_id: image.id,
            devices: InProcess::new(image),
            page_input: PageInput::default(),
            service,
            completions: Vec::new(),
            feeds_updated: false,
        }
    }
}

impl Devices for InProcessHost {
    fn begin_frame(&mut self, frame: u32, input: &FrameInput, mem: GuestMem) -> Result<FrameInputs, String> {
        if !input.completions.is_empty() {
            return Err("the content service gives the completions".into());
        }
        let completions = std::mem::take(&mut self.completions);
        let mut out = self.devices.begin_frame(frame, &FrameInput { completions: &completions, ..*input }, mem)?;
        // The page's events among the host's: before the first host event,
        // or after host event i.
        let host = input.events;
        let all = self.page_input.events(self.devices.page(), host);
        let mut next_host = 0;
        let mut placed = Vec::new();
        for e in all {
            if next_host < host.len() && e == host[next_host] {
                next_host += 1;
                continue;
            }
            let (position, index) = if next_host == 0 { (position::BEFORE, 0) } else { (position::AFTER_HOST, next_host as u16 - 1) };
            let flags = if e.kind == Event::PAGE_LOCATED { EVENT_PAIR } else { 0 };
            placed.push(PosEvent { event: e, position, flags, index });
        }
        if std::mem::take(&mut self.feeds_updated) {
            placed.push(PosEvent { event: Event::feeds_updated(), position: position::AFTER_INPUT, flags: 0, index: 0 });
        }
        placed.append(&mut out.events);
        out.events = placed;
        Ok(out)
    }

    fn idle_frame(&mut self) {
        self.devices.idle_frame();
    }

    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], budget: i64, mem: GuestMem) -> Result<CallOutcome, String> {
        self.devices.call(slot, function, args, budget, mem)
    }

    fn end_frame(&mut self, frame: u32, stopped: bool, mem: GuestMem) -> Result<(), String> {
        self.devices.end_frame(frame, stopped, mem)?;
        self.page_input.after_frame(self.devices.page());
        if let Some(service) = &mut self.service {
            service.submit_owned(0, Owner::Image(self.image_id), self.devices.take_content_requests());
            self.completions = service.wait_finished();
            self.feeds_updated = service.feeds_updated();
        }
        Ok(())
    }

    fn write_state(&mut self, slot: u8, out: &mut Vec<u8>) -> Result<(), String> {
        self.devices.write_state(slot, out)
    }

    fn list3(&self) -> std::borrow::Cow<'_, [u8]> {
        self.devices.list3()
    }

    fn restore_state(&mut self, slot: u8, bytes: &[u8], ram_len: usize) -> Result<usize, terse_core::RestoreError> {
        self.devices.restore_state(slot, bytes, ram_len)
    }

    fn new_machine(&self, _serial: u32, image: &Image, identity: Option<&terse_core::ProgramId>) -> Result<terse_core::Machine<Self>, terse_core::StartFailure> {
        let memory = terse_core::devices::Memory::Owned { rom: image.rom.clone(), ram: vec![0; image.ram_pages as usize * 8192] };
        let mut m = terse_core::Machine::with_devices(image, memory, InProcessHost::new(image, None));
        m.set_identity(identity.cloned());
        Ok(m)
    }
}

/// Where the content device's data is, as `terse run` takes it.
#[derive(Clone, Debug, Default)]
pub struct ContentOptions {
    pub store: Option<std::path::PathBuf>,
    pub config: Option<std::path::PathBuf>,
    pub data: Option<std::path::PathBuf>,
    pub relays: Vec<String>,
    /// How requests that need consent are answered (SPEC 9.5): deny (the
    /// default headless), allow, or ask (the core shows the question).
    pub consent: Option<terse_net::Consent>,
    pub reading: bool,
    pub no_feed_relays: bool,
    pub no_program_download: bool,
}

impl ContentOptions {
    /// Takes `--store`, `--config`, `--data`, `--relay`, `--consent`,
    /// `--profile reading`, `--no-feed-relays`, `--no-program-download`
    /// from `args` (returning true) or not (false: not one of them).
    pub fn take(&mut self, arg: &str, value: &mut dyn FnMut() -> Result<String, String>) -> Result<bool, String> {
        match arg {
            "--store" => self.store = Some(value()?.into()),
            "--config" => self.config = Some(value()?.into()),
            "--data" => self.data = Some(value()?.into()),
            "--relay" => self.relays.push(value()?),
            "--consent" => {
                let c = value()?;
                self.consent = Some(terse_net::Consent::parse(&c).ok_or(format!("bad --consent {c} (allow, deny or ask)"))?);
            }
            "--profile" => match value()?.as_str() {
                "reading" => self.reading = true,
                p => return Err(format!("unknown profile {p} (the only one is reading)")),
            },
            "--no-feed-relays" => self.no_feed_relays = true,
            "--no-program-download" => self.no_program_download = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The same options as arguments, for a content driver's command line.
    pub fn args(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (k, p) in [("--store", &self.store), ("--config", &self.config), ("--data", &self.data)] {
            if let Some(p) = p {
                v.extend([k.to_string(), p.display().to_string()]);
            }
        }
        for r in &self.relays {
            v.extend(["--relay".to_string(), r.clone()]);
        }
        let consent = match self.consent {
            Some(terse_net::Consent::Allow) => "allow",
            Some(terse_net::Consent::Ask) => "ask",
            _ => "deny",
        };
        v.extend(["--consent".to_string(), consent.to_string()]);
        if self.reading {
            v.extend(["--profile".to_string(), "reading".to_string()]);
        }
        if self.no_feed_relays {
            v.push("--no-feed-relays".into());
        }
        if self.no_program_download {
            v.push("--no-program-download".into());
        }
        v
    }

    /// The content service `terse run` would start: the store with the
    /// manual, the config's subscriptions, identity and nicknames, the
    /// data directory's records; caught up with the relays first.
    pub fn start(&self) -> Result<ContentService, String> {
        use terse_net::{Node, Records, Subscriptions, builtin, local, profile::Nicknames};
        let relays = if self.relays.is_empty() { Vec::new() } else { local::find_relays(&self.relays, None, 0)?.urls() };
        let mut node = Node::open(self.store.clone(), &relays, terse_net::relay::DEFAULT_TIMEOUT)?;
        node.set_feed_relays(!self.no_feed_relays);
        builtin::install_manual(node.store())?;
        let config = self.config.clone().or_else(terse_net::default_config_dir).ok_or("no config directory (use --config)")?;
        let data = self.data.clone().or_else(terse_net::default_data_dir).ok_or("no data directory (use --data)")?;
        node.set_subscriptions(Subscriptions::load(config.join("subscriptions"))?);
        node.set_identity_path(config.join("identity.key"))?;
        node.set_nicknames(Nicknames::load_from_config(&config)?);
        node.set_records(Records::in_data_dir(&data));
        node.set_snapshots(terse_net::Snapshots::in_data_dir(&data));
        node.set_program_download(!self.no_program_download);
        let consent = self.consent.unwrap_or(terse_net::Consent::Deny);
        let mut content = ContentService::start_with_consent(node, consent);
        content.set_reading_profile(self.reading);
        if content.following()
            && let Err(e) = content.wait_caught_up(std::time::Duration::from_secs(60))
        {
            eprintln!("terse-bus: feeds: {e}; running with the entries held locally");
        }
        Ok(content)
    }
}
