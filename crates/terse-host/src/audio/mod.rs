//! Plays the sound device (SPEC §sound.presentation) through the system's
//! audio output, and says so when it cannot (§sound.presentation: the
//! host tells the user once, the machines never know).
//!
//! Where the sound goes (`TERSE_AUDIO`, default `auto`):
//! - Linux: a PulseAudio server (PipeWire's `pipewire-pulse` or
//!   PulseAudio itself) through `libpulse-simple`, else ALSA's `default`
//!   PCM through `libasound` (which PipeWire, PulseAudio or the card
//!   serves). Both libraries are loaded when the first sound is made
//!   (dlopen), so building needs no audio headers and the binary links
//!   no audio library: a system without them is told why it is silent.
//! - macOS and Windows: cpal (CoreAudio, WASAPI).
//!
//! `TERSE_AUDIO=off` never opens anything and tells nobody (the user's
//! choice); `none` behaves as a host that found no output (for tests);
//! `pulse` or `pulse:<server>` (e.g. `pulse:unix:/run/…/native`) and
//! `alsa` or `alsa:<pcm>` choose one output (Linux).
//! `TERSE_AUDIO_STATS=1` prints the output's measured latency and
//! underruns on standard error when it closes.
//!
//! The output is opened when a machine first gives a frame that is not
//! silent, so a program that makes no sound never opens it. The samples
//! are presentation, not machine state: when the output falls behind, the
//! oldest are dropped; when it runs dry, it plays silence.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(all(feature = "audio", target_os = "linux"))]
mod alsa;
#[cfg(all(feature = "audio", not(target_os = "linux")))]
mod cpal_out;
#[cfg(all(feature = "audio", target_os = "linux"))]
mod pulse;

/// Sample pairs a second.
pub const RATE: u32 = terse_core::sound::RATE;
/// Sample pairs a frame.
const FRAME: usize = terse_core::sound::FRAME_SAMPLES;
/// The output's own buffer, in pairs: three frames (50 ms), or
/// `TERSE_AUDIO_BUFFER_MS` (for measuring)...
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(crate) fn buffer() -> usize {
    std::env::var("TERSE_AUDIO_BUFFER_MS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(3 * FRAME, |ms| (ms * RATE as usize / 1000).clamp(FRAME / 4, 20 * FRAME))
}

/// ...which starts playing once it holds two thirds of that.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(crate) fn start() -> usize {
    buffer() * 2 / 3
}
/// At most this many pairs wait in front of the output (two frames):
/// more is latency, and the oldest go.
const MAX_QUEUED: usize = 2 * FRAME;
/// After the output could not be opened or stopped, the next try waits
/// this long.
const RETRY: Duration = Duration::from_secs(5);

/// Where the sound goes (`TERSE_AUDIO`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The system's usual output, whichever works.
    Auto,
    /// Never play and never say (the user's choice, a self-test).
    Off,
    /// As if no output were found (tests the notice).
    None,
    /// A PulseAudio server: the default one, or this one.
    Pulse(Option<String>),
    /// This ALSA PCM.
    Alsa(String),
    /// `TERSE_AUDIO` holds something else.
    Bad(String),
}

impl Choice {
    pub fn parse(v: &str) -> Choice {
        match v {
            "" | "auto" => Choice::Auto,
            "off" => Choice::Off,
            "none" => Choice::None,
            "pulse" => Choice::Pulse(None),
            "alsa" => Choice::Alsa("default".into()),
            _ => match v.split_once(':') {
                Some(("pulse", s)) if !s.is_empty() => Choice::Pulse(Some(s.into())),
                Some(("alsa", p)) if !p.is_empty() => Choice::Alsa(p.into()),
                _ => Choice::Bad(v.into()),
            },
        }
    }

    /// `TERSE_AUDIO`; unset, `auto`, except in a self-test (which runs
    /// unattended and never makes a sound unless told where).
    pub fn from_env(selftest: bool) -> Choice {
        match std::env::var("TERSE_AUDIO") {
            Ok(v) => Choice::parse(&v),
            Err(_) if selftest => Choice::Off,
            Err(_) => Choice::Auto,
        }
    }
}

/// Why sound cannot be heard: a short cause for the notice (the user is
/// told again only when it changes) and the details for standard error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Silence {
    pub cause: String,
    pub detail: String,
}

impl Silence {
    fn new(cause: &str, detail: impl Into<String>) -> Silence {
        Silence { cause: cause.into(), detail: detail.into() }
    }

    /// The notice: heading, what, then.
    pub fn notice(&self) -> (String, String, String) {
        (
            "no sound".into(),
            format!("Sound is playing but cannot be heard: {}.", self.cause),
            "It needs PipeWire, PulseAudio or ALSA. terse-host --hardware-check says more.".into(),
        )
    }

    /// The line for standard error (after the program's name and `: `).
    pub fn line(&self) -> String {
        format!(
            "no sound: {} ({}); the machines run as usual. Sound plays through PipeWire or PulseAudio (libpulse-simple.so.0), else ALSA (libasound.so.2); `terse-host --hardware-check` says what this host finds, TERSE_AUDIO=off silences this",
            self.cause, self.detail
        )
    }
}

/// The cause when this host has no output code at all.
pub const BUILT_WITHOUT: &str = "this terse-host was built without sound";

/// What this build can play through, without opening anything
/// (`--version`).
pub fn compiled() -> &'static str {
    if cfg!(not(feature = "audio")) {
        "none (built without the `audio` feature)"
    } else if cfg!(target_os = "linux") {
        "PulseAudio or PipeWire (libpulse-simple.so.0), else ALSA (libasound.so.2), loaded when a program first makes sound"
    } else if cfg!(target_os = "macos") {
        "CoreAudio (cpal)"
    } else if cfg!(windows) {
        "WASAPI (cpal)"
    } else {
        "the system's default output (cpal)"
    }
}

/// Opens the output `TERSE_AUDIO` names and closes it again, without a
/// sample (`--hardware-check`): what it plays through, or why not.
pub fn probe() -> Result<String, Silence> {
    match Choice::from_env(false) {
        Choice::Off => Ok("off (TERSE_AUDIO=off)".into()),
        c => Output::open(&c).map(|o| o.name.clone()),
    }
}

/// The samples waiting for the output and what became of it.
#[derive(Default)]
pub(crate) struct Queue {
    pub samples: VecDeque<i16>,
    /// The output is closing.
    pub stop: bool,
    /// The output stopped working, and why.
    pub dead: Option<String>,
    /// Pairs dropped because too many waited.
    pub dropped: u64,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub q: Mutex<Queue>,
    pub cv: Condvar,
}

impl Shared {
    fn push(&self, frame: &[i16]) {
        let mut q = self.q.lock().unwrap();
        q.samples.extend(frame);
        let excess = q.samples.len().saturating_sub(2 * MAX_QUEUED);
        if excess > 0 {
            q.samples.drain(..excess);
            q.dropped += excess as u64 / 2;
        }
        self.cv.notify_one();
    }
}

/// An output fed by a thread of its own that blocks in `write`.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(crate) trait Writer: Send {
    /// Interleaved pairs; blocks while the output's buffer is full.
    fn write(&mut self, s: &[i16]) -> Result<(), String>;
    /// Microseconds from now until a pair written now is heard.
    fn delay_us(&mut self) -> Option<u64>;
    /// Times the output ran dry, if it can tell.
    fn underruns(&self) -> Option<u64>;
    /// Waits until what was written has been played (when closing).
    fn drain(&mut self) {}
}

/// What an output measured (`TERSE_AUDIO_STATS`).
#[derive(Default)]
struct Stats {
    writes: u64,
    pairs: u64,
    lat_sum: f64,
    lat_n: u64,
    lat_min: f64,
    lat_max: f64,
    underruns: Option<u64>,
}

enum Keep {
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    Thread(Option<std::thread::JoinHandle<Stats>>),
    #[cfg(all(feature = "audio", not(target_os = "linux")))]
    Stream(#[allow(dead_code)] cpal_out::Stream),
}

/// An open audio output.
pub struct Output {
    shared: Arc<Shared>,
    /// What it plays through, for people.
    pub name: String,
    keep: Keep,
}

impl Output {
    /// The output `choice` names, at 48,000 Hz, stereo, or why not.
    pub fn open(choice: &Choice) -> Result<Output, Silence> {
        let shared = Arc::new(Shared::default());
        match choice {
            Choice::Off | Choice::None => Err(Silence::new("no audio output was found", "TERSE_AUDIO=none")),
            Choice::Bad(v) => Err(Silence::new(
                "TERSE_AUDIO is not understood",
                format!("TERSE_AUDIO={v}: it is auto, off, none, pulse, pulse:<server>, alsa or alsa:<pcm>"),
            )),
            _ if cfg!(not(feature = "audio")) => Err(Silence::new(BUILT_WITHOUT, "the `audio` cargo feature was off")),
            #[cfg(feature = "audio")]
            c => backend(c, shared),
            #[cfg(not(feature = "audio"))]
            _ => unreachable!("{shared:p}"),
        }
    }

    /// One frame's samples, left and right interleaved.
    pub fn push(&self, frame: &[i16]) {
        self.shared.push(frame);
    }

    /// Why it stopped working, if it did.
    pub fn dead(&self) -> Option<String> {
        self.shared.q.lock().unwrap().dead.clone()
    }

    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    fn threaded(name: String, shared: Arc<Shared>, w: Box<dyn Writer>) -> Output {
        let s = shared.clone();
        let t = std::thread::Builder::new().name("terse-audio".into()).spawn(move || feed(&s, w)).expect("a thread for the sound");
        Output { shared, name, keep: Keep::Thread(Some(t)) }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        {
            let mut q = self.shared.q.lock().unwrap();
            q.stop = true;
            self.shared.cv.notify_all();
        }
        let stats = match &mut self.keep {
            Keep::Thread(t) => t.take().and_then(|t| t.join().ok()),
            #[cfg(all(feature = "audio", not(target_os = "linux")))]
            Keep::Stream(_) => None,
        };
        if std::env::var_os("TERSE_AUDIO_STATS").is_some() {
            let dropped = self.shared.q.lock().unwrap().dropped;
            match stats {
                Some(s) => eprintln!(
                    "terse-host: sound stats: {}: {:.1} s in {} writes; latency (queue + output) mean {:.1} ms, min {:.1}, max {:.1}; underruns {}; pairs dropped {dropped}",
                    self.name,
                    s.pairs as f64 / RATE as f64,
                    s.writes,
                    if s.lat_n > 0 { s.lat_sum / s.lat_n as f64 } else { 0.0 },
                    s.lat_min,
                    s.lat_max,
                    s.underruns.map_or("unknown".into(), |u| u.to_string()),
                ),
                None => eprintln!("terse-host: sound stats: {}: pairs dropped {dropped}", self.name),
            }
        }
    }
}

/// The output's thread: waits for samples and writes them, a frame at
/// most at a time, until the output closes or fails.
fn feed(shared: &Shared, mut w: Box<dyn Writer>) -> Stats {
    let stats_on = std::env::var_os("TERSE_AUDIO_STATS").is_some();
    let mut st = Stats { lat_min: f64::MAX, ..Default::default() };
    let mut chunk = Vec::with_capacity(2 * FRAME);
    loop {
        let waiting;
        {
            let mut q = shared.q.lock().unwrap();
            while q.samples.is_empty() && !q.stop {
                q = shared.cv.wait_timeout(q, Duration::from_millis(100)).unwrap().0;
            }
            if q.stop {
                // Closing: what is left (two frames at most) is still
                // played, so that a program's last sound is not cut.
                let rest: Vec<i16> = q.samples.drain(..).collect();
                drop(q);
                if rest.is_empty() || w.write(&rest).is_ok() {
                    w.drain();
                }
                break;
            }
            let n = q.samples.len().min(2 * FRAME);
            chunk.clear();
            chunk.extend(q.samples.drain(..n));
            waiting = q.samples.len() / 2;
        }
        if stats_on && let Some(d) = w.delay_us() {
            let ms = d as f64 / 1000.0 + (waiting + chunk.len() / 2) as f64 * 1000.0 / RATE as f64;
            st.lat_sum += ms;
            st.lat_n += 1;
            st.lat_min = st.lat_min.min(ms);
            st.lat_max = st.lat_max.max(ms);
        }
        if let Err(e) = w.write(&chunk) {
            shared.q.lock().unwrap().dead = Some(e);
            break;
        }
        st.writes += 1;
        st.pairs += chunk.len() as u64 / 2;
    }
    if st.lat_n == 0 {
        st.lat_min = 0.0;
    }
    st.underruns = w.underruns();
    st
}

#[cfg(all(feature = "audio", target_os = "linux"))]
fn backend(c: &Choice, shared: Arc<Shared>) -> Result<Output, Silence> {
    let none = "no audio output was found";
    match c {
        Choice::Pulse(server) => {
            let (w, name) = pulse::open(server.as_deref()).map_err(|e| Silence::new(none, format!("PulseAudio: {e}")))?;
            Ok(Output::threaded(name, shared, Box::new(w)))
        }
        Choice::Alsa(pcm) => {
            let (w, name) = alsa::open(pcm).map_err(|e| Silence::new(none, format!("ALSA: {e}")))?;
            Ok(Output::threaded(name, shared, Box::new(w)))
        }
        _ => {
            // A sound server first (PipeWire or PulseAudio, the usual
            // desktop), then ALSA (a server through its ALSA plugin, or the
            // card itself).
            let p = match pulse::open(None) {
                Ok((w, name)) => return Ok(Output::threaded(name, shared, Box::new(w))),
                Err(e) => e,
            };
            match alsa::open("default") {
                Ok((w, name)) => Ok(Output::threaded(name, shared, Box::new(w))),
                Err(a) => Err(Silence::new(none, format!("PulseAudio/PipeWire: {p}; ALSA: {a}"))),
            }
        }
    }
}

#[cfg(all(feature = "audio", not(target_os = "linux")))]
fn backend(c: &Choice, shared: Arc<Shared>) -> Result<Output, Silence> {
    match c {
        Choice::Pulse(_) | Choice::Alsa(_) => Err(Silence::new("TERSE_AUDIO is not understood", "pulse and alsa are for Linux")),
        _ => {
            let (stream, name) = cpal_out::open(shared.clone()).map_err(|e| Silence::new("no audio output was found", e))?;
            Ok(Output { shared, name, keep: Keep::Stream(stream) })
        }
    }
}

/// The host's volume, applied to the mixed samples on their way to the
/// output: never to the machines (their state, `--audio-out`). Kept in
/// `<config>/sound` (lines `volume <0-100>` and `mute on|off`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    /// 0 to 100.
    pub level: u8,
    pub muted: bool,
}

impl Default for Volume {
    fn default() -> Volume {
        Volume { level: 100, muted: false }
    }
}

/// The file in the config directory that keeps the volume.
pub const SOUND_FILE: &str = "sound";

/// A change of the volume asked for by a key the host keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKey {
    Down,
    Up,
    Mute,
}

impl Volume {
    /// The factor, in 1/65,536: the level squared (so that equal steps
    /// sound about equally large), 0 when muted.
    pub fn gain(&self) -> i32 {
        if self.muted { 0 } else { (self.level.min(100) as i32).pow(2) * 65_536 / 10_000 }
    }

    /// `samples` at this volume.
    pub fn apply(&self, samples: &mut [i16]) {
        let g = self.gain();
        if g == 65_536 {
            return;
        }
        for s in samples {
            *s = ((*s as i32 * g) >> 16) as i16;
        }
    }

    /// The volume kept in `<config>/sound`, else full.
    pub fn load(config: &std::path::Path) -> Volume {
        let mut v = Volume::default();
        let Ok(text) = std::fs::read_to_string(config.join(SOUND_FILE)) else { return v };
        for line in text.lines() {
            match line.split_whitespace().collect::<Vec<_>>()[..] {
                ["volume", n] => {
                    if let Ok(n) = n.parse::<u8>() {
                        v.level = n.min(100);
                    }
                }
                ["mute", m] => v.muted = m == "on",
                _ => {}
            }
        }
        v
    }

    pub fn save(&self, config: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(config).map_err(|e| format!("{}: {e}", config.display()))?;
        let p = config.join(SOUND_FILE);
        let text = format!("volume {}\nmute {}\n", self.level, if self.muted { "on" } else { "off" });
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The key's change: a tenth down or up (unmuting), or mute toggled.
    pub fn change(&mut self, k: VolumeKey) {
        match k {
            VolumeKey::Down => {
                self.level = self.level.saturating_sub(10);
                self.muted = false;
            }
            VolumeKey::Up => {
                self.level = (self.level + 10).min(100);
                self.muted = false;
            }
            VolumeKey::Mute => self.muted = !self.muted,
        }
    }

    /// For the notice: "Volume 70%." or "Muted (volume 70%).".
    pub fn describe(&self) -> String {
        if self.muted { format!("Muted (volume {}%).", self.level) } else { format!("Volume {}%.", self.level) }
    }
}

/// The volume's start and where it is kept, set once by the program
/// before its window opens (`configure`).
static SETTINGS: std::sync::OnceLock<(Option<std::path::PathBuf>, Volume)> = std::sync::OnceLock::new();

/// Where the volume is kept (`<config>/sound`) and what overrides it for
/// this run (`--volume`, `--mute`).
pub fn configure(config: Option<std::path::PathBuf>, volume: Option<u8>, mute: bool) {
    let mut v = config.as_deref().map(Volume::load).unwrap_or_default();
    if let Some(n) = volume {
        v.level = n.min(100);
        v.muted = false;
    }
    if mute {
        v.muted = true;
    }
    let _ = SETTINGS.set((config, v));
}

/// The host's sound: closed until a machine first makes a sound; says
/// once why it cannot be heard, and again only when that changes.
pub struct Audio {
    choice: Choice,
    output: Option<Output>,
    /// The cause the user was told last, while it holds.
    told: Option<String>,
    /// No new try before then.
    retry_at: Option<Instant>,
    volume: Volume,
    /// Where a changed volume is kept.
    config: Option<std::path::PathBuf>,
}

impl Audio {
    pub fn new(choice: Choice) -> Audio {
        let (config, volume) = SETTINGS.get().cloned().unwrap_or_default();
        Audio { choice, output: None, told: None, retry_at: None, volume, config }
    }

    /// A volume key: the volume changed and kept; what to show.
    pub fn change(&mut self, k: VolumeKey) -> String {
        self.volume.change(k);
        if let Some(c) = &self.config
            && let Err(e) = self.volume.save(c)
        {
            eprintln!("terse-host: the volume is not kept: {e}");
        }
        self.volume.describe()
    }

    pub fn volume(&self) -> Volume {
        self.volume
    }

    /// The system frame just run: its mixed samples, if any machine gave
    /// some. Returns why sound cannot be heard when the user must now be
    /// told (the first time, or the cause changed).
    pub fn frame(&mut self, samples: Option<&[i16]>) -> Option<Silence> {
        let s = samples?;
        if self.choice == Choice::Off {
            return None;
        }
        // A frame with sound opens the output, muted or not, so that
        // unmuting is heard at once.
        let sound = s.iter().any(|&x| x != 0);
        // The volume (and mute) applies only on the way out.
        let mut buf = [0i16; 2 * FRAME];
        let s = if self.volume.gain() == 65_536 || s.len() != buf.len() {
            s
        } else {
            buf.copy_from_slice(s);
            self.volume.apply(&mut buf);
            &buf[..]
        };
        if let Some(o) = &self.output {
            match o.dead() {
                None => {
                    o.push(s);
                    return None;
                }
                Some(e) => {
                    let name = o.name.clone();
                    self.output = None;
                    self.retry_at = Some(Instant::now() + RETRY);
                    return self.tell(Silence::new("the audio output stopped", format!("{name}: {e}")));
                }
            }
        }
        // Not open: only a frame with sound in it opens it.
        if !sound || self.retry_at.is_some_and(|t| Instant::now() < t) {
            return None;
        }
        match Output::open(&self.choice) {
            Ok(o) => {
                if self.told.take().is_some() {
                    eprintln!("terse-host: sound: playing again, through {}", o.name);
                }
                o.push(s);
                if std::env::var_os("TERSE_AUDIO_STATS").is_some() {
                    // For measuring the latency against a recording of the
                    // output: when the first sound left the host.
                    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                    eprintln!("terse-host: sound stats: first sound handed to {} at {:.6}", o.name, t.as_secs_f64());
                }
                self.output = Some(o);
                self.retry_at = None;
                None
            }
            Err(e) => {
                self.retry_at = Some(Instant::now() + RETRY);
                self.tell(e)
            }
        }
    }

    fn tell(&mut self, s: Silence) -> Option<Silence> {
        if self.told.as_deref() == Some(s.cause.as_str()) {
            return None;
        }
        self.told = Some(s.cause.clone());
        Some(s)
    }

    /// What it plays through now, if it is open.
    pub fn playing(&self) -> Option<&str> {
        self.output.as_ref().map(|o| o.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound() -> Vec<i16> {
        (0..2 * FRAME).map(|i| (i as i16 % 100) * 50).collect()
    }

    #[test]
    fn choices() {
        assert_eq!(Choice::parse(""), Choice::Auto);
        assert_eq!(Choice::parse("off"), Choice::Off);
        assert_eq!(Choice::parse("pulse"), Choice::Pulse(None));
        assert_eq!(Choice::parse("pulse:unix:/x/native"), Choice::Pulse(Some("unix:/x/native".into())));
        assert_eq!(Choice::parse("alsa"), Choice::Alsa("default".into()));
        assert_eq!(Choice::parse("alsa:null"), Choice::Alsa("null".into()));
        assert_eq!(Choice::parse("loud"), Choice::Bad("loud".into()));
    }

    /// No output: told at the first sound, not before (silence opens
    /// nothing) and not again while the cause holds.
    #[test]
    fn told_once() {
        let mut a = Audio::new(Choice::None);
        assert_eq!(a.frame(None), None);
        assert_eq!(a.frame(Some(&[0; 2 * FRAME])), None);
        let s = a.frame(Some(&sound())).expect("told at the first sound");
        assert_eq!(s.cause, "no audio output was found");
        assert!(s.line().contains("TERSE_AUDIO=none"), "{}", s.line());
        assert!(s.notice().1.len() <= terse_core::COLS - 2, "{}", s.notice().1);
        assert!(s.notice().2.len() <= terse_core::COLS - 2, "{}", s.notice().2);
        for _ in 0..10 {
            assert_eq!(a.frame(Some(&sound())), None);
        }
        // Another cause is told.
        a.retry_at = None;
        a.choice = Choice::Bad("x".into());
        assert_eq!(a.frame(Some(&sound())).map(|s| s.cause), Some("TERSE_AUDIO is not understood".into()));
    }

    #[test]
    fn off_tells_nobody() {
        let mut a = Audio::new(Choice::Off);
        assert_eq!(a.frame(Some(&sound())), None);
        assert!(a.playing().is_none());
    }

    /// A writer that records, standing in for an output.
    struct Rec(Arc<Mutex<Vec<i16>>>, usize);
    impl Writer for Rec {
        fn write(&mut self, s: &[i16]) -> Result<(), String> {
            if self.1 == 0 {
                return Err("gone".into());
            }
            self.1 -= 1;
            self.0.lock().unwrap().extend_from_slice(s);
            Ok(())
        }
        fn delay_us(&mut self) -> Option<u64> {
            None
        }
        fn underruns(&self) -> Option<u64> {
            None
        }
    }

    /// The thread plays what was pushed, in order; an output that stops
    /// is told (once) and the next sound tries again later.
    #[test]
    fn thread_plays_then_dies() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(Shared::default());
        let o = Output::threaded("test".into(), shared, Box::new(Rec(got.clone(), 2)));
        let one = sound();
        o.push(&one);
        let t = Instant::now();
        while got.lock().unwrap().len() < one.len() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(*got.lock().unwrap(), one);
        o.push(&one);
        o.push(&one);
        let t = Instant::now();
        while o.dead().is_none() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(o.dead().as_deref(), Some("gone"));
        let mut a = Audio::new(Choice::None);
        a.output = Some(o);
        let s = a.frame(Some(&one)).expect("told it stopped");
        assert_eq!(s.cause, "the audio output stopped");
        assert!(a.output.is_none() && a.retry_at.is_some());
        assert_eq!(a.frame(Some(&one)), None, "no new try at once");
    }

    #[test]
    fn volume() {
        let mut v = Volume::default();
        let mut s = [10_000i16, -10_000, 32_767, -32_768];
        v.apply(&mut s);
        assert_eq!(s, [10_000, -10_000, 32_767, -32_768], "full volume leaves the samples alone");
        v.change(VolumeKey::Down);
        v.change(VolumeKey::Down);
        assert_eq!(v.level, 80);
        let mut s = [10_000i16, -10_000];
        v.apply(&mut s);
        assert_eq!(s, [6_399, -6_400]);
        v.change(VolumeKey::Mute);
        assert_eq!(v.describe(), "Muted (volume 80%).");
        let mut s = [10_000i16];
        v.apply(&mut s);
        assert_eq!(s, [0]);
        v.change(VolumeKey::Up);
        assert_eq!(v, Volume { level: 90, muted: false });
        for _ in 0..5 {
            v.change(VolumeKey::Up);
        }
        assert_eq!(v.level, 100);
        let dir = tempfile::tempdir().unwrap();
        v.level = 30;
        v.muted = true;
        v.save(dir.path()).unwrap();
        assert_eq!(Volume::load(dir.path()), v);
        assert_eq!(Volume::load(&dir.path().join("none")), Volume::default());
    }

    /// Too much waiting: the oldest go.
    #[test]
    fn queue_drops_oldest() {
        let s = Shared::default();
        for i in 0..5 {
            s.push(&[i as i16; 2 * FRAME]);
        }
        let q = s.q.lock().unwrap();
        assert_eq!(q.samples.len(), 2 * MAX_QUEUED);
        assert_eq!(q.samples[0], 3);
        assert_eq!(q.dropped, 3 * FRAME as u64);
    }
}
