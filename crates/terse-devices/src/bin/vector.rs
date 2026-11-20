//! `terse-dev-vector`: the 2D and 3D vector displays (SPEC §vector, slot 3;
//! SPEC §vector3d, slot 9) as one driver (DEVICES.md 1.1: one process serves
//! both slots, because device 9's `style` and `persistence` are device
//! 3's, SPEC §vector3d.calls, one section of the state). terse-core's `Vector` and
//! `Vector3` for the state; per machine two layers drawn with terse-host's
//! shapes in the clean style: the 2D list (z 30) and the 3D list's
//! segments seen from the eye (z 35). It publishes the figures-as-outlines
//! flag the page needs and device 9's list, which the core needs for
//! device 8 and the shell's scene; it subscribes to the eye and to the
//! view of the shell's scene (DEVICES.md 7.2).

use std::collections::HashMap;

use terse_bus::driver::{Begin, CallIn, Device, Inputs, LayerFile, LayerOut, Mem, Present, PresentOut, Reply, Session};
use terse_bus::proto::{CallEntry, DeviceInfo, LayerDecl, Open, SlotInfo, access, property, rule, z};
use terse_core::machine::copy_charge;
use terse_core::vector3::{SLOT as SLOT3, SceneView, Transform};
use terse_core::{ERR_UNKNOWN, Primitive, Vector, Vector3};
use terse_devices::{clear, over4, rect, scissor, union};
use terse_host::display::decode_shapes;
use terse_host::soft::base::IRect;
use terse_host::soft::shapes::{Geom, Mode, Shape, visit};

const LAYER: u8 = 0;

fn table2() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 1, terse_core::vector::MAX_LIST as u32, access::READ).results(1, 1).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(2).batch(rule::AT_MOST, 0, 1, 0, 0),
        e(3).batch(rule::AT_MOST, 0, 255, 0, 0),
        e(4).batch(rule::AT_MOST, 0, 1, 0, 0),
    ]
}

/// Device 9 (SPEC §vector3d.calls): `submit` (1 to 16,384 bytes in RAM; both
/// failures 1), `clear`, and `style` and `persistence`, device 3's.
fn table3() -> Vec<CallEntry> {
    let e = CallEntry::plain;
    vec![
        e(0).buffer(0, 1, 1, 1, terse_core::vector3::MAX_LIST as u32, access::READ).results(1, 1).batch(rule::CONSTANT, 0, 0, 0, 0).per4(),
        e(1).batch(rule::CONSTANT, 0, 0, 0, 0),
        e(2).batch(rule::AT_MOST, 0, 1, 0, 0),
        e(3).batch(rule::AT_MOST, 0, 255, 0, 0),
    ]
}

/// A layer and the shapes drawn in it.
struct Drawn {
    picture: (u32, u32),
    screen: (f32, f32, f32, f32),
    layer: LayerFile,
    shapes: Vec<Shape>,
    bounds: Vec<IRect>,
}

/// One machine's displays.
struct Inst {
    /// Device 3, there for the shared style also when only 9 is listed.
    v: Vector,
    v3: Option<Vector3>,
    lists2: bool,
    eye: (f32, f32),
    view: SceneView,
    drawn: [Option<Drawn>; 2],
}

impl Inst {
    fn outlines(&self) -> bool {
        self.v.outlined() || self.v.phosphor()
    }

    /// The primitives of layer `k` (0: the 2D list; 1: the 3D segments).
    fn primitives(&self, k: usize) -> Vec<Primitive> {
        let mut out = Vec::new();
        match k {
            0 => decode_shapes(self.v.list(), self.outlines(), &mut out),
            _ => {
                if let Some(v3) = self.v3.as_ref().filter(|v| !v.list().is_empty()) {
                    terse_core::vector3::project_scene(v3.list(), self.eye, &self.view, &mut out, &mut Vec::new(), &mut Vec::new());
                }
            }
        }
        out
    }
}

struct VectorDriver {
    presents: bool,
    slots: Vec<u8>,
    inst: HashMap<u32, Inst>,
}

/// Draws `shapes` into a layer, where they or the last ones were.
fn draw(drawn: &mut Option<Drawn>, p: &Present, session: &Session, instance: u32, slot: u8, shapes: Vec<Shape>) -> Result<LayerOut, String> {
    let mut fresh = false;
    if drawn.as_ref().is_none_or(|d| d.picture != p.picture || d.screen != p.screen) {
        let layer = LayerFile::create(session.dir(), instance, slot, LAYER, p.picture)?;
        *drawn = Some(Drawn { picture: p.picture, screen: p.screen, layer, shapes: Vec::new(), bounds: Vec::new() });
        fresh = true;
    }
    let d = drawn.as_mut().unwrap();
    if !fresh && shapes == d.shapes {
        return Ok(LayerOut { slot, id: LAYER, new_file: None, damage: Vec::new() });
    }
    let geom = Geom::for_rect(p.screen);
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
    Ok(LayerOut { slot, id: LAYER, new_file: fresh.then(|| d.layer.path.clone()), damage: if damage.is_empty() { vec![] } else { vec![rect(damage)] } })
}

/// The view of the shell's scene from its property: which Screens, then
/// the transform as 12 f64 (rows, then the translation).
fn decode_view(b: &[u8]) -> SceneView {
    if b.len() != 1 + 12 * 8 {
        return SceneView::LISTED;
    }
    let f = |i: usize| f64::from_le_bytes(b[1 + 8 * i..9 + 8 * i].try_into().unwrap());
    let m = [[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]];
    SceneView { screens: b[0], view: Transform { m, t: [f(9), f(10), f(11)] } }
}

impl Device for VectorDriver {
    fn info(&mut self, session: &Session) -> DeviceInfo {
        self.presents = session.presents();
        self.slots = session.hello.slots.clone();
        let layer = |zv| if self.presents { vec![LayerDecl { id: LAYER, z: zv }] } else { Vec::new() };
        let mut slots = Vec::new();
        if session.asks(3) {
            slots.push(SlotInfo { slot: 3, version: 0, layers: layer(z::VECTOR), table: table2() });
        }
        if session.asks(SLOT3) {
            slots.push(SlotInfo { slot: SLOT3, version: 0, layers: layer(z::VECTOR3), table: table3() });
        }
        DeviceInfo {
            name: "terse-dev-vector (Rust)".into(),
            native: (0, 0),
            slots,
            publishes: vec![property::OUTLINES.into(), property::LIST3.into()],
            subscribes: vec![property::EYE.into(), property::VIEW.into()],
            services: Vec::new(),
        }
    }

    fn open(&mut self, instance: u32, open: &Open, _: &Session) -> Result<(), String> {
        if open.lists(SLOT3) && !self.slots.contains(&3) {
            return Err("device 9 needs this driver to serve device 3 too (SPEC §vector3d.calls)".into());
        }
        let inst = Inst {
            v: Vector::default(),
            v3: open.lists(SLOT3).then(Vector3::default),
            lists2: open.lists(3),
            eye: (0.0, 0.0),
            view: SceneView::LISTED,
            drawn: [None, None],
        };
        self.inst.insert(instance, inst);
        Ok(())
    }

    fn close(&mut self, instance: u32) {
        self.inst.remove(&instance);
    }

    fn reset(&mut self, instance: u32) {
        if let Some(i) = self.inst.get_mut(&instance) {
            i.v = Vector::default();
            if let Some(v3) = &mut i.v3 {
                *v3 = Vector3::default();
            }
        }
    }

    fn call(&mut self, instance: u32, c: &CallIn, _: &mut dyn Mem) -> Reply {
        let Some(i) = self.inst.get_mut(&instance) else { return Reply { a0: ERR_UNKNOWN, ..Default::default() } };
        let (a0, charge) = match (c.slot, c.function) {
            (3, 0) => {
                let r = i.v.submit(c.buf);
                (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
            }
            (3, 1) => {
                i.v.clear();
                (0, 0)
            }
            (SLOT3, 0) if i.v3.is_some() => {
                let r = i.v3.as_mut().unwrap().submit(c.buf);
                (r, if r == 0 { copy_charge(c.buf.len()) } else { 0 })
            }
            (SLOT3, 1) if i.v3.is_some() => {
                i.v3.as_mut().unwrap().clear();
                (0, 0)
            }
            // The style and persistence are device 3's (SPEC §vector3d.calls).
            (3 | SLOT3, 2) => (i.v.set_style(c.args[0]), 0),
            (3 | SLOT3, 3) => (i.v.set_persistence(c.args[0]), 0),
            (3, 4) => (i.v.set_fill(c.args[0]), 0),
            _ => (ERR_UNKNOWN, 0),
        };
        Reply { a0, a1: None, charge }
    }

    fn begin(&mut self, _: u32, _: &Begin, _: &mut dyn Mem) -> Inputs {
        Inputs::default()
    }

    fn end(&mut self, _: u32, _: u32, _: bool) {}

    fn state(&self, instance: u32, slot: u8) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(i) = self.inst.get(&instance) {
            match slot {
                3 => i.v.write_state(&mut out),
                _ => {
                    if let Some(v3) = &i.v3 {
                        v3.write_state(&mut out);
                    }
                }
            }
        }
        out
    }

    fn restore(&mut self, instance: u32, slot: u8, bytes: &[u8]) -> Option<usize> {
        let i = self.inst.get_mut(&instance)?;
        let mut r = terse_core::snapshot::Reader::new(bytes);
        match slot {
            3 => {
                let mut v = Vector::default();
                v.restore(&mut r).ok()?;
                // Without device 3 the fill mode and the 2D list cannot be set.
                if !i.lists2 && (!v.fill() || !v.list().is_empty()) {
                    return None;
                }
                let n = r.position();
                let mut again = Vec::new();
                v.write_state(&mut again);
                (again == bytes[..n]).then_some(())?;
                i.v = v;
                Some(n)
            }
            _ => {
                let v3 = i.v3.as_mut()?;
                let mut v = Vector3::default();
                v.restore(&mut r).ok()?;
                *v3 = v;
                Some(r.position())
            }
        }
    }

    fn get(&mut self, instance: u32, name: &str) -> Option<Vec<u8>> {
        let i = self.inst.get(&instance)?;
        match name {
            property::OUTLINES => Some(vec![i.outlines() as u8]),
            property::LIST3 => Some(i.v3.as_ref().map_or(Vec::new(), |v| v.list().to_vec())),
            _ => None,
        }
    }

    fn set(&mut self, instance: u32, name: &str, value: &[u8]) {
        let Some(i) = self.inst.get_mut(&instance) else { return };
        match name {
            property::EYE if value.len() == 8 => {
                let f = |k: usize| f32::from_le_bytes(value[k..k + 4].try_into().unwrap());
                i.eye = (f(0), f(4));
            }
            property::VIEW => i.view = decode_view(value),
            _ => {}
        }
    }

    fn present(&mut self, instance: u32, p: &Present, session: &Session) -> PresentOut {
        let Some(i) = self.inst.get_mut(&instance) else { return PresentOut::default() };
        if !self.presents {
            return PresentOut::default();
        }
        let mut out = PresentOut::default();
        for (k, slot, on) in [(0, 3u8, i.lists2 && self.slots.contains(&3)), (1, SLOT3, i.v3.is_some())] {
            if !on {
                continue;
            }
            let shapes: Vec<Shape> = i.primitives(k).iter().map(|s| Shape::new(s, 0)).collect();
            match draw(&mut i.drawn[k], p, session, instance, slot, shapes) {
                Ok(l) => out.layers.push(l),
                Err(e) => eprintln!("terse-dev-vector: {e}"),
            }
        }
        out
    }
}

fn main() -> std::process::ExitCode {
    terse_bus::driver::run(&mut VectorDriver { presents: false, slots: Vec::new(), inst: HashMap::new() }, "terse-dev-vector")
}
