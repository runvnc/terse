//! The same devices compiled into the core (terse-core's terminal, vector
//! display, content and page, and terse-net's content service), behind the
//! same [`Devices`] trait as the bus: for comparing the two with
//! everything else equal (`terse-bus --in-process`).

use terse_core::content::{Content, ram_range, readable};
use terse_core::machine::{call, charge, copy_charge};
use terse_core::page::{Page, PageInput};
use terse_core::system::System;
use terse_core::terminal::{CELL_BYTES, CELLS};
use terse_core::{COLS, Completion, ERR_BUFFER, ERR_UNKNOWN, Event, Image, ROWS, Terminal, Vector};
use terse_net::{ContentService, Owner};

use crate::core::{CallOutcome, Devices, FrameInputs, GuestMem};
use crate::proto::{EVENT_PAIR, PosEvent, position};

pub struct InProcess {
    image_id: [u8; 32],
    terminal: Option<Terminal>,
    vector: Option<Vector>,
    content: Option<Content>,
    page: Option<Page>,
    page_input: PageInput,
    service: Option<ContentService>,
    completions: Vec<Completion>,
    feeds_updated: bool,
}

impl InProcess {
    pub fn new(image: &Image, service: Option<ContentService>) -> Self {
        InProcess {
            image_id: image.id,
            terminal: image.has_device(2).then(Terminal::default),
            vector: image.has_device(3).then(Vector::default),
            content: image.has_device(4).then(Content::default),
            page: image.has_device(7).then(Page::default),
            page_input: PageInput::default(),
            service,
            completions: Vec::new(),
            feeds_updated: false,
        }
    }
}

impl Devices for InProcess {
    fn begin_frame(&mut self, _frame: u32, _time: u32, host: &[Event], replay: Option<&[(u8, Vec<u8>)]>, mem: GuestMem) -> Result<FrameInputs, String> {
        if replay.is_some() {
            return Err("in-process devices do not replay".into());
        }
        let mut out = FrameInputs::default();
        if let Some(page) = &mut self.page {
            page.begin_frame();
        }
        // The page's events among the host's: before the first host event,
        // or after host event i.
        let all = self.page_input.events(self.page.as_ref(), host);
        let mut next_host = 0;
        for e in all {
            if next_host < host.len() && e == host[next_host] {
                next_host += 1;
                continue;
            }
            let (position, index) = if next_host == 0 { (position::BEFORE, 0) } else { (position::AFTER_HOST, next_host as u16 - 1) };
            let flags = if e.kind == Event::PAGE_LOCATED { EVENT_PAIR } else { 0 };
            out.events.push(PosEvent { event: e, position, flags, index });
        }
        if std::mem::take(&mut self.feeds_updated) {
            out.events.push(PosEvent { event: Event::feeds_updated(), position: position::AFTER_INPUT, flags: 0, index: 0 });
        }
        if let Some(content) = &mut self.content {
            let mut completions = content.take_orphans();
            completions.append(&mut self.completions);
            let mut sys = System::default();
            let (_, _, delivered) = content.complete(&completions, mem.ram, &mut sys);
            out.charge = copy_charge(delivered);
            for &e in sys.queue() {
                out.events.push(PosEvent { event: e, position: position::COMPLETION, flags: 0, index: 0 });
            }
        }
        Ok(out)
    }

    fn call(&mut self, slot: u8, function: u8, args: [u32; 6], budget: i64, mem: GuestMem) -> Result<CallOutcome, String> {
        let n = (slot as u32) << 8 | function as u32;
        let [a0, a1, a2, a3, ..] = args;
        let (rom, ram) = (mem.rom, mem.ram);
        let mut units = 0;
        let mut r1 = None;
        let r0 = match slot {
            2 => {
                let t = self.terminal.as_mut().unwrap();
                match n {
                    call::TERM_WRITE => match readable(rom, ram, a0, a1 as usize) {
                        None => ERR_BUFFER,
                        Some(bytes) => {
                            let (done, charged) = t.write_limited(bytes, budget, charge::WRITE_BYTE, charge::SCROLL_CLEAR);
                            units += charged;
                            done as u32
                        }
                    },
                    call::TERM_COMMAND => {
                        if a0 < 0x100 {
                            units += t.command(a0 as u8) * charge::SCROLL_CLEAR;
                        }
                        0
                    }
                    call::TERM_SET_CURSOR => {
                        t.set_cursor(a0, a1);
                        0
                    }
                    call::TERM_GET_CURSOR => {
                        let (col, row) = t.cursor();
                        r1 = Some(row as u32);
                        col as u32
                    }
                    call::TERM_SET_COLOURS => {
                        let (fg, bg) = t.colors();
                        t.set_colours(u8::try_from(a0).unwrap_or(fg), u8::try_from(a1).unwrap_or(bg));
                        0
                    }
                    call::TERM_GET_COLOURS => {
                        let (fg, bg) = t.colors();
                        r1 = Some(bg as u32);
                        fg as u32
                    }
                    call::TERM_CELLS_READ => {
                        let len = if a2 as usize <= CELLS { a2 as usize * CELL_BYTES } else { usize::MAX };
                        match ram_range(ram.len(), a1, len).filter(|_| (a0 as usize) < CELLS && a2 as usize <= CELLS) {
                            None => ERR_BUFFER,
                            Some(at) => {
                                t.cells_read(a0 as usize, &mut ram[at..at + len]);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_CELLS_WRITE => {
                        let len = if a2 as usize <= CELLS { a2 as usize * CELL_BYTES } else { usize::MAX };
                        match readable(rom, ram, a1, len).filter(|_| (a0 as usize) < CELLS && a2 as usize <= CELLS) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.cells_write(a0 as usize, bytes);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_PALETTE_SET => {
                        let len = if a2 <= 256 { a2 as usize * 3 } else { usize::MAX };
                        match readable(rom, ram, a1, len).filter(|_| a0 < 256 && a2 <= 256) {
                            None => ERR_BUFFER,
                            Some(bytes) => {
                                t.palette_set(a0 as usize, bytes);
                                units += copy_charge(len);
                                a2
                            }
                        }
                    }
                    call::TERM_SIZE => {
                        r1 = Some(ROWS as u32);
                        COLS as u32
                    }
                    _ => ERR_UNKNOWN,
                }
            }
            3 => {
                let v = self.vector.as_mut().unwrap();
                match n {
                    call::VEC_SUBMIT => match ram_range(ram.len(), a0, a1 as usize) {
                        Some(at) if v.submit(&ram[at..at + a1 as usize]) == terse_core::vector::RESULT_DONE => {
                            units += copy_charge(a1 as usize);
                            terse_core::vector::RESULT_DONE
                        }
                        _ => terse_core::vector::RESULT_REJECTED,
                    },
                    call::VEC_CLEAR => {
                        v.clear();
                        0
                    }
                    call::VEC_STYLE => v.set_style(a0),
                    call::VEC_PERSISTENCE => v.set_persistence(a0),
                    call::VEC_FILL => v.set_fill(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            4 if n == call::CONTENT_SUBMIT => {
                let (handle, taken) = self.content.as_mut().unwrap().submit(a0, rom, ram);
                units += copy_charge(taken);
                handle
            }
            7 => {
                let page = self.page.as_mut().unwrap();
                match n {
                    call::PAGE_SUBMIT => {
                        if a1 == 0 || a1 as usize > terse_core::page::MAX_DOCUMENT {
                            terse_core::page::RESULT_REJECTED
                        } else {
                            match readable(rom, ram, a0, a1 as usize) {
                                None => ERR_BUFFER,
                                Some(bytes) => {
                                    units += copy_charge(bytes.len());
                                    page.submit(bytes)
                                }
                            }
                        }
                    }
                    call::PAGE_CLEAR => {
                        page.clear();
                        0
                    }
                    call::PAGE_REGION => {
                        page.set_region(a0, a1, a2, a3);
                        0
                    }
                    call::PAGE_SCROLL => {
                        page.set_scroll(a0);
                        0
                    }
                    call::PAGE_BACKGROUND => {
                        page.set_background(a0);
                        0
                    }
                    call::PAGE_LOCATE => page.locate(a0),
                    _ => ERR_UNKNOWN,
                }
            }
            _ => ERR_UNKNOWN,
        };
        Ok(CallOutcome { a0: r0, a1: r1, charge: units })
    }

    fn end_frame(&mut self, _frame: u32, _stopped: bool, _mem: GuestMem) -> Result<(), String> {
        self.page_input.after_frame(self.page.as_ref());
        if let (Some(content), Some(service)) = (&mut self.content, &mut self.service) {
            service.submit_owned(0, Owner::Image(self.image_id), content.take_requests());
            self.completions = service.wait_finished();
            self.feeds_updated = service.feeds_updated();
        }
        Ok(())
    }

    fn state(&mut self, slot: u8) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        match slot {
            2 => self.terminal.as_ref().ok_or("no terminal")?.write_state(&mut out),
            3 => self.vector.as_ref().ok_or("no vector display")?.write_state(&mut out),
            4 => self.content.as_ref().ok_or("no content device")?.write_state(&mut out),
            7 => self.page.as_ref().ok_or("no page")?.write_state(&mut out),
            _ => return Err(format!("no state section for slot {slot}")),
        }
        Ok(out)
    }
}

/// Where the content device's data is, as `terse run` takes it.
#[derive(Clone, Debug, Default)]
pub struct ContentOptions {
    pub store: Option<std::path::PathBuf>,
    pub config: Option<std::path::PathBuf>,
    pub data: Option<std::path::PathBuf>,
    pub relays: Vec<String>,
    pub consent_allow: bool,
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
                self.consent_allow = match value()?.as_str() {
                    "allow" => true,
                    "deny" => false,
                    c => return Err(format!("bad --consent {c} (allow or deny)")),
                }
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
        v.extend(["--consent".to_string(), if self.consent_allow { "allow" } else { "deny" }.to_string()]);
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
        node.set_program_download(!self.no_program_download);
        let consent = if self.consent_allow { terse_net::Consent::Allow } else { terse_net::Consent::Deny };
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
