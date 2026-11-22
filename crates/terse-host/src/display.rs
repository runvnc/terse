//! What every renderer shares: where glyph bitmaps come from, where the
//! screen sits in a viewport, which primitives a display list draws, and
//! the [`Display`] trait the host drives, which the GPU renderer
//! (`screens.rs`, wgpu) and the software renderer (`soft/`) implement.

use std::sync::Arc;

use terse_core::vector::{MAX_LIST, decode_outlines_within, decode_within};
#[cfg(test)]
use terse_core::vector::{decode, decode_outlines};
use terse_core::vector3::Panel;
use terse_core::{Primitive, SCREEN_HEIGHT, SCREEN_WIDTH, System, Vector, Vector3};
use terse_net::Prompt;

use crate::page::HostLayout;

/// Supplies glyph coverage bitmaps, one or two terminal cells in size.
pub trait GlyphSource {
    /// Cell size in pixels (width, height). Should be exactly 1:2 (SPEC §screen:
    /// square screen units); the renderer shows the grid at 4:3 regardless,
    /// so glyphs from a cell of another shape are stretched.
    fn cell_size(&self) -> (u32, u32);
    /// Writes the coverage of `ch` spanning `cells` cells (1, or 2 for a wide
    /// character) into `out` (`cells * width * height` bytes, row major,
    /// already zeroed). Unknown characters may be left blank.
    fn rasterize(&mut self, ch: char, cells: u32, out: &mut [u8]);
    /// The same glyphs for a cell `width` pixels wide (and twice as high),
    /// for a screen shown smaller or larger than the native size.
    fn resized(&self, width: u32) -> Box<dyn GlyphSource>;
}

/// The grid rectangle (x, y, width, height) inside a viewport: the largest
/// integer multiple of the native size `grid` that fits, or a scaled-down fit,
/// centred. The scale is the same on both axes, so a 4:3 `grid` stays 4:3.
pub fn grid_rect(grid: (u32, u32), viewport: (u32, u32)) -> (f32, f32, f32, f32) {
    let (gw, gh) = grid;
    let fit = (viewport.0 as f32 / gw as f32).min(viewport.1 as f32 / gh as f32);
    let scale = if fit >= 1.0 { fit.floor() } else { fit };
    let (w, h) = (gw as f32 * scale, gh as f32 * scale);
    (((viewport.0 as f32 - w) / 2.0).floor(), ((viewport.1 as f32 - h) / 2.0).floor(), w, h)
}

/// A viewport position in screen units, given the grid rectangle: the grid
/// spans exactly the 640 × 480 screen.
pub fn screen_pos(rect: (f32, f32, f32, f32), x: f64, y: f64) -> (u16, u16) {
    let (ox, oy, w, h) = rect;
    let unit = |p: f64, origin: f32, size: f32, units: u16| {
        ((p as f32 - origin) / size * units as f32).floor().clamp(0.0, (units - 1) as f32) as u16
    };
    (unit(x, ox, w, SCREEN_WIDTH), unit(y, oy, h, SCREEN_HEIGHT))
}

/// The shell's screen in a system (SPEC §machines.screens): the whole viewport at 4:3,
/// with glyphs rasterised for that size, rather than the largest whole
/// multiple of the native grid.
pub fn system_rect(viewport: (u32, u32)) -> (f32, f32, f32, f32) {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let scale = (vw / 640.0).min(vh / 480.0);
    let (w, h) = ((640.0 * scale).floor(), (480.0 * scale).floor());
    (((vw - w) / 2.0).floor(), ((vh - h) / 2.0).floor(), w, h)
}

/// A machine's picture (SPEC §machines.screens, in the shell's units) in pixels, given
/// the shell's screen rectangle.
pub fn picture_rect(shell: (f32, f32, f32, f32), p: terse_core::Region) -> (f32, f32, f32, f32) {
    let (ox, oy, w, _) = shell;
    let scale = w / SCREEN_WIDTH as f32;
    let x0 = (ox + p.x as f32 * scale).round();
    let y0 = (oy + p.y as f32 * scale).round();
    let x1 = (ox + (p.x + p.w) as f32 * scale).round();
    let y1 = (oy + (p.y + p.h) as f32 * scale).round();
    (x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
}

/// Glyph cell width for a screen drawn into `rect`: whole pixels per cell,
/// never more than the cell shown, so a glyph is drawn at its size or a
/// fraction of a pixel larger.
pub fn cell_width_for(rect: (f32, f32, f32, f32)) -> u32 {
    ((rect.2 / terse_core::COLS as f32).floor() as u32).max(2)
}

/// Whether the list is drawn as outlines: in the phosphor presentation
/// (`phosphor`, which may be forced), or as `Vector::outlined` says (fill
/// off in the clean style).
pub fn outlined(phosphor: bool, vector: Option<&Vector>) -> bool {
    phosphor || vector.is_some_and(Vector::outlined)
}

/// Most primitives a list can give: a rectangle command is 5 bytes and four
/// outline lines in the phosphor style (otherwise a 3-byte circle or disc
/// is the densest).
pub const MAX_PRIMITIVES: usize = MAX_LIST / 5 * 4 + 4;

/// The primitives the host draws for `list`: its outlines
/// (`decode_outlines`) or its primitives (`decode`), within the ink cap.
pub fn decode_shapes(list: &[u8], outlines: bool, out: &mut Vec<Primitive>) {
    decode_shapes_within(list, outlines, terse_core::vector::INK_CAP, out)
}

/// [`decode_shapes`] within a lower ink cap (a machine's share of a
/// system's ink, SPEC §machines.ink).
pub fn decode_shapes_within(list: &[u8], outlines: bool, cap: u64, out: &mut Vec<Primitive>) {
    out.clear();
    if outlines {
        out.extend(decode_outlines_within(list, cap).take(MAX_PRIMITIVES));
    } else {
        out.extend(decode_within(list, cap).take(MAX_PRIMITIVES));
    }
}

/// The ink caps a machine's lists are drawn within: the devices' own
/// (SPEC §vector.ink-cap, §vector3d.caps), or its share of a system's ink
/// (SPEC §machines.ink).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caps {
    pub ink2: u64,
    pub ink3: f64,
}

impl Caps {
    pub const FULL: Caps = Caps { ink2: terse_core::vector::INK_CAP, ink3: terse_core::vector3::INK_CAP };

    /// The caps of a machine's share.
    pub fn of(share: Option<&terse_core::system_ink::Share>) -> Caps {
        share.map_or(Caps::FULL, |s| Caps { ink2: s.cap2, ink3: s.cap3 })
    }
}

/// Most shapes the vector layer draws: a 2D list's primitives and a 3D
/// list's segments (SPEC §vector3d.caps).
pub const MAX_SHAPES: usize = MAX_PRIMITIVES + terse_core::vector3::SEGMENT_CAP;

/// The eye the host sees 3D lists from: screen units from the reference
/// eye (SPEC §vector3d.space), each at most 160.
pub type Eye = (f32, f32);

/// What a machine's vector layer shows: its 2D list (as [`decode_shapes`]
/// gives it), then, over it, its 3D list's segments seen from `eye`
/// (`terse_core::vector3::project`, the same for every renderer).
#[derive(Clone, Copy)]
pub struct VectorScene<'a> {
    pub vector: Option<&'a Vector>,
    pub vector3: Option<&'a Vector3>,
    pub eye: Eye,
    /// The ink caps its lists are drawn within.
    pub caps: Caps,
}

/// Identifies what [`VectorScene::shapes`] gives: the lists' generations,
/// the eye (only if there is a 3D list) and whether as outlines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneKey {
    lists: (u64, u64),
    eye: Option<(u32, u32)>,
    caps: (u64, u64),
    outlines: bool,
}

impl VectorScene<'_> {
    pub fn key(&self, outlines: bool) -> SceneKey {
        let v3 = self.vector3.filter(|v| !v.list().is_empty());
        SceneKey {
            lists: (self.vector.map_or(0, Vector::generation), self.vector3.map_or(0, Vector3::generation)),
            eye: v3.map(|_| (self.eye.0.to_bits(), self.eye.1.to_bits())),
            outlines,
            caps: (self.caps.ink2, self.caps.ink3.to_bits()),
        }
    }

    /// The shapes in drawing order into `out`, and into `opcodes` a
    /// signature that is the same for two scenes whose shapes line up (the
    /// same shapes, maybe moved: for the afterglow's smear).
    pub fn shapes(&self, outlines: bool, out: &mut Vec<Primitive>, opcodes: &mut Vec<u8>) {
        let list = self.vector.map_or(&[][..], Vector::list);
        decode_shapes_within(list, outlines, self.caps.ink2, out);
        *opcodes = crate::phosphor::opcodes(list);
        if let Some(v) = self.vector3.filter(|v| !v.list().is_empty()) {
            // With faces, how many segments each figure gave is part of the
            // signature: what is hidden changes as the models turn.
            let mut pieces = Vec::new();
            let scene = terse_core::vector3::SceneView::LISTED;
            terse_core::vector3::project_scene_within(v.list(), self.eye, &scene, out, &mut pieces, &mut Vec::new(), self.caps.ink3);
            opcodes.push(0xFF);
            opcodes.extend(terse_core::vector3::opcodes(v.list()));
            opcodes.extend(pieces);
        }
    }
}

/// A machine's 3D scene as the host draws it over its glass (SPEC §machines.screens,
/// 13.5): the shell's when Screens are in effect, or any machine's when
/// Surfaces are. The panels, far to near (Screens, not the machine a
/// finished view shows flat, and the machine's own Surfaces, handle 0),
/// the 3D segments, cut by the faces and the panels, which are drawn over
/// the panels, and the source rectangles of the Surfaces (screen units),
/// which are left off the glass. `None`: neither is in effect, and the
/// screen is drawn as a single machine's, its 3D list in its vector layer.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneLayers {
    pub panels: Vec<Panel>,
    pub lines: Vec<Primitive>,
    pub rects: Vec<[u16; 4]>,
}

impl SceneLayers {
    /// Whether any panel is one of the machine's own Surfaces.
    pub fn has_surfaces(&self) -> bool {
        !self.rects.is_empty()
    }
}

/// The scene of `system`'s shell seen from `eye` when Screens are in
/// effect (its own Surfaces among the panels).
pub fn scene_layers<D: terse_core::devices::Devices>(system: &System<D>, eye: Eye) -> Option<SceneLayers> {
    let scene = system.scene(eye)?;
    let list = system.shell().list3();
    let list = &list[..];
    let (mut lines, mut panels) = (Vec::new(), Vec::new());
    let p = terse_core::vector3::project_scene(list, eye, &scene.view, &mut lines, &mut Vec::new(), &mut panels);
    panels.retain(|p| scene.faced == 0 || p.handle != scene.faced);
    Some(SceneLayers { panels, lines, rects: p.surface_rects().to_vec() })
}

/// The panels of machines whose pictures did not fit the system ink (SPEC
/// §machines.ink) are not drawn: the shell's screen shows where they are
/// (they still hide the shell's lines, as faces do).
pub fn plate_panels(layers: &mut Option<SceneLayers>, shares: &terse_core::system_ink::Shares) {
    if let Some(l) = layers {
        l.panels.retain(|p| p.is_surface() || !shares.of(p.handle).is_some_and(|s| s.plate));
    }
}

/// A machine's own scene seen from `eye` when its 3D list has Surfaces in
/// effect (SPEC §vector3d.presentation): its Surfaces, its 3D segments and the rectangles
/// left off its glass.
/// `cap3`: the ink cap its 3D list is drawn within ([`Caps`]).
pub fn surface_layers<D: terse_core::devices::Devices>(machine: &terse_core::Machine<D>, eye: Eye, cap3: f64) -> Option<SceneLayers> {
    surface_layers_of(&machine.list3(), eye, cap3)
}

/// [`surface_layers`] of a machine whose 3D list is `list`.
pub fn surface_layers_of(list: &[u8], eye: Eye, cap3: f64) -> Option<SceneLayers> {
    if !terse_core::vector3::has_surface(list) {
        return None;
    }
    let (mut lines, mut panels) = (Vec::new(), Vec::new());
    let scene = terse_core::vector3::SceneView::LISTED;
    let p = terse_core::vector3::project_scene_within(list, eye, &scene, &mut lines, &mut Vec::new(), &mut panels, cap3);
    (p.surfaces > 0).then(|| SceneLayers { panels, lines, rects: p.surface_rects().to_vec() })
}

/// Where the pixels of a screen drawn into `rect` show the source
/// rectangle `r` (screen units): x0, y0, x1, y1 (pixels whose centres lie
/// in it are x0..x1, y0..y1).
pub fn source_pixels(rect: (f32, f32, f32, f32), r: [u16; 4]) -> (i32, i32, i32, i32) {
    let s = rect.2 as f64 / SCREEN_WIDTH as f64;
    let at = |o: f32, u: u16| (o as f64 + u as f64 * s - 0.5).ceil() as i32;
    (at(rect.0, r[0]), at(rect.1, r[1]), at(rect.0, r[0] + r[2]), at(rect.1, r[1] + r[3]))
}

/// The size (pixels) a machine's whole screen is drawn at for its own
/// Surfaces, its glass drawn into `rect`: the glass's own size, so that a
/// Surface showing its source as large as the glass does shows the same
/// pixels, unless a Surface magnifies its source: then as large as the one
/// that magnifies most needs (in whole cells), at most twice the glass;
/// `current` is kept while it is large enough and not half as large again
/// as needed.
pub fn surface_size(panels: &[Panel], eye: Eye, rect: (f32, f32, f32, f32), current: Option<(u32, u32)>) -> (u32, u32) {
    let need = panels
        .iter()
        .filter(|p| p.is_surface())
        .map(|p| panel_width_px(p, eye, rect) * SCREEN_WIDTH as f64 / p.source[2].max(1) as f64)
        .fold(0.0, f64::max);
    let glass = ((rect.2.round() as u32).max(1), (rect.3.round() as u32).max(1));
    if need <= glass.0 as f64 {
        return glass;
    }
    if let Some(c) = current
        && c.0 as f64 >= need
        && (c.0 as f64) < need * 1.5
        && c.0 > glass.0
    {
        return c;
    }
    let cols = terse_core::COLS as u32;
    let w = ((need / cols as f64).ceil() as u32 * cols).min(2 * glass.0).max(glass.0);
    (w, w * 3 / 4)
}

/// The eye at (ex, ey) in screen units, as `project` places it.
pub fn eye_point(eye: Eye) -> (f64, f64) {
    let m = terse_core::vector3::MAX_EYE_OFFSET;
    (terse_core::vector3::EYE_X + eye.0.clamp(-m, m) as f64, terse_core::vector3::EYE_Y + eye.1.clamp(-m, m) as f64)
}

/// A point of the scene on a screen drawn into `rect` (pixels): its pixel
/// position and its w = (960 + z) / 960, by which a GPU divides (SPEC
/// §vector3d.space), for a point in front of the near plane.
pub fn to_pixels(p: [f64; 3], eye: (f64, f64), rect: (f32, f32, f32, f32)) -> (f64, f64, f64) {
    let (gx, gy) = terse_core::vector3::to_glass(p, eye);
    let s = rect.2 as f64 / SCREEN_WIDTH as f64;
    (rect.0 as f64 + gx * s, rect.1 as f64 + gy * s, (terse_core::vector3::EYE_DISTANCE + p[2]) / terse_core::vector3::EYE_DISTANCE)
}

/// How wide (pixels) a panel looks on a screen drawn into `rect`: the
/// longer of its top and bottom edges, or the longer of its sides times
/// its source's shape (4/3 for a Screen), on the glass (points behind the
/// near plane taken at it).
pub fn panel_width_px(p: &Panel, eye: Eye, rect: (f32, f32, f32, f32)) -> f64 {
    let eye = eye_point(eye);
    let g = p.corners.map(|c| {
        let c = [c[0], c[1], c[2].max(terse_core::vector3::NEAR_Z + 1.0)];
        let (x, y, _) = to_pixels(c, eye, rect);
        (x, y)
    });
    let len = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let shape = p.source[2] as f64 / p.source[3].max(1) as f64;
    len(g[0], g[1]).max(len(g[3], g[2])).max(len(g[0], g[3]).max(len(g[1], g[2])) * shape)
}

/// The glyph cell width (pixels) a panel's machine is drawn with (its
/// picture is 80 cells wide, so 80 × the cell width pixels): the nearest
/// to `width` pixels on the target (so a panel facing the viewer is drawn
/// from one level, magnified or minified by less than half a cell), at
/// least 2 and at most `cap` (the cell of the shell's own screen: never
/// sharper than a machine shown flat over the whole screen); `current` is
/// kept while it is at most one less and two more, so a panel that moves
/// a little is not drawn again at every size.
pub fn panel_cell_width(width: f64, cap: u32, current: Option<u32>) -> u32 {
    let cap = cap.max(2);
    let need = ((width / terse_core::COLS as f64).round() as u32).clamp(2, cap);
    match current {
        Some(c) if c + 1 >= need && c <= (need + 2).min(cap) => c,
        _ => need,
    }
}

/// The colour of a panel seen from behind (SPEC §machines.screens): opaque, dark.
pub const PANEL_BACK: [u8; 3] = [24, 26, 32];

/// The host's consent prompt as the screens show it: the prompt, how many
/// requests wait, and the asking machine's title.
pub type ShownPrompt<'a> = Option<(&'a Prompt, usize, String)>;

/// Which renderer draws the screens (`--renderer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererChoice {
    Gpu,
    Software,
    /// The GPU if an adapter is found, else the software renderer.
    Auto,
}

impl RendererChoice {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gpu" => Some(Self::Gpu),
            "software" | "soft" | "cpu" => Some(Self::Software),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// The screens of a system (SPEC §machines.screens) as a renderer shows them: the
/// host gives it what every machine shows after each frame and asks it for
/// the picture. Both renderers draw the same picture.
pub trait Display {
    /// Takes what every machine shows after a frame: the shell's screen
    /// (with the consent prompt if one is asked) and each child's picture.
    /// Machines in `ran` (serials) ran a frame: their phosphor light
    /// advances. `layout` gives the page layout to show for a serial.
    fn update(
        &mut self,
        system: &System,
        viewport: (u32, u32),
        ran: &[u32],
        layout: &dyn Fn(u32) -> Option<Arc<HostLayout>>,
        prompt: ShownPrompt,
    );
    /// Where the eye is for 3D lists (SPEC §vector3d.space): screen units from the
    /// reference eye, the same for every machine. Takes effect at the next
    /// `update`.
    fn set_eye(&mut self, eye: Eye);
    /// How the phosphor style is drawn (`--tube`, `--vignette`): presentation
    /// only, nothing a program can observe.
    fn set_look(&mut self, look: crate::phosphor::Look);
    /// The host's notice over the shell's bottom rows (`PERFORMANCE.md`
    /// R15: a slow machine, or the host's own drawing), shown from the
    /// next `update` while no consent prompt is; `None` takes it away.
    fn set_notice(&mut self, notice: Option<crate::consent::Notice>);
    /// The user's presentation options: the phosphor style without its
    /// afterglow, the phosphor presentation whatever the style, and whether
    /// the page's scroll is animated (and its figures rasterised on their
    /// own thread; not for screenshots).
    fn configure(&mut self, no_trails: bool, force_phosphor: bool, animate_page: bool);
    /// Redraws every phosphor beam from what is shown now, without
    /// advancing the afterglow (a screenshot's last picture).
    fn refresh_beams(&mut self, viewport: (u32, u32));
    /// The shell's phosphor light advances one frame with nothing new (the
    /// system stopped: the afterglow goes on fading).
    fn fade_shell(&mut self, viewport: (u32, u32));
    /// Whether the shell's screen is in the phosphor presentation.
    fn shell_phosphor(&self) -> bool;
    /// The page anchor under (x, y) in machine `serial`'s screen units, as
    /// its page was last shown (SPEC §page.events).
    fn anchor(&self, serial: u32, x: u16, y: u16) -> u16;
    /// A viewport position in the shell's screen units (SPEC §screen).
    fn screen_pos(&self, viewport: (u32, u32), x: f64, y: f64) -> (u16, u16);
    /// The height of one terminal row of the shell's screen in pixels.
    fn row_height(&self, viewport: (u32, u32)) -> f64;
    /// What the shell's page figures did since the last call (`TERSE_PAGE_TRACE`).
    fn take_page_figure_log(&mut self) -> Vec<String>;
    /// Draws the whole picture offscreen and returns it as RGBA, 8 bits a
    /// channel, sRGB, `viewport` pixels, rows packed.
    fn snapshot(&mut self, viewport: (u32, u32)) -> Result<Vec<u8>, String>;
    /// Draws the whole picture offscreen and waits until it is done (for
    /// timing), without reading it back.
    fn render_offscreen(&mut self, viewport: (u32, u32)) -> Result<(), String>;
    /// The size of the grid at the font's native size (4:3).
    fn grid_size(&self) -> (u32, u32);
    /// A name for the user.
    fn name(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer draws exactly what `Vector::shapes` gives: outlines in
    /// the phosphor style or with fill off, the capped list otherwise; and
    /// outlines when the phosphor presentation is forced.
    #[test]
    fn draws_what_the_device_says() {
        // A disc, a rectangle and a triangle.
        let list = [
            &[0x04, 0, 1, 0, 1][..],
            &[0x07, 64, 0],
            &[0x08, 0, 2, 0, 2],
            &[0x09, 0, 3, 0, 3, 0, 4, 0, 1],
        ]
        .concat();
        for (style, fill) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let mut v = Vector::default();
            assert_eq!(v.submit(&list), 0);
            v.set_style(style);
            v.set_fill(fill);
            let mut drawn = Vec::new();
            decode_shapes(v.list(), outlined(v.phosphor(), Some(&v)), &mut drawn);
            assert_eq!(drawn, v.shapes().collect::<Vec<_>>(), "style {style} fill {fill}");
            let filled = drawn.iter().any(|p| !matches!(p, Primitive::Line { .. } | Primitive::Circle { .. }));
            assert_eq!(filled, style == 0 && fill == 0, "style {style} fill {fill}");
            decode_shapes(v.list(), outlined(true, Some(&v)), &mut drawn);
            assert_eq!(drawn, decode_outlines(v.list()).collect::<Vec<_>>());
        }
        assert!(!outlined(false, None) && outlined(true, None));
    }

    /// A 3D list's segments come after the 2D list's shapes, as
    /// `vector3::project` gives them for the eye; the key changes with the
    /// eye only when there is a 3D list.
    #[test]
    fn scenes_add_the_projected_segments() {
        use terse_core::Vector3;
        let mut v = Vector::default();
        v.submit(&[0x04, 0, 1, 0, 1, 0x05, 0, 2, 0, 2]);
        let mut v3 = Vector3::default();
        let z = (-4000i16).to_le_bytes();
        v3.submit(&[0x03, 0, 1, 0, 1, z[0], z[1], 0x04, 0, 2, 0, 2, 0, 0, 0x05]);
        let scene = |eye| VectorScene { vector: Some(&v), vector3: Some(&v3), eye, caps: Caps::FULL };
        let (mut shapes, mut opcodes) = (Vec::new(), Vec::new());
        scene((40.0, 0.0)).shapes(false, &mut shapes, &mut opcodes);
        let mut want = v.shapes().collect::<Vec<_>>();
        terse_core::vector3::project(v3.list(), (40.0, 0.0), &mut want);
        assert_eq!(shapes, want);
        assert_eq!(shapes.len(), 3);
        assert_eq!(opcodes, [0x04, 0x05, 0xFF, 0x03, 0x04, 0x05]);
        // With a face, the number of segments each figure gave follows (2
        // bytes each): a point, hidden by nothing.
        let mut v3f = Vector3::default();
        let mut list = vec![0x0C, 3];
        list.extend([0u8; 18]);
        list.push(0x05);
        v3f.submit(&list);
        VectorScene { vector: None, vector3: Some(&v3f), eye: (0.0, 0.0), caps: Caps::FULL }.shapes(false, &mut shapes, &mut opcodes);
        assert_eq!(opcodes, [0xFF, 0x0C, 0x05, 1, 0]);
        assert_ne!(scene((40.0, 0.0)).key(false), scene((0.0, 0.0)).key(false));
        let flat = |eye| VectorScene { vector: Some(&v), vector3: None, eye, caps: Caps::FULL };
        assert_eq!(flat((40.0, 0.0)).key(false), flat((0.0, 0.0)).key(false));
        assert_ne!(scene((0.0, 0.0)).key(false), scene((0.0, 0.0)).key(true));
    }

    /// The densest lists fit: all rectangles in the phosphor style (4 lines
    /// per 5 bytes), all discs otherwise.
    #[test]
    fn densest_lists_fit() {
        let rects: Vec<u8> = [8u8, 1, 0, 1, 0].iter().copied().cycle().take(MAX_LIST).collect();
        assert_eq!(decode_outlines(&rects).count(), MAX_LIST / 5 * 4);
        let discs: Vec<u8> = [7u8, 1, 0].iter().copied().cycle().take(MAX_LIST).collect();
        assert_eq!(decode(&discs).count(), MAX_LIST / 3);
        assert!(MAX_LIST / 5 * 4 <= MAX_PRIMITIVES && MAX_LIST / 3 <= MAX_PRIMITIVES);
    }

    #[test]
    fn viewport_to_screen_units() {
        // A 10 × 20 px cell grid (800 × 600) shown at 2x in a 1700 × 1300
        // viewport: offset (50, 50), 1600 × 1200.
        let rect = grid_rect((800, 600), (1700, 1300));
        assert_eq!(rect, (50.0, 50.0, 1600.0, 1200.0));
        assert_eq!(screen_pos(rect, 50.0, 50.0), (0, 0));
        assert_eq!(screen_pos(rect, 1649.9, 1249.9), (639, 479));
        // Cell (column 3, row 2) starts at screen (24, 32) (SPEC §screen).
        assert_eq!(screen_pos(rect, 50.0 + 3.0 * 20.0, 50.0 + 2.0 * 40.0), (24, 32));
        // Outside the grid: clamped.
        assert_eq!(screen_pos(rect, 0.0, 2000.0), (0, 479));
        assert_eq!(screen_pos(rect, 1699.0, -5.0), (639, 0));
        // Scaled down to fit.
        let rect = grid_rect((800, 600), (400, 600));
        assert_eq!(rect, (0.0, 150.0, 400.0, 300.0));
        assert_eq!(screen_pos(rect, 200.0, 300.0), (320, 240));
    }

    /// Whatever the window's shape, the grid is 4:3 and centred, so a screen
    /// unit is as wide as it is high (SPEC §screen), and the pointer maps back
    /// through the same rectangle.
    #[test]
    fn grid_stays_four_by_three() {
        let native = (880, 660); // 11 x 22 pixel cells
        for viewport in [(880, 660), (1920, 1080), (1000, 2000), (300, 200), (1761, 1321), (881, 1500)] {
            let (x, y, w, h) = grid_rect(native, viewport);
            assert!((w / h - 4.0 / 3.0).abs() < 1e-6, "{viewport:?}: {w} x {h}");
            assert!(w <= viewport.0 as f32 && h <= viewport.1 as f32, "{viewport:?}");
            // Centred (the offset is a whole pixel, the size need not be).
            assert!((x - (viewport.0 as f32 - w - x)).abs() < 2.0, "{viewport:?}");
            assert!((y - (viewport.1 as f32 - h - y)).abs() < 2.0, "{viewport:?}");
            // One screen unit is the same size on both axes.
            assert!((w / 640.0 - h / 480.0).abs() < 1e-4);
            // The centre of the window is the centre of the screen.
            let c = screen_pos((x, y, w, h), viewport.0 as f64 / 2.0, viewport.1 as f64 / 2.0);
            assert!(c.0.abs_diff(320) <= 1 && c.1.abs_diff(240) <= 1, "{viewport:?}: {c:?}");
        }
        assert_eq!(grid_rect(native, (1920, 1080)), (520.0, 210.0, 880.0, 660.0));
        assert_eq!(grid_rect(native, (1760, 1400)), (0.0, 40.0, 1760.0, 1320.0));
    }
}
