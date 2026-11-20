//! `terse-dev-page`: the page (SPEC §page, slot 7) as a driver (DEVICES.md):
//! per machine, terse-core's `Page` for the state and the layout events
//! (height, anchor, located) from terse-core's `PageInput`, laid out
//! between frames with the reference layout, or, when the core presents,
//! with real fonts (terse-host's typesetter, shared by every machine, as
//! `terse-host --screenshot` does); one layer (z 20) drawn by terse-host's
//! software page renderer. It subscribes to the vector display's
//! figures-as-outlines flag (SPEC §page.layout).
//!
//! Options: `--fonts` or `--reference` force the layout; `--page-sans`,
//! `--page-serif`, `--page-mono` name the fonts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerFile, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, EVENT_PAIR, LayerDecl, Open, PosEvent, SlotInfo, access, position, property, rule, z};
use terse_core::machine::copy_charge;
use terse_core::page::{MAX_DOCUMENT, Page, PageInput, ReferenceLayouter};
use terse_core::{ERR_UNKNOWN, Event};
use terse_devices::{clear, draw_page, rect, union};
use terse_host::page::{HostLayout, PageFonts, SyncLayouter, Typesetter};
use terse_host::soft::base::IRect;
use terse_host::soft::page::{PageKey, SoftPage};

const LAYER: u8 = 0;
const SLOT: u8 = 7;

fn table() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 1, MAX_DOCUMENT as u32, access::READ | access::ROM).results(1, terse_core::ERR_BUFFER).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(2).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(3).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(4).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(5),
    ]
}

struct Drawn {
    picture: (u32, u32),
    screen: (f32, f32, f32, f32),
    layer: LayerFile,
    key: Option<PageKey>,
    scissor: IRect,
}

type Latest = Arc<Mutex<Option<Arc<HostLayout>>>>;

/// One machine's page.
struct Inst {
    page: Page,
    input: PageInput,
    latest: Option<Latest>,
    soft: SoftPage,
    drawn: Option<Drawn>,
    outlines: bool,
}

struct PageDriver {
    fonts: Option<bool>,
    page_fonts: PageFonts,
    /// The fonts, shared by every machine's layouter.
    typesetter: Option<Arc<Mutex<Typesetter>>>,
    replay: bool,
    presents: bool,
    inst: HashMap<u32, Inst>,
}

/// The events of a frame as a record: 8 bytes each, as in INPUTS.
fn encode(events: &[PosEvent]) -> Vec<u8> {
    let mut out = Vec::with_capacity(events.len() * 8);
    for e in events {
        out.extend_from_slice(&[e.event.kind, e.event.a, e.event.b, e.event.c, e.position, e.flags]);
        out.extend_from_slice(&e.index.to_le_bytes());
    }
    out
}

fn decode(b: &[u8]) -> Vec<PosEvent> {
    b.chunks_exact(8)
        .map(|c| PosEvent { event: Event { kind: c[0], a: c[1], b: c[2], c: c[3] }, position: c[4], flags: c[5], index: u16::from_le_bytes([c[6], c[7]]) })
        .collect()
}

impl Inst {
    fn draw(&mut self, instance: u32, p: &Present, session: &Session) -> Result<PresentOut, String> {
        self.soft.set_outlines(self.outlines);
        let layout = self.latest.as_ref().and_then(|l| l.lock().unwrap().clone());
        self.soft.set(Some(&self.page), layout);
        let mut fresh = false;
        if self.drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen) {
            let layer = LayerFile::create(session.dir(), instance, SLOT, LAYER, p.picture)?;
            self.drawn = Some(Drawn { picture: p.picture, screen: p.screen, layer, key: None, scissor: IRect::EMPTY });
            fresh = true;
        }
        let d = self.drawn.as_mut().unwrap();
        let key = self.soft.key(p.screen, false);
        if !fresh && d.key.as_ref() == Some(&key) {
            return Ok(PresentOut { layers: vec![LayerOut { slot: SLOT, id: LAYER, new_file: None, damage: Vec::new() }] });
        }
        self.soft.prepare(p.screen, p.picture, false);
        let scissor = self.soft.quads.scissor;
        let damage = if fresh { IRect::of_size(p.picture) } else { union(d.scissor, scissor) };
        let w = p.picture.0 as usize;
        let px = d.layer.pixels();
        if !damage.is_empty() {
            clear(px, w, damage);
            draw_page(&self.soft.quads, px, w, damage);
        }
        // The key after preparing (figures that arrived count).
        d.key = Some(self.soft.key(p.screen, false));
        d.scissor = scissor;
        let damage = if damage.is_empty() { vec![] } else { vec![rect(damage)] };
        Ok(PresentOut { layers: vec![LayerOut { slot: SLOT, id: LAYER, new_file: fresh.then(|| d.layer.path.clone()), damage }] })
    }
}

impl Device for PageDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        self.replay = session.replay();
        self.presents = session.presents();
        // Real fonts when the picture is shown (as terse-host lays out),
        // the reference layout otherwise (as terse run does).
        if self.fonts.unwrap_or(self.presents) && !self.replay {
            match Typesetter::new(&self.page_fonts) {
                Ok(t) => self.typesetter = Some(Arc::new(Mutex::new(t))),
                Err(e) => eprintln!("terse-dev-page: {e}; using the reference layout"),
            }
        }
        let layers = if self.presents { vec![LayerDecl { id: LAYER, z: z::PAGE }] } else { Vec::new() };
        DeviceInfo {
            name: "terse-dev-page (Rust)".into(),
            native: (0, 0),
            slots: vec![SlotInfo { slot: SLOT, version: 0, layers, table: table() }],
            publishes: Vec::new(),
            subscribes: vec![property::OUTLINES.into()],
            services: Vec::new(),
        }
    }

    fn open(&mut self, instance: u32, _: &Open, _: &Session) -> Result<(), String> {
        let (input, latest) = match &self.typesetter {
            Some(t) => {
                let layouter = SyncLayouter::shared(t.clone());
                let latest = layouter.latest.clone();
                (PageInput::new(Box::new(layouter)), Some(latest))
            }
            None => (PageInput::new(Box::new(ReferenceLayouter::default())), None),
        };
        let mut soft = SoftPage::default();
        // Pictures show the finished page: no scroll animation, figures at once.
        soft.set_animate(false);
        self.inst.insert(instance, Inst { page: Page::default(), input, latest, soft, drawn: None, outlines: false });
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.inst.remove(&instance);
    }

    fn reset(&mut self, instance: u32) {
        if let Some(i) = self.inst.get_mut(&instance) {
            i.page = Page::default();
        }
    }

    fn call(&mut self, instance: u32, c: &CallIn, _: &mut dyn Mem) -> Reply {
        let Some(i) = self.inst.get_mut(&instance) else { return Reply { a0: ERR_UNKNOWN, ..Default::default() } };
        let page = &mut i.page;
        let [a0, a1, a2, a3, ..] = c.args;
        let (r, charge) = match c.function {
            0 => {
                let r = page.submit(c.buf);
                (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
            }
            1 => {
                page.clear();
                (0, 0)
            }
            2 => {
                page.set_region(a0, a1, a2, a3);
                (0, 0)
            }
            3 => {
                page.set_scroll(a0);
                (0, 0)
            }
            4 => {
                page.set_background(a0);
                (0, 0)
            }
            5 => (page.locate(a0), 0),
            _ => (ERR_UNKNOWN, 0),
        };
        Reply { a0: r, a1: None, charge }
    }

    fn begin(&mut self, instance: u32, b: &Begin, _: &mut dyn Mem) -> Inputs {
        let Some(i) = self.inst.get_mut(&instance) else { return Inputs::default() };
        // The last frame's locates are the layouter's now.
        i.page.begin_frame();
        let events = match b.replay {
            Some(record) => decode(record),
            None => {
                let all = i.input.events(Some(&i.page), b.host);
                let mut next_host = 0;
                let mut out = Vec::new();
                for e in all {
                    if next_host < b.host.len() && e == b.host[next_host] {
                        next_host += 1;
                        continue;
                    }
                    let (position, index) = if next_host == 0 { (position::BEFORE, 0) } else { (position::AFTER_HOST, next_host as u16 - 1) };
                    let flags = if e.kind == Event::PAGE_LOCATED { EVENT_PAIR } else { 0 };
                    out.push(PosEvent { event: e, position, flags, index });
                }
                out
            }
        };
        Inputs { charge: 0, record: encode(&events), events }
    }

    fn end(&mut self, instance: u32, _: u32, _: bool) {
        if let Some(i) = self.inst.get_mut(&instance)
            && !self.replay
        {
            i.input.after_frame(Some(&i.page));
        }
    }

    fn state(&self, instance: u32, _: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(i) = self.inst.get(&instance) {
            i.page.write_state(&mut out);
        }
        out
    }

    fn restore(&mut self, instance: u32, _: u8, bytes: &[u8]) -> Option<usize> {
        let i = self.inst.get_mut(&instance)?;
        let mut p = Page::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        p.restore(&mut r).ok()?;
        let n = r.position();
        let mut again = Vec::new();
        p.write_state(&mut again);
        (again == bytes[..n]).then_some(())?;
        i.page = p;
        Some(n)
    }

    fn set(&mut self, instance: u32, name: &str, value: &[u8]) {
        if let Some(i) = self.inst.get_mut(&instance)
            && name == property::OUTLINES
        {
            i.outlines = value.first().is_some_and(|&v| v != 0);
        }
    }

    fn present(&mut self, instance: u32, p: &Present, session: &Session) -> PresentOut {
        if !self.presents {
            return PresentOut::default();
        }
        let Some(i) = self.inst.get_mut(&instance) else { return PresentOut::default() };
        i.draw(instance, p, session).unwrap_or_else(|e| {
            eprintln!("terse-dev-page: {e}");
            PresentOut::default()
        })
    }
}

fn main() -> std::process::ExitCode {
    let mut fonts = None;
    let mut page_fonts = PageFonts::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--fonts" => fonts = Some(true),
            "--reference" => fonts = Some(false),
            "--page-sans" => page_fonts.sans = args.next(),
            "--page-serif" => page_fonts.serif = args.next(),
            "--page-mono" => page_fonts.mono = args.next(),
            _ => {
                eprintln!("terse-dev-page: unknown option {a}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    let mut d = PageDriver { fonts, page_fonts, typesetter: None, replay: false, presents: false, inst: HashMap::new() };
    terse_bus::driver::run(&mut d, "terse-dev-page")
}
