//! `terse-dev-vector`: the 2D vector display (SPEC 10, slot 3) as a driver
//! (DEVICES.md): terse-core's `Vector` for the state, one layer (z 30)
//! drawn with terse-host's shapes in the clean style, and the
//! figures-as-outlines property the page needs.

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerFile, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, LayerDecl, access, property, rule, z};
use terse_core::machine::copy_charge;
use terse_core::{ERR_UNKNOWN, Vector};
use terse_devices::{clear, over4, rect, scissor, union};
use terse_host::display::decode_shapes;
use terse_host::soft::base::IRect;
use terse_host::soft::shapes::{Geom, Mode, Shape, visit};

const LAYER: u8 = 0;

fn table() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 1, terse_core::vector::MAX_LIST as u32, access::READ).results(1, 1).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(2).batch(rule::AT_MOST, 0, 1, 0, 0),
        e(3).batch(rule::AT_MOST, 0, 255, 0, 0),
        e(4).batch(rule::AT_MOST, 0, 1, 0, 0),
    ]
}

struct Drawn {
    picture: (u32, u32),
    screen: (f32, f32, f32, f32),
    layer: LayerFile,
    shapes: Vec<Shape>,
    bounds: Vec<IRect>,
}

struct VectorDriver {
    v: Vector,
    presents: bool,
    drawn: Option<Drawn>,
}

impl VectorDriver {
    fn draw(&mut self, p: &Present, session: &Session) -> Result<PresentOut, String> {
        let outlines = self.v.outlined() || self.v.phosphor();
        let props = vec![(property::OUTLINES, outlines as u32)];
        let mut fresh = false;
        if self.drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen) {
            self.drawn = Some(Drawn { picture: p.picture, screen: p.screen, layer: LayerFile::create(session.dir(), 3, LAYER, p.picture)?, shapes: Vec::new(), bounds: Vec::new() });
            fresh = true;
        }
        let d = self.drawn.as_mut().unwrap();
        let geom = Geom::for_rect(p.screen);
        let mut prims = Vec::new();
        decode_shapes(self.v.list(), outlines, &mut prims);
        let shapes: Vec<Shape> = prims.iter().map(|s| Shape::new(s, 0)).collect();
        if !fresh && shapes == d.shapes {
            return Ok(PresentOut { layers: vec![LayerOut { id: LAYER, new_file: None, damage: Vec::new() }], properties: props });
        }
        let bounds: Vec<IRect> = shapes.iter().map(|s| s.bounds(&geom, false)).collect();
        let clip_all = scissor(p.screen, p.picture);
        let mut damage = if fresh { IRect::of_size(p.picture) } else { IRect::EMPTY };
        for b in d.bounds.iter().chain(&bounds) {
            damage = union(damage, *b);
        }
        let damage = damage.intersect(&IRect::of_size(p.picture));
        let w = p.picture.0 as usize;
        let px = d.layer.pixels();
        if !damage.is_empty() {
            clear(px, w, damage);
            let clip = damage.intersect(&clip_all);
            for (s, b) in shapes.iter().zip(&bounds) {
                if b.intersect(&clip).is_empty() {
                    continue;
                }
                let [r, g, bl, a] = s.rgba();
                if a == 0 {
                    continue;
                }
                let c = [r as f32, g as f32, bl as f32];
                let alpha = a as f32 / 255.0;
                visit(s, &geom, Mode::Cover, clip, |x, y, v| {
                    let q = &mut px[y as usize * w + x as usize];
                    *q = over4(*q, c, alpha * v);
                });
            }
        }
        d.shapes = shapes;
        d.bounds = bounds;
        Ok(PresentOut {
            layers: vec![LayerOut { id: LAYER, new_file: fresh.then(|| d.layer.path.clone()), damage: if damage.is_empty() { vec![] } else { vec![rect(damage)] } }],
            properties: props,
        })
    }
}

impl Device for VectorDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        self.presents = session.presents();
        let layers = if self.presents { vec![LayerDecl { id: LAYER, z: z::VECTOR }] } else { Vec::new() };
        DeviceInfo { version: 0, name: "terse-dev-vector (Rust)".into(), native: (0, 0), layers, table: table() }
    }

    fn reset(&mut self) {
        self.v = Vector::default();
    }

    fn call(&mut self, c: &CallIn, _: &mut dyn Mem) -> Reply {
        let (a0, charge) = match c.function {
            0 => {
                let r = self.v.submit(c.buf);
                (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
            }
            1 => {
                self.v.clear();
                (0, 0)
            }
            2 => (self.v.set_style(c.args[0]), 0),
            3 => (self.v.set_persistence(c.args[0]), 0),
            4 => (self.v.set_fill(c.args[0]), 0),
            _ => (ERR_UNKNOWN, 0),
        };
        Reply { a0, a1: None, charge }
    }

    fn begin(&mut self, _: &Begin, _: &mut dyn Mem) -> Inputs {
        Inputs::default()
    }

    fn end(&mut self, _: u32, _: bool) {}

    fn state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.v.write_state(&mut out);
        out
    }

    fn restore(&mut self, bytes: &[u8]) -> bool {
        let mut v = Vector::default();
        let mut r = terse_core::snapshot::Reader::new(bytes);
        if v.restore(&mut r).is_err() || r.remaining() != 0 {
            return false;
        }
        let mut again = Vec::new();
        v.write_state(&mut again);
        if again != bytes {
            return false;
        }
        self.v = v;
        true
    }

    fn present(&mut self, p: &Present, session: &Session) -> PresentOut {
        if !self.presents {
            let outlines = self.v.outlined() || self.v.phosphor();
            return PresentOut { layers: Vec::new(), properties: vec![(property::OUTLINES, outlines as u32)] };
        }
        self.draw(p, session).unwrap_or_else(|e| {
            eprintln!("terse-dev-vector: {e}");
            PresentOut::default()
        })
    }
}

fn main() -> std::process::ExitCode {
    terse_bus::driver::run(&mut VectorDriver { v: Vector::default(), presents: false, drawn: None }, "terse-dev-vector")
}
