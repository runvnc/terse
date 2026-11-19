//! `terse-dev-page`: the page (SPEC 11, slot 7) as a driver (DEVICES.md):
//! terse-core's `Page` for the state; the layout events (height, anchor,
//! located) from terse-core's `PageInput`, laid out between frames with the
//! reference layout, or, when the core presents, with real fonts
//! (terse-host's typesetter, as `terse-host --screenshot` does); one layer
//! (z 20) drawn by terse-host's software page renderer.
//!
//! Options: `--fonts` or `--reference` force the layout; `--page-sans`,
//! `--page-serif`, `--page-mono` name the fonts.

use std::sync::{Arc, Mutex};

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerFile, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, EVENT_PAIR, LayerDecl, PosEvent, access, position, property, rule, z};
use terse_core::machine::copy_charge;
use terse_core::page::{MAX_DOCUMENT, Page, PageInput, ReferenceLayouter};
use terse_core::{ERR_UNKNOWN, Event};
use terse_devices::{clear, draw_page, rect, union};
use terse_host::page::{HostLayout, PageFonts, SyncLayouter, Typesetter};
use terse_host::soft::base::IRect;
use terse_host::soft::page::{PageKey, SoftPage};

const LAYER: u8 = 0;

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

struct PageDriver {
    page: Page,
    input: PageInput,
    latest: Option<Latest>,
    fonts: Option<bool>,
    page_fonts: PageFonts,
    soft: SoftPage,
    drawn: Option<Drawn>,
    replay: bool,
    presents: bool,
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

impl PageDriver {
    fn draw(&mut self, p: &Present, session: &Session) -> Result<PresentOut, String> {
        let outlines = p.properties.iter().any(|&(s, k, v)| s == 3 && k == property::OUTLINES && v != 0);
        self.soft.set_outlines(outlines);
        let layout = self.latest.as_ref().and_then(|l| l.lock().unwrap().clone());
        self.soft.set(Some(&self.page), layout);
        let mut fresh = false;
        if self.drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen) {
            self.drawn = Some(Drawn { picture: p.picture, screen: p.screen, layer: LayerFile::create(session.dir(), 7, LAYER, p.picture)?, key: None, scissor: IRect::EMPTY });
            fresh = true;
        }
        let d = self.drawn.as_mut().unwrap();
        let key = self.soft.key(p.screen, false);
        if !fresh && d.key.as_ref() == Some(&key) {
            return Ok(PresentOut { layers: vec![LayerOut { id: LAYER, new_file: None, damage: Vec::new() }], properties: Vec::new() });
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
        Ok(PresentOut {
            layers: vec![LayerOut { id: LAYER, new_file: fresh.then(|| d.layer.path.clone()), damage: if damage.is_empty() { vec![] } else { vec![rect(damage)] } }],
            properties: Vec::new(),
        })
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
                Ok(t) => {
                    let layouter = SyncLayouter::shared(Arc::new(Mutex::new(t)));
                    self.latest = Some(layouter.latest.clone());
                    self.input = PageInput::new(Box::new(layouter));
                }
                Err(e) => {
                    eprintln!("terse-dev-page: {e}; using the reference layout");
                    self.input = PageInput::new(Box::new(ReferenceLayouter::default()));
                }
            }
        }
        let layers = if self.presents { vec![LayerDecl { id: LAYER, z: z::PAGE }] } else { Vec::new() };
        DeviceInfo { version: 0, name: "terse-dev-page (Rust)".into(), native: (0, 0), layers, table: table() }
    }

    fn reset(&mut self) {
        self.page = Page::default();
    }

    fn call(&mut self, c: &CallIn, _: &mut dyn Mem) -> Reply {
        let [a0, a1, a2, a3, ..] = c.args;
        let (r, charge) = match c.function {
            0 => {
                let r = self.page.submit(c.buf);
                (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
            }
            1 => {
                self.page.clear();
                (0, 0)
            }
            2 => {
                self.page.set_region(a0, a1, a2, a3);
                (0, 0)
            }
            3 => {
                self.page.set_scroll(a0);
                (0, 0)
            }
            4 => {
                self.page.set_background(a0);
                (0, 0)
            }
            5 => (self.page.locate(a0), 0),
            _ => (ERR_UNKNOWN, 0),
        };
        Reply { a0: r, a1: None, charge }
    }

    fn begin(&mut self, b: &Begin, _: &mut dyn Mem) -> Inputs {
        // The last frame's locates are the layouter's now.
        self.page.begin_frame();
        let events = match b.replay {
            Some(record) => decode(record),
            None => {
                let all = self.input.events(Some(&self.page), b.host);
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

    fn end(&mut self, _: u32, _: bool) {
        if !self.replay {
            self.input.after_frame(Some(&self.page));
        }
    }

    fn state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.page.write_state(&mut out);
        out
    }

    fn restore(&mut self, bytes: &[u8]) -> bool {
        let mut p = Page::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        if p.restore(&mut r).is_err() || r.remaining() != 0 {
            return false;
        }
        let mut again = Vec::new();
        p.write_state(&mut again);
        if again != bytes {
            return false;
        }
        self.page = p;
        true
    }

    fn present(&mut self, p: &Present, session: &Session) -> PresentOut {
        if !self.presents {
            return PresentOut::default();
        }
        self.draw(p, session).unwrap_or_else(|e| {
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
    let mut soft = SoftPage::default();
    // Screenshots show the finished page: no scroll animation, figures at once.
    soft.set_animate(false);
    let mut d = PageDriver { page: Page::default(), input: PageInput::default(), latest: None, fonts, page_fonts, soft, drawn: None, replay: false, presents: false };
    terse_bus::driver::run(&mut d, "terse-dev-page")
}
