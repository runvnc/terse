//! The start-up hardware check (`PERFORMANCE.md` R16): the host draws each
//! device's worst legal frame (`terse_core::worst`, the images behind
//! `terse perf`'s W rows) with the renderer it will use, and keeps the
//! result per host build and computer, so it runs once, not at every
//! start. Where this computer cannot keep up, the host does not offer what
//! is optional (the phosphor look) and says which device's worst frames it
//! cannot draw in time; a late frame at run time then means misbehaviour,
//! not weak hardware.

use std::path::PathBuf;
use std::time::Instant;

use terse_core::{Image, Script, System};
use terse_net::HeadlessSystem;
use terse_perflog::env::Environment;
use terse_perflog::judge::FRAME_NS;
use terse_perflog::{Component, Line};

use crate::Looks;

/// The worst legal frames checked: per device its costliest at the caps,
/// and the look at its costliest.
pub const SCENARIOS: [(&str, u8, bool); 5] =
    [("2d-rects", 3, false), ("3d-points-faces", 9, false), ("term-basic", 2, false), ("2d-lines-phosphor", 3, true), ("3d-points-phosphor", 9, true)];
/// Frames drawn of each (the first is not timed: it builds caches).
const FRAMES: u32 = 4;

/// What the check found: per scenario, the slowest timed frame (ms).
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub env: String,
    pub results: Vec<(String, f64)>,
}

impl Check {
    /// `ok`, `shed` (the look is not offered) or `slow` (the device's
    /// worst legal frame takes more than a frame: R16 says such a host
    /// should not provide it; the reference reports it) for each scenario.
    pub fn verdicts(&self) -> Vec<(String, u8, f64, &'static str)> {
        let frame = FRAME_NS as f64 / 1e6;
        self.results
            .iter()
            .filter_map(|(id, ms)| {
                let (_, slot, look) = SCENARIOS.iter().find(|s| s.0 == id)?;
                let v = match (look, *ms) {
                    (true, m) if m > frame / 2.0 => "shed",
                    (false, m) if m > frame => "slow",
                    _ => "ok",
                };
                Some((id.clone(), *slot, *ms, v))
            })
            .collect()
    }

    /// Whether the look is not to be offered.
    pub fn shed_look(&self) -> bool {
        self.verdicts().iter().any(|v| v.3 == "shed")
    }

    /// The record's `check` lines.
    pub fn lines(&self) -> Vec<Line> {
        self.verdicts().into_iter().map(|(_, slot, ms, v)| Line::Check { component: Component::Device(slot), ms: (ms * 10.0).round() / 10.0, result: v.into() }).collect()
    }

    fn encode(&self) -> String {
        let mut s = format!("{}\n", self.env);
        for (id, ms) in &self.results {
            s.push_str(&format!("{id} {ms:.2}\n"));
        }
        s
    }

    fn decode(text: &str) -> Option<Check> {
        let mut lines = text.lines();
        let env = lines.next()?.to_string();
        let results = lines.map(|l| l.split_once(' ').and_then(|(a, b)| Some((a.to_string(), b.parse().ok()?)))).collect::<Option<Vec<_>>>()?;
        Some(Check { env, results })
    }
}

/// The computer and the renderer asked for, as the cache keys them.
fn env_key(looks: &Looks) -> String {
    let renderer = match looks.renderer {
        crate::display::RendererChoice::Software => "software",
        crate::display::RendererChoice::Gpu => "gpu",
        crate::display::RendererChoice::Auto => "auto",
    };
    Line::Env(Environment::detect(renderer)).encode()
}

/// Draws the scenarios with `looks`'s renderer; returns the check.
pub fn run(looks: &Looks) -> Result<Check, String> {
    let mut display = crate::headless_display(looks)?;
    display.configure(false, false, true);
    let mut look = looks.look;
    look.floor = 0;
    display.set_look(look);
    let viewport = display.grid_size();
    let scenarios = terse_core::worst::scenarios();
    let mut results = Vec::new();
    for (id, _, _) in SCENARIOS {
        let Some(s) = scenarios.iter().find(|s| s.id == id) else { continue };
        let image = Image::parse(&s.image).map_err(|e| format!("{id}: {e}"))?;
        let mut system = System::new(&image, false, Vec::new());
        let mut headless = HeadlessSystem::new(None, Script::default());
        let shown = |_: u32| None;
        let mut worst: f64 = 0.0;
        for f in 0..FRAMES {
            headless.frame(&mut system);
            let start = Instant::now();
            display.update(&system, viewport, &[0], &shown, None);
            display.render_offscreen(viewport)?;
            if f > 0 {
                worst = worst.max(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        results.push((id.to_string(), worst));
    }
    Ok(Check { env: env_key(looks), results })
}

/// The cached check of this build on this computer, or a new one (cached
/// in `<cache>/terse/hwcheck-<size>-<mtime>.txt` of the program file). `None` if it cannot be made.
pub fn cached(looks: &Looks, cache: Option<PathBuf>) -> Option<(Check, bool)> {
    // The build, known without hashing the program (start-up must not
    // wait for that): its file's size and modification time.
    let build = std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| format!("{}-{}", m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())))
        .unwrap_or_default();
    let path = cache.map(|d| d.join(format!("hwcheck-{build}.txt")));
    if let Some(p) = &path
        && let Some(c) = std::fs::read_to_string(p).ok().and_then(|t| Check::decode(&t))
        && c.env == env_key(looks)
    {
        return Some((c, true));
    }
    let c = run(looks).ok()?;
    if let Some(p) = &path {
        let _ = p.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(p, c.encode());
    }
    Some((c, false))
}

/// `$XDG_CACHE_HOME/terse`, else `~/.cache/terse`.
pub fn default_cache() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("terse"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_and_cache_format() {
        let c = Check { env: "env x".into(), results: vec![("2d-rects".into(), 6.5), ("2d-lines-phosphor".into(), 9.0), ("term-basic".into(), 20.0)] };
        let v: Vec<&str> = c.verdicts().iter().map(|v| v.3).collect();
        assert_eq!(v, ["ok", "shed", "slow"]);
        assert!(c.shed_look());
        assert_eq!(Check::decode(&c.encode()), Some(c.clone()));
        assert_eq!(c.lines()[2], Line::Check { component: Component::Device(2), ms: 20.0, result: "slow".into() });
    }
}
