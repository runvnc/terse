//! Pixel buffers, rectangles, damage and the worker threads of the
//! software renderer.

/// A rectangle of whole pixels, `x0..x1` × `y0..y1` (half open).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub const EMPTY: IRect = IRect { x0: 0, y0: 0, x1: 0, y1: 0 };

    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        IRect { x0, y0, x1, y1 }
    }

    pub fn of_size(size: (u32, u32)) -> Self {
        IRect { x0: 0, y0: 0, x1: size.0 as i32, y1: size.1 as i32 }
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn width(&self) -> usize {
        (self.x1 - self.x0).max(0) as usize
    }

    pub fn height(&self) -> usize {
        (self.y1 - self.y0).max(0) as usize
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }

    /// The pixels a float rectangle (x, y, w, h) touches: floor of the
    /// start, ceiling of the end.
    pub fn covering(r: (f32, f32, f32, f32)) -> IRect {
        IRect {
            x0: r.0.floor() as i32,
            y0: r.1.floor() as i32,
            x1: (r.0 + r.2).ceil() as i32,
            y1: (r.1 + r.3).ceil() as i32,
        }
    }
}

/// A pixel `0x00RRGGBB` (XRGB8888: the byte order of BGRA in memory on a
/// little-endian machine), as softbuffer and DRM dumb buffers take it.
#[inline]
pub fn pack(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

#[inline]
pub fn unpack(p: u32) -> [u8; 3] {
    [(p >> 16) as u8, (p >> 8) as u8, p as u8]
}

/// `a` and `b` mixed by `t` (0–255) with rounding, per channel.
#[inline]
pub fn mix(a: u32, b: u32, t: u32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s) & 255, (b >> s) & 255);
        ((x * (255 - t) + y * t + 127) / 255) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// `src` over `dst` with opacity `alpha` (0–1), rounded as an 8-bit target
/// is.
#[inline]
pub fn over(dst: u32, src: [f32; 3], alpha: f32) -> u32 {
    let [r, g, b] = unpack(dst);
    let k = 1.0 - alpha;
    let ch = |s: f32, d: u8| (s * alpha + d as f32 * k + 0.5) as u8;
    pack(ch(src[0], r), ch(src[1], g), ch(src[2], b))
}

/// Pixels covering `area` (in the frame's coordinates), rows packed.
#[derive(Clone, Default)]
pub struct Pixels {
    pub area: IRect,
    pub data: Vec<u32>,
}

impl Pixels {
    pub fn new(area: IRect) -> Self {
        Pixels { area, data: vec![0; area.width() * area.height()] }
    }

    pub fn stride(&self) -> usize {
        self.area.width()
    }

    /// The row `y` (frame coordinates) from `x0` to `x1`.
    pub fn row(&self, y: i32, x0: i32, x1: i32) -> &[u32] {
        let s = self.stride();
        let base = (y - self.area.y0) as usize * s;
        &self.data[base + (x0 - self.area.x0) as usize..base + (x1 - self.area.x0) as usize]
    }

    /// RGBA, 8 bits a channel.
    pub fn to_rgba(&self) -> Vec<u8> {
        self.data.iter().flat_map(|&p| {
            let [r, g, b] = unpack(p);
            [r, g, b, 255]
        }).collect()
    }
}

/// Side of a damage tile in pixels. Damage is kept per tile of the frame;
/// a tile row is also the unit of work handed to a thread.
pub const TILE: i32 = 32;

/// What changed in the frame since it was last drawn: a set of tiles of
/// `TILE` pixels, in the frame's coordinates.
#[derive(Clone, Default)]
pub struct Damage {
    size: (u32, u32),
    cols: usize,
    tiles: Vec<bool>,
    any: bool,
}

impl Damage {
    pub fn new(size: (u32, u32)) -> Self {
        let cols = (size.0 as usize).div_ceil(TILE as usize);
        let rows = (size.1 as usize).div_ceil(TILE as usize);
        Damage { size, cols, tiles: vec![false; cols * rows], any: false }
    }

    pub fn is_empty(&self) -> bool {
        !self.any
    }

    pub fn add(&mut self, r: IRect) {
        let r = r.intersect(&IRect::of_size(self.size));
        if r.is_empty() {
            return;
        }
        self.any = true;
        for ty in (r.y0 / TILE) as usize..=((r.y1 - 1) / TILE) as usize {
            let row = &mut self.tiles[ty * self.cols..(ty + 1) * self.cols];
            row[(r.x0 / TILE) as usize..=((r.x1 - 1) / TILE) as usize].fill(true);
        }
    }

    pub fn add_all(&mut self) {
        let all = IRect::of_size(self.size);
        self.add(all);
    }

    pub fn merge(&mut self, other: &Damage) {
        if other.size != self.size {
            self.add_all();
            return;
        }
        if !other.any {
            return;
        }
        self.any = true;
        for (a, b) in self.tiles.iter_mut().zip(&other.tiles) {
            *a |= *b;
        }
    }

    pub fn clear(&mut self) {
        if self.any {
            self.tiles.fill(false);
            self.any = false;
        }
    }

    /// The damaged runs of each tile row, as rectangles clipped to the
    /// frame, grouped by tile row: (tile row, runs).
    pub fn rows(&self) -> Vec<(usize, Vec<IRect>)> {
        let mut out = Vec::new();
        if !self.any {
            return out;
        }
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        for (ty, row) in self.tiles.chunks(self.cols.max(1)).enumerate() {
            let mut runs = Vec::new();
            let mut x = 0;
            while x < row.len() {
                if !row[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < row.len() && row[x] {
                    x += 1;
                }
                let y0 = ty as i32 * TILE;
                runs.push(IRect::new(start as i32 * TILE, y0, (x as i32 * TILE).min(w), (y0 + TILE).min(h)));
            }
            if !runs.is_empty() {
                out.push((ty, runs));
            }
        }
        out
    }

    /// The damaged rectangles: each tile row's runs, those of consecutive
    /// rows with the same span joined.
    pub fn rects(&self) -> Vec<IRect> {
        let mut out: Vec<IRect> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for (_, runs) in self.rows() {
            let mut next = Vec::new();
            for r in runs {
                match open.iter().find(|&&i| out[i].x0 == r.x0 && out[i].x1 == r.x1 && out[i].y1 == r.y0) {
                    Some(&i) => {
                        out[i].y1 = r.y1;
                        next.push(i);
                    }
                    None => {
                        out.push(r);
                        next.push(out.len() - 1);
                    }
                }
            }
            open = next;
        }
        out
    }

}

/// The rows of `data` (an image covering `area`, `stride` pixels a row),
/// split at the frame's tile rows: (first row's y, the rows). Each is
/// handed to one thread at a time.
pub fn tile_rows<T>(data: &mut [T], stride: usize, area: IRect) -> Vec<(i32, &mut [T])> {
    let mut out = Vec::new();
    let mut rest = data;
    let mut y = area.y0;
    while y < area.y1 {
        let next = ((y.div_euclid(TILE) + 1) * TILE).min(area.y1);
        let (band, tail) = rest.split_at_mut((next - y) as usize * stride);
        out.push((y, band));
        rest = tail;
        y = next;
    }
    out
}

/// The threads that draw (`--threads`): with one, everything runs on the
/// calling thread.
pub struct Pool {
    pool: Option<rayon::ThreadPool>,
    threads: usize,
}

impl Pool {
    /// `threads` workers; 0: one per core.
    pub fn new(threads: usize) -> Self {
        let threads = if threads == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()) } else { threads };
        let pool = (threads > 1)
            .then(|| rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("draw {i}")).build().ok())
            .flatten();
        Pool { threads: if pool.is_some() { threads } else { 1 }, pool }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Runs `f` on every item, on the workers if there are several.
    pub fn each<T: Send>(&self, items: Vec<T>, f: impl Fn(T) + Sync + Send) {
        match &self.pool {
            Some(pool) if items.len() > 1 => {
                use rayon::prelude::*;
                pool.install(|| items.into_par_iter().for_each(f));
            }
            _ => items.into_iter().for_each(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_tiles_and_rects() {
        let mut d = Damage::new((100, 70));
        assert!(d.is_empty() && d.rects().is_empty());
        d.add(IRect::new(5, 5, 6, 6));
        d.add(IRect::new(40, 40, 41, 41));
        assert_eq!(d.rects(), vec![IRect::new(0, 0, 32, 32), IRect::new(32, 32, 64, 64)]);
        d.add(IRect::new(-10, 33, 200, 34));
        let rows = d.rows();
        assert_eq!(rows[1], (1, vec![IRect::new(0, 32, 100, 64)]));
        d.add_all();
        assert_eq!(d.rects(), vec![IRect::new(0, 0, 100, 70)]);
        d.clear();
        assert!(d.is_empty());
    }

    #[test]
    fn rows_split_at_tile_rows() {
        let area = IRect::new(0, 20, 3, 70);
        let mut data = vec![0u8; 3 * 50];
        let rows = tile_rows(&mut data, 3, area);
        let spans: Vec<(i32, usize)> = rows.iter().map(|(y, r)| (*y, r.len() / 3)).collect();
        assert_eq!(spans, vec![(20, 12), (32, 32), (64, 6)]);
    }

    #[test]
    fn blending_rounds() {
        assert_eq!(mix(pack(0, 0, 0), pack(255, 255, 255), 128), pack(128, 128, 128));
        assert_eq!(mix(pack(10, 20, 30), pack(200, 100, 0), 0), pack(10, 20, 30));
        assert_eq!(mix(pack(10, 20, 30), pack(200, 100, 0), 255), pack(200, 100, 0));
        assert_eq!(over(pack(0, 0, 0), [255.0, 0.0, 100.0], 0.5), pack(128, 0, 50));
    }
}
