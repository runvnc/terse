//! Device 7, Page (SPEC 11): typeset proportional text.
//!
//! The device only validates and copies the document and keeps the
//! rectangle, the scroll value and the background. Laying the text out is
//! the host's business: [`parse`] turns a document into paragraphs of styled
//! runs (the one decoder every host uses), and [`reference_layout`] is the
//! deterministic layout of SPEC 11.6 that a host without fonts (`terse run`,
//! the tests) uses. [`PageWatch`] decides when a host reports the height
//! (event `08h`), and [`PageInput`] does that and the anchor events for a
//! headless host.

use std::fmt::Write as _;

use crate::system::{Event, SCREEN_HEIGHT, SCREEN_WIDTH};
use crate::wide::is_wide;

/// Longest document a submit accepts (SPEC 11.2).
pub const MAX_DOCUMENT: usize = 65_536;
/// Results of `submit`.
pub const RESULT_DONE: u32 = 0;
pub const RESULT_REJECTED: u32 = 1;
/// The background after reset: opaque black (`r | g << 8 | b << 16 | a << 24`).
pub const DEFAULT_BACKGROUND: u32 = 0xFF00_0000;
/// Largest height event `08h` can carry (24 bits).
pub const MAX_REPORTED_HEIGHT: u32 = 0xFF_FFFF;

/// Document opcodes (SPEC 11.3).
pub mod op {
    pub const END: u8 = 0x00;
    pub const TEXT: u8 = 0x01;
    pub const FONT: u8 = 0x02;
    pub const SIZE: u8 = 0x03;
    pub const COLOUR: u8 = 0x04;
    pub const PARAGRAPH: u8 = 0x05;
    pub const RULE: u8 = 0x06;
    pub const ANCHOR: u8 = 0x07;
}

/// Font faces (SPEC 11.3).
pub mod face {
    pub const SANS: u8 = 0;
    pub const SERIF: u8 = 1;
    pub const MONO: u8 = 2;
}

/// Sizes are clamped to this range (SPEC 11.3).
pub const MIN_SIZE: u8 = 6;
pub const MAX_SIZE: u8 = 96;
/// Initial text state (SPEC 11.3).
pub const DEFAULT_SIZE: u8 = 14;
pub const DEFAULT_COLOUR: [u8; 3] = [229, 229, 229];
/// Height of a rule paragraph in screen units (SPEC 11.3).
pub const RULE_HEIGHT: u32 = 8;

/// The rectangle in screen units: x, y, width, height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Region {
    /// Whether the page is shown at all (a width or height of 0 hides it).
    pub fn visible(&self) -> bool {
        self.w > 0 && self.h > 0
    }
}

/// The device state (SPEC 11.7): the document, the rectangle, the scroll
/// value and the background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// `None`: no document (after reset and `clear`).
    document: Option<Vec<u8>>,
    region: Region,
    scroll: u32,
    background: u32,
    /// Changes at every accepted submit and every clear (not machine state;
    /// hosts use it to know when to lay out and report the height).
    generation: u64,
}

impl Default for Page {
    fn default() -> Self {
        Page { document: None, region: Region::default(), scroll: 0, background: DEFAULT_BACKGROUND, generation: 0 }
    }
}

impl Page {
    /// Replaces the document with `bytes` and sets the scroll value to 0 if
    /// it is 1 to 65,536 bytes long; else returns [`RESULT_REJECTED`] and
    /// keeps everything. The caller checks the buffer.
    pub fn submit(&mut self, bytes: &[u8]) -> u32 {
        if bytes.is_empty() || bytes.len() > MAX_DOCUMENT {
            return RESULT_REJECTED;
        }
        let doc = self.document.get_or_insert_with(Vec::new);
        doc.clear();
        doc.extend_from_slice(bytes);
        self.scroll = 0;
        self.generation += 1;
        RESULT_DONE
    }

    /// No document.
    pub fn clear(&mut self) {
        self.document = None;
        self.generation += 1;
    }

    /// Sets the rectangle, clamped so it lies on the screen.
    pub fn set_region(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let (sw, sh) = (SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
        let x = x.min(sw);
        let y = y.min(sh);
        self.region = Region { x: x as u16, y: y as u16, w: w.min(sw - x) as u16, h: h.min(sh - y) as u16 };
    }

    pub fn set_scroll(&mut self, y: u32) {
        self.scroll = y;
    }

    pub fn set_background(&mut self, rgba: u32) {
        self.background = rgba;
    }

    /// The document bytes, or `None` when there is no document.
    pub fn document(&self) -> Option<&[u8]> {
        self.document.as_deref()
    }

    pub fn region(&self) -> Region {
        self.region
    }

    /// The scroll value as stored (a signed number; see [`Page::scroll_for`]).
    pub fn scroll(&self) -> u32 {
        self.scroll
    }

    /// The scroll value the host presents for a document `height` units
    /// high: the stored value as a signed number, clamped to
    /// 0 … max(0, height − h).
    pub fn scroll_for(&self, height: u32) -> u32 {
        let max = height.saturating_sub(self.region.h as u32) as i64;
        (self.scroll as i32 as i64).clamp(0, max) as u32
    }

    /// The background as (r, g, b, a).
    pub fn background(&self) -> [u8; 4] {
        self.background.to_le_bytes()
    }

    /// Changes at every accepted submit and every clear.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The rectangle, scroll value, background and document (STATE.md,
    /// page section); not the generation counter.
    pub(crate) fn write_state(&self, out: &mut Vec<u8>) {
        let r = self.region;
        for v in [r.x, r.y, r.w, r.h] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.scroll.to_le_bytes());
        out.extend_from_slice(&self.background.to_le_bytes());
        // No document is length 0 (a document is at least one byte).
        let doc = self.document.as_deref().unwrap_or(&[]);
        out.extend_from_slice(&(doc.len() as u32).to_le_bytes());
        out.extend_from_slice(doc);
    }
}

/// Alignment of a paragraph, relative to its text direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Centre,
    End,
}

/// The text state a run was added with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    /// [`face::SANS`], [`face::SERIF`] or [`face::MONO`].
    pub face: u8,
    pub bold: bool,
    pub italic: bool,
    /// Screen units, 6 to 96.
    pub size: u8,
    pub rgb: [u8; 3],
    /// 0: no anchor.
    pub anchor: u16,
}

impl Default for Style {
    fn default() -> Self {
        Style { face: face::SANS, bold: false, italic: false, size: DEFAULT_SIZE, rgb: DEFAULT_COLOUR, anchor: 0 }
    }
}

/// Text in one style. The text is already cleaned (SPEC 11.3): no control
/// characters but U+000A (a line break); tabs are spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// A paragraph: text, or a rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paragraph {
    pub align: Align,
    /// Screen units from the start edge, for every line.
    pub indent: u8,
    /// Screen units of gap before the paragraph.
    pub space: u8,
    /// Lines break only at U+000A and are clipped at the rectangle's edge.
    pub nowrap: bool,
    pub runs: Vec<Run>,
    /// A rule paragraph: a line in this colour across the rectangle.
    pub rule: Option<[u8; 3]>,
}

impl Paragraph {
    /// Whether the paragraph has any characters (a rule has none).
    pub fn has_text(&self) -> bool {
        self.runs.iter().any(|r| !r.text.is_empty())
    }
}

/// Decodes text for a Text command: malformed UTF-8 as U+FFFD (the maximal
/// subpart rule, as the terminal does), tab as space, other control
/// characters (U+0000-U+001F but U+000A, U+007F-U+009F) dropped.
fn clean(bytes: &[u8], out: &mut String) {
    for ch in String::from_utf8_lossy(bytes).chars() {
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// Decodes a document (SPEC 11.3) into paragraphs. It ends at End, at the
/// end of the bytes, at an unknown opcode or at a command whose operands
/// are cut off; the commands before that are used. There is always at least
/// the implicit first paragraph.
pub fn parse(doc: &[u8]) -> Vec<Paragraph> {
    let mut paragraphs = vec![Paragraph::default()];
    // Properties of the latest Paragraph command, for text after a rule.
    let mut props = Paragraph::default();
    let mut style = Style::default();
    let mut at = 0;
    while at < doc.len() {
        let operands = match doc[at] {
            op::END => break,
            op::TEXT => match doc.get(at + 1..at + 3) {
                Some(n) => 2 + u16::from_le_bytes([n[0], n[1]]) as usize,
                None => break,
            },
            op::FONT => 2,
            op::SIZE => 1,
            op::COLOUR => 3,
            op::PARAGRAPH => 4,
            op::RULE => 0,
            op::ANCHOR => 2,
            _ => break,
        };
        let Some(args) = doc.get(at + 1..at + 1 + operands) else { break };
        match doc[at] {
            op::TEXT => {
                let last = paragraphs.last_mut().unwrap();
                if last.rule.is_some() {
                    // Text after a rule starts a paragraph like the latest one.
                    paragraphs.push(Paragraph { runs: Vec::new(), rule: None, ..props.clone() });
                }
                let p = paragraphs.last_mut().unwrap();
                match p.runs.last_mut() {
                    Some(run) if run.style == style => clean(&args[2..], &mut run.text),
                    _ => {
                        let mut text = String::new();
                        clean(&args[2..], &mut text);
                        p.runs.push(Run { text, style });
                    }
                }
            }
            op::FONT => {
                style.face = if args[0] <= face::MONO { args[0] } else { face::SANS };
                style.bold = args[1] & 1 != 0;
                style.italic = args[1] & 2 != 0;
            }
            op::SIZE => style.size = args[0].clamp(MIN_SIZE, MAX_SIZE),
            op::COLOUR => style.rgb = [args[0], args[1], args[2]],
            op::PARAGRAPH => {
                props = Paragraph {
                    align: match args[0] {
                        1 => Align::Centre,
                        2 => Align::End,
                        _ => Align::Start,
                    },
                    indent: args[1],
                    space: args[2],
                    nowrap: args[3] & 1 != 0,
                    runs: Vec::new(),
                    rule: None,
                };
                paragraphs.push(props.clone());
            }
            op::RULE => paragraphs.push(Paragraph { rule: Some(style.rgb), ..Paragraph::default() }),
            op::ANCHOR => style.anchor = u16::from_le_bytes([args[0], args[1]]),
            _ => unreachable!(),
        }
        at += 1 + operands;
    }
    for p in &mut paragraphs {
        p.runs.retain(|r| !r.text.is_empty());
    }
    paragraphs
}

/// A span of one style on a laid-out line: its x range in half screen units
/// from the rectangle's left edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub x0: u32,
    pub x1: u32,
    pub text: String,
    pub style: Style,
}

/// One line of the reference layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Top of the line, in screen units from the top of the document.
    pub y: u32,
    pub height: u32,
    /// The spans, left to right (empty for an empty line and a rule).
    pub spans: Vec<Span>,
    /// A rule: a line across the rectangle in this colour, at its middle.
    pub rule: Option<[u8; 3]>,
    /// Lines of a no-wrap paragraph may be wider than the rectangle.
    pub nowrap: bool,
}

/// The reference layout of a document at a width (SPEC 11.6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The document's height in screen units.
    pub height: u32,
    pub lines: Vec<Line>,
}

/// Advance of `ch` in half screen units at `size`: 0.5 × size, or 1 × size
/// for a wide character (the terminal's table; every character above
/// U+FFFF counts as wide).
fn advance(ch: char, size: u8) -> u32 {
    let wide = ch as u32 > 0xFFFF || is_wide(ch as u16);
    size as u32 * if wide { 2 } else { 1 }
}

/// Line height for a size: ceil(1.25 × size).
pub fn line_height(size: u8) -> u32 {
    (5 * size as u32).div_ceil(4)
}

/// The reference layout (SPEC 11.6): every character advances half its size
/// (a wide one its whole size), lines are ceil(1.25 × size) high (the
/// largest size on the line), text wraps greedily at spaces and a word
/// longer than the line is split; rules are 8 units high. Text is laid out
/// left to right whatever its script.
pub fn reference_layout(doc: &[u8], width: u16) -> Layout {
    let mut layout = Layout::default();
    let full = 2 * width as u32;
    let mut y: u32 = 0;
    for p in parse(doc) {
        y = y.saturating_add(p.space as u32);
        if let Some(rgb) = p.rule {
            layout.lines.push(Line { y, height: RULE_HEIGHT, spans: Vec::new(), rule: Some(rgb), nowrap: false });
            y = y.saturating_add(RULE_HEIGHT);
            continue;
        }
        if !p.has_text() {
            continue;
        }
        let avail = full.saturating_sub(2 * p.indent as u32);
        // Split into segments at line breaks; each segment remembers the
        // style in effect at its end, for the height of an empty line.
        let mut segments: Vec<(Vec<(char, Style)>, Style)> = vec![(Vec::new(), p.runs[0].style)];
        for run in &p.runs {
            for ch in run.text.chars() {
                let seg = segments.last_mut().unwrap();
                if ch == '\n' {
                    seg.1 = run.style;
                    segments.push((Vec::new(), run.style));
                } else {
                    seg.0.push((ch, run.style));
                    seg.1 = run.style;
                }
            }
        }
        for (chars, end_style) in segments {
            let mut lines: Vec<Vec<(char, Style)>> = Vec::new();
            if p.nowrap || chars.is_empty() {
                lines.push(chars);
            } else {
                wrap(&chars, avail, &mut lines);
            }
            for line in lines {
                let height = line.iter().map(|(_, s)| line_height(s.size)).max().unwrap_or(line_height(end_style.size));
                let w: u32 = line.iter().map(|&(c, s)| advance(c, s.size)).sum();
                let offset = match p.align {
                    Align::Start => 0,
                    Align::Centre => avail.saturating_sub(w) / 2,
                    Align::End => avail.saturating_sub(w),
                };
                let mut x = 2 * p.indent as u32 + offset;
                let mut spans: Vec<Span> = Vec::new();
                for (c, s) in line {
                    let a = advance(c, s.size);
                    match spans.last_mut() {
                        Some(span) if span.style == s => {
                            span.text.push(c);
                            span.x1 += a;
                        }
                        _ => spans.push(Span { x0: x, x1: x + a, text: c.to_string(), style: s }),
                    }
                    x += a;
                }
                layout.lines.push(Line { y, height, spans, rule: None, nowrap: p.nowrap });
                y = y.saturating_add(height);
            }
        }
    }
    layout.height = y;
    layout
}

/// Greedy wrapping at spaces within `avail` half units. Spaces where a line
/// breaks are dropped; a word wider than the line is split, at least one
/// character per line.
fn wrap(chars: &[(char, Style)], avail: u32, lines: &mut Vec<Vec<(char, Style)>>) {
    let adv = |&(c, s): &(char, Style)| advance(c, s.size);
    let mut line: Vec<(char, Style)> = Vec::new();
    let mut width = 0;
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        // Spaces before the next word.
        let space_start = i;
        while i < chars.len() && chars[i].0 == ' ' {
            i += 1;
        }
        let spaces = &chars[space_start..i];
        let word_start = i;
        while i < chars.len() && chars[i].0 != ' ' {
            i += 1;
        }
        let word = &chars[word_start..i];
        let space_w: u32 = spaces.iter().map(adv).sum();
        let word_w: u32 = word.iter().map(adv).sum();
        if first || width + space_w + word_w <= avail {
            // Leading spaces of the segment are kept; so are spaces that fit.
            line.extend_from_slice(spaces);
            width += space_w;
        } else {
            lines.push(std::mem::take(&mut line));
            width = 0;
        }
        first = false;
        if width + word_w <= avail {
            line.extend_from_slice(word);
            width += word_w;
            continue;
        }
        // Split the word.
        for c in word {
            let a = adv(c);
            if width + a > avail && !line.iter().all(|(c, _)| *c == ' ') {
                lines.push(std::mem::take(&mut line));
                width = 0;
            }
            line.push(*c);
            width += a;
        }
    }
    // Trailing spaces do not take room.
    lines.push(line);
}

impl Layout {
    /// The lines that intersect the rectangle when scrolled to `scroll`
    /// (already clamped) with a rectangle `h` units high.
    pub fn visible(&self, scroll: u32, h: u16) -> impl Iterator<Item = &Line> {
        let bottom = scroll.saturating_add(h as u32);
        self.lines.iter().filter(move |l| l.y < bottom && l.y.saturating_add(l.height) > scroll)
    }

    /// The anchor of the text at (x, y), in screen units from the rectangle's
    /// left edge and the document's top; 0 if none.
    pub fn anchor_at(&self, x: u32, y: u32) -> u16 {
        let hx = 2 * x;
        self.lines
            .iter()
            .filter(|l| l.y <= y && y < l.y + l.height)
            .flat_map(|l| &l.spans)
            .find(|s| s.x0 <= hx && hx < s.x1)
            .map_or(0, |s| s.style.anchor)
    }

    /// The visible lines as text (`terse run --dump-page`): a header with the
    /// height and scroll, then one line per visible line:
    /// `<y> <height> <x>  <face><size>[b][i] |<text>`, with anchored text as
    /// `[text]{id}`, a rule as `-- rule --`, and x in screen units.
    pub fn dump(&self, page: &Page) -> String {
        let r = page.region();
        let scroll = page.scroll_for(self.height);
        let mut out = format!(
            "-- page: height {}, scroll {} (stored {}), region {} {} {} {} --\n",
            self.height,
            scroll,
            page.scroll() as i32,
            r.x,
            r.y,
            r.w,
            r.h
        );
        if !r.visible() {
            return out;
        }
        for line in self.visible(scroll, r.h) {
            let _ = write!(out, "{:5} {:3} ", line.y, line.height);
            if line.rule.is_some() {
                out.push_str("    0  -- rule --\n");
                continue;
            }
            let Some(first) = line.spans.first() else {
                out.push_str("    0\n");
                continue;
            };
            let s = first.style;
            let face = ["sans", "serif", "mono"][s.face as usize];
            let _ = write!(
                out,
                "{:5}{} {face}{}{}{} |",
                first.x0 / 2,
                if first.x0 % 2 == 1 { ".5" } else { "  " },
                s.size,
                if s.bold { "b" } else { "" },
                if s.italic { "i" } else { "" }
            );
            for span in &line.spans {
                if span.style.anchor != 0 {
                    let _ = write!(out, "[{}]{{{}}}", span.text, span.style.anchor);
                } else {
                    out.push_str(&span.text);
                }
            }
            out.push('\n');
        }
        out
    }
}

/// When a host lays the page out and reports its height (SPEC 11.5): at
/// every new document (submit or clear) and when the rectangle's width
/// changes the height. Host bookkeeping, not machine state.
#[derive(Clone, Debug, Default)]
pub struct PageWatch {
    /// The (generation, width) laid out last.
    done: Option<(u64, u16)>,
    /// The generation and height reported last.
    reported: Option<(u64, u32)>,
}

/// What to lay out: a document generation at a width.
pub type LayoutKey = (u64, u16);

impl PageWatch {
    /// The layout the host should make now, if the page has changed since
    /// the last one. A document is laid out only once the rectangle has a
    /// width; no document has height 0 at any width.
    pub fn wanted(&self, page: &Page) -> Option<LayoutKey> {
        let width = if page.document().is_some() { page.region().w } else { 0 };
        let key = (page.generation(), width);
        if self.done == Some(key) || (page.document().is_some() && width == 0) {
            return None;
        }
        // The very first state (no document ever) needs no report.
        if self.done.is_none() && page.generation() == 0 {
            return None;
        }
        Some(key)
    }

    /// A layout for `key` finished with `height`: returns the event to send
    /// at the start of the next frame, if any (a new document, or a new
    /// height for the same one).
    pub fn finished(&mut self, key: LayoutKey, height: u32) -> Option<Event> {
        self.done = Some(key);
        let height = height.min(MAX_REPORTED_HEIGHT);
        if self.reported.is_some_and(|(g, h)| g == key.0 && h == height) {
            return None;
        }
        self.reported = Some((key.0, height));
        Some(Event::page_laid_out(height))
    }
}

/// Something that lays a document out for a host: the reference layout, or
/// a host's fonts.
pub trait Layouter {
    /// Lays `doc` out at `width` screen units and returns its height; the
    /// result is kept for [`Layouter::anchor_at`].
    fn layout(&mut self, doc: &[u8], width: u16) -> u32;
    /// The anchor of the text at (x, y) in the last layout, in screen units
    /// from the rectangle's left edge and the document's top; 0 if none.
    fn anchor_at(&self, x: u32, y: u32) -> u16;
    /// The height of the last layout.
    fn height(&self) -> u32;
}

/// The reference layout as a [`Layouter`].
#[derive(Clone, Debug, Default)]
pub struct ReferenceLayouter {
    pub layout: Layout,
}

impl Layouter for ReferenceLayouter {
    fn layout(&mut self, doc: &[u8], width: u16) -> u32 {
        self.layout = reference_layout(doc, width);
        self.layout.height
    }

    fn anchor_at(&self, x: u32, y: u32) -> u16 {
        self.layout.anchor_at(x, y)
    }

    fn height(&self) -> u32 {
        self.layout.height
    }
}

/// The page events of a host that lays out synchronously between frames
/// (`terse run`, `terse-host --screenshot`): the height one frame after a
/// change, and anchor events after pointer presses.
pub struct PageInput {
    watch: PageWatch,
    layouter: Box<dyn Layouter + Send>,
    /// Whether the layouter holds a layout of the current page.
    laid_out: bool,
    /// Buttons of the last pointer event.
    buttons: u8,
    /// The height event for the next frame.
    next: Option<Event>,
}

impl Default for PageInput {
    fn default() -> Self {
        Self::new(Box::new(ReferenceLayouter::default()))
    }
}

impl PageInput {
    pub fn new(layouter: Box<dyn Layouter + Send>) -> Self {
        PageInput { watch: PageWatch::default(), layouter, laid_out: false, buttons: 0, next: None }
    }

    /// The events for the next frame, given its other events: the pending
    /// height event first, and after each pointer event that presses a
    /// button over anchored text, an anchor event (SPEC 5).
    pub fn events(&mut self, page: Option<&Page>, events: &[Event]) -> Vec<Event> {
        let mut out = Vec::with_capacity(events.len() + 1);
        out.extend(self.next.take());
        for &e in events {
            out.push(e);
            if e.kind != Event::POINTER {
                continue;
            }
            let (x, y, buttons) = e.pointer_parts();
            let pressed = buttons & !self.buttons != 0;
            self.buttons = buttons;
            if let Some(page) = page.filter(|_| pressed && self.laid_out) {
                let anchor = anchor_at(page, self.layouter.as_ref(), x, y);
                if anchor != 0 {
                    out.push(Event::page_anchor(anchor, buttons));
                }
            }
        }
        out
    }

    /// After a frame: lays the page out if it changed and keeps the height
    /// event for the next frame.
    pub fn after_frame(&mut self, page: Option<&Page>) {
        let Some(page) = page else { return };
        if let Some(key) = self.watch.wanted(page) {
            let height = match page.document() {
                Some(doc) => {
                    self.laid_out = true;
                    self.layouter.layout(doc, key.1)
                }
                None => {
                    self.laid_out = false;
                    0
                }
            };
            if let Some(e) = self.watch.finished(key, height) {
                self.next = Some(e);
            }
        } else if page.document().is_none() {
            self.laid_out = false;
        }
    }

    pub fn layouter(&self) -> &dyn Layouter {
        self.layouter.as_ref()
    }
}

/// The anchor under the screen position (x, y) on `page` as `layouter` last
/// laid it out; 0 if none or outside the rectangle.
pub fn anchor_at(page: &Page, layouter: &dyn Layouter, x: u16, y: u16) -> u16 {
    let r = page.region();
    if !r.visible() || x < r.x || y < r.y || x >= r.x + r.w || y >= r.y + r.h {
        return 0;
    }
    let scroll = page.scroll_for(layouter.height());
    layouter.anchor_at((x - r.x) as u32, (y - r.y) as u32 + scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: impl AsRef<[u8]>) -> Vec<u8> {
        let s = s.as_ref();
        let mut v = vec![op::TEXT];
        v.extend_from_slice(&(s.len() as u16).to_le_bytes());
        v.extend_from_slice(s);
        v
    }

    #[test]
    fn region_is_clamped_to_the_screen() {
        let mut p = Page::default();
        p.set_region(600, 400, 100, 100);
        assert_eq!(p.region(), Region { x: 600, y: 400, w: 40, h: 80 });
        p.set_region(9999, 0, 5, 9999);
        assert_eq!(p.region(), Region { x: 640, y: 0, w: 0, h: 480 });
        assert!(!p.region().visible());
    }

    #[test]
    fn submit_rules() {
        let mut p = Page::default();
        assert_eq!(p.submit(&[]), RESULT_REJECTED);
        assert_eq!(p.submit(&vec![0; MAX_DOCUMENT + 1]), RESULT_REJECTED);
        assert_eq!(p.document(), None);
        p.set_scroll(50);
        assert_eq!(p.submit(&vec![0; MAX_DOCUMENT]), RESULT_DONE);
        assert_eq!(p.scroll(), 0);
        let g = p.generation();
        p.submit(&vec![0; MAX_DOCUMENT]);
        assert_eq!(p.generation(), g + 1, "the same bytes again are a new document");
        p.clear();
        assert_eq!(p.document(), None);
    }

    #[test]
    fn scroll_is_clamped_when_presented() {
        let mut p = Page::default();
        p.set_region(0, 0, 100, 100);
        p.set_scroll(-5i32 as u32);
        assert_eq!(p.scroll_for(500), 0);
        p.set_scroll(450);
        assert_eq!(p.scroll_for(500), 400);
        assert_eq!(p.scroll_for(50), 0);
    }

    #[test]
    fn parse_states_and_ends() {
        let mut doc = text(b"a\tb\x01c");
        doc.extend_from_slice(&[op::FONT, 7, 3, op::SIZE, 200, op::COLOUR, 1, 2, 3, op::ANCHOR, 5, 0]);
        doc.extend(text("d"));
        doc.extend_from_slice(&[op::PARAGRAPH, 1, 10, 20, 1]);
        doc.extend(text("\u{1}"));
        doc.extend_from_slice(&[op::RULE]);
        doc.extend(text(b"e\xff"));
        doc.extend_from_slice(&[op::TEXT, 9, 0, b'x']); // cut off
        let ps = parse(&doc);
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0].runs[0].text, "a bc");
        assert_eq!(
            ps[0].runs[1].style,
            Style { face: 0, bold: true, italic: true, size: 96, rgb: [1, 2, 3], anchor: 5 }
        );
        assert_eq!((ps[1].align, ps[1].indent, ps[1].space, ps[1].nowrap, ps[1].has_text()), (Align::Centre, 10, 20, true, false));
        assert_eq!(ps[2].rule, Some([1, 2, 3]));
        // Text after a rule: a paragraph like the latest Paragraph command.
        assert_eq!((ps[3].align, ps[3].space), (Align::Centre, 20));
        assert_eq!(ps[3].runs[0].text, "e\u{FFFD}");
        // Unknown opcode ends it.
        assert_eq!(parse(&[op::SIZE, 20, 0x99, op::RULE]).len(), 1);
        assert_eq!(parse(&[op::SIZE]).len(), 1);
    }

    #[test]
    fn reference_wraps_greedily() {
        // Size 10: 5 units per character; width 50: 10 characters a line.
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("aaa bbb ccc dddddddddddddddddddddd e"));
        let l = reference_layout(&doc, 50);
        let lines: Vec<String> = l.lines.iter().map(|l| l.spans.iter().map(|s| s.text.clone()).collect()).collect();
        assert_eq!(lines, ["aaa bbb", "ccc", "dddddddddd", "dddddddddd", "dd e"]);
        assert_eq!(l.lines[1].y, 13);
        assert_eq!(l.height, 5 * 13);
        // Wide characters take a whole size.
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("日本語x"));
        let l = reference_layout(&doc, 50);
        assert_eq!(l.lines[0].spans[0].x1, 2 * 35);
    }

    #[test]
    fn reference_paragraphs_rules_alignment() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("ab\n\nc"));
        doc.extend_from_slice(&[op::PARAGRAPH, 2, 4, 6, 0]);
        doc.extend(text("xy"));
        doc.extend_from_slice(&[op::RULE, op::PARAGRAPH, 1, 0, 0, 0]);
        doc.extend(text("z"));
        let l = reference_layout(&doc, 50);
        let ys: Vec<u32> = l.lines.iter().map(|l| l.y).collect();
        assert_eq!(ys, [0, 13, 26, 45, 58, 66]);
        assert_eq!(l.height, 79);
        // End aligned with indent 4: 100 half units, indent 8: 8 + 92 - 10.
        assert_eq!(l.lines[3].spans[0].x0, 8 + 92 - 10 * 2);
        assert_eq!(l.lines[4].rule, Some(DEFAULT_COLOUR));
        assert_eq!(l.lines[5].spans[0].x0, (100 - 10) / 2);
    }

    #[test]
    fn watch_reports_new_documents_and_heights() {
        let mut p = Page::default();
        let mut w = PageWatch::default();
        assert_eq!(w.wanted(&p), None);
        p.set_region(0, 0, 100, 100);
        assert_eq!(w.wanted(&p), None);
        p.submit(&text("hello"));
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 18), Some(Event::page_laid_out(18)));
        assert_eq!(w.wanted(&p), None);
        // A width change with the same height: laid out, not reported.
        p.set_region(0, 0, 90, 100);
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 18), None);
        p.set_region(0, 0, 10, 100);
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 36), Some(Event::page_laid_out(36)));
        // Hidden by width 0: waits.
        p.set_region(0, 0, 0, 100);
        p.submit(&text("x"));
        assert_eq!(w.wanted(&p), None);
        p.clear();
        let k = w.wanted(&p).unwrap();
        assert_eq!(w.finished(k, 0), Some(Event::page_laid_out(0)));
        assert_eq!(w.finished((99, 1), 1 << 30), Some(Event::page_laid_out(MAX_REPORTED_HEIGHT)));
    }

    #[test]
    fn anchors_are_hit() {
        let mut doc = vec![op::SIZE, 10];
        doc.extend(text("ab "));
        doc.extend_from_slice(&[op::ANCHOR, 0x34, 0x12]);
        doc.extend(text("link"));
        let mut p = Page::default();
        p.set_region(100, 100, 200, 100);
        p.submit(&doc);
        let mut input = PageInput::default();
        input.after_frame(Some(&p));
        let ev = input.events(Some(&p), &[Event::pointer(100 + 16, 105, 1), Event::pointer(100 + 16, 105, 3), Event::pointer(100, 105, 1)]);
        assert_eq!(ev[0], Event::page_laid_out(13));
        assert_eq!(ev[2], Event::page_anchor(0x1234, 1));
        assert_eq!(ev[4], Event::page_anchor(0x1234, 3));
        assert_eq!(ev.len(), 6);
    }
}
