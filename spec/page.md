# §page Device 7 — Page (version 0)

Part of the Terse machine specification, v0 (draft): `SPEC.md` is the core
machine and says how these documents fit together; this one is complete
for its device, given the core sections it builds on (below).

Shows typeset prose: proportional fonts, shaped, bidirectional and broken
into lines at the width of a rectangle on the screen, with figures drawn
from paths between the paragraphs. The program hands the device a
**document**, a short binary list of styled text and figures much like a
display list, and a rectangle; the host lays the text out with its own fonts
and draws the part that is scrolled into view. The device parses no markup
language and does no layout for the program.

Layout is the host's business and differs between hosts, because fonts
differ. The only facts about it a program learns are the laid-out height of
the document and, when it asks, where an anchor is, and it learns them
through input events (§page.events), so a recorded run replays exactly
whatever the fonts.

**Builds on** `SPEC.md`: screen units (§screen), the call convention
(§calls.convention), events (§events), charges (§charges), the
terminal's UTF-8 decoding and wide characters (§terminal.put,
§terminal.cells), and the 2D vector display's layers, style and fill mode
(§vector.layers, §vector.phosphor, §vector.list). The reading profile
requires this device (§profiles).

## §page.rectangle Rectangle and scrolling

The page is drawn inside its **rectangle** (x, y, width, height in screen
units, §screen) and nothing of it outside. The document is laid out as one
column as wide as the rectangle; the **scroll value** is the distance in
screen units from the top of the document to the top of the rectangle. A
rectangle with a width or height of 0 hides the page.

## §page.calls Calls

| `a7` | Call | Arguments | Result | Class |
|---|---|---|---|---|
| `700h` | `submit` | `a0` = document address, `a1` = length | 0 done, 1 rejected | deferred |
| `701h` | `clear` | — | 0 | deferred |
| `702h` | `region` | `a0` = x, `a1` = y, `a2` = width, `a3` = height | 0 | deferred |
| `703h` | `scroll` | `a0` = y | 0 | deferred |
| `704h` | `background` | `a0` = colour | 0 | deferred |
| `705h` | `locate` | `a0` = anchor id | 0 done, 1 rejected | answered |

- **`submit`** copies `a1` bytes from address `a0` into the device,
  replacing the document, sets the scroll value to 0, and returns 0. If the
  length is 0 or more than 65,536 it returns 1 (rejected) and changes
  nothing. Otherwise the bytes must lie wholly in ROM or wholly in RAM; if
  they do not, the call returns `FFFF_FFFEh` and changes nothing (§calls.convention). A submit of the same bytes again is still a new document
  (§page.events).
- **`clear`** removes the document: the page shows only its background.
  The scroll value is kept.
- **`region`** sets the rectangle to x = `a0`, y = `a1`, width = `a2`,
  height = `a3`, clamped so it lies on the screen: x to at most 640, y to at
  most 480, the width to at most 640 − x and the height to at most 480 − y.
- **`scroll`** sets the scroll value to `a0`, stored as given (32 bits).
  When it presents the page, the host takes the value as a signed number and
  clamps it to 0 … max(0, height − rectangle height), where height is the
  document's laid-out height; the stored value does not change.
- **`background`** sets the colour behind the text in the rectangle:
  `a0` = r | g << 8 | b << 16 | a << 24, where `a` is opacity (255 opaque,
  0 transparent, so the cell backgrounds show through).
- **`locate`** asks where anchor `a0` is laid out in the current document;
  the host answers later with the two page located events (§page.events).
  It returns 0, or 1 (rejected, and nothing is asked) if `a0` is 0 or above
  65,535, if there is no document, or if the program has already made 8
  locates in this frame.

After reset there is no document, the rectangle is (0, 0, 0, 0) (hidden),
the scroll value is 0 and the background is `FF00_0000h`, opaque black.

## §page.document Document

A document is 1 to 65,536 bytes: a sequence of commands, each an opcode byte
followed by its operands, little-endian. It ends at an End command, at the
end of the bytes, at an unknown opcode, at a command whose operands are
cut off or at a Table command whose column count is 0 or above 8; the
commands before that are used.

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `00h` | End | — | Ends the document |
| `01h` | Text | n (2 bytes), then n bytes of UTF-8 | Adds the text to the current paragraph in the current font, size, colour and anchor |
| `02h` | Font | face (1 byte: 0 sans, 1 serif, 2 monospace), flags (1 byte: bit 0 bold, bit 1 italic) | Sets the font for later text; an unknown face is 0; other flag bits are ignored |
| `03h` | Size | size (1 byte) | Sets the font size in screen units, clamped to 6–96 |
| `04h` | Colour | r, g, b | Sets the text colour |
| `05h` | Paragraph | align (0 start, 1 centre, 2 end; anything else is 0), indent (1 byte), space (1 byte), flags (1 byte) | Starts a new paragraph with these properties |
| `06h` | Rule | — | Starts a new paragraph that is a horizontal line across the rectangle, in the current colour |
| `07h` | Anchor | id (2 bytes) | Later text and figures belong to anchor `id`; 0 is no anchor |
| `08h` | Figure | width (2 bytes), height (2 bytes), align (1 byte), n (2 bytes), then n bytes of path commands | A figure: a block of its own, width × height screen units |
| `09h` | Table | columns (1 byte, 1–8), flags (1 byte: bit 0 header), then one align byte per column (0 start, 1 centre, 2 end; anything else is 0) | Starts a table: a block of its own, a grid of cells |
| `0Ah` | Cell | — | Starts the table's next cell |
| `0Bh` | End table | — | Ends the table |

State at the start of a document: sans, no flags, size 14, colour
(229, 229, 229), anchor 0, and an implicit first paragraph that is
start-aligned with indent 0, space 0 and wrapping. Font, size, colour and
anchor carry on across paragraphs.

- **Text.** Each Text command is decoded on its own: malformed UTF-8 becomes
  U+FFFD as in the terminal (§terminal.put; a sequence split across two Text
  commands is malformed in both), but code points above U+FFFF are kept.
  U+000A breaks the line inside the paragraph; U+0009 is a space; other
  control characters (U+0000–U+001F, U+007F–U+009F) are ignored. Consecutive
  spaces are kept.
- **Paragraph properties.** `indent` is in screen units from the start edge
  and applies to every line of the paragraph. `space` is the gap in screen
  units above the paragraph, also for the first paragraph and also for a
  paragraph with no text (which takes no other room), so an empty paragraph
  is a vertical gap. Flags bit 0 is **no wrap**: lines break only at U+000A,
  and a line wider than the rectangle is cut off at the rectangle's edge.
  Flags bit 1 is **ellipsis**, used only with no wrap: a line wider than
  the rectangle less the indent ends in an ellipsis (U+2026) where it is
  cut, so that the ellipsis is wholly shown. Other flag bits are ignored.
- **Start and end** follow each paragraph's text direction, which the host
  takes from its text (the first strong character, as in the Unicode
  bidirectional algorithm): left for left-to-right text, right for
  right-to-left text. Indent is measured from that start edge.
- **Figure.** Width and height are clamped to 1–1024; align is 0 start
  (left), 1 centre, 2 end (right), anything else 0, whatever the
  direction of the text around it. A figure ends the
  current paragraph and is a block of its own, as high as its height, with
  the space of the latest Paragraph command (or of the implicit first
  paragraph) above it; if the current paragraph has no text yet, the figure
  takes its place instead and that paragraph's space is the space above the
  figure. Text after a figure starts a new paragraph as after a rule. The
  figure is placed in the rectangle's whole width, without indent: centre
  puts half the leftover width before it, end all of it. A figure wider than
  the rectangle starts at its left edge and is cut off at its right edge,
  not scaled. Drawing is clipped to the figure's box. The colour and anchor
  in effect when the Figure command appears belong to the figure.
- **Rule.** A rule paragraph is 8 screen units high with a line about one
  unit thick across the whole width of the rectangle at its middle; it has
  no space above it (put an empty Paragraph before it for a gap). Text that
  follows a rule starts a new paragraph with the properties of the latest
  Paragraph command (or those of the implicit first one).
- **Table.** A table is a block of its own, a grid of cells laid out as
  §page.layout says. It ends the current paragraph and takes its space as
  a figure does: the space of the latest Paragraph command (or of the
  implicit first paragraph) above it, or, if the current paragraph has no
  text yet, it takes that paragraph's place and its space. Other flag bits
  are ignored. The cells come in rows of `columns` cells: each Cell
  command starts the next cell, in row order (the first Cell starts the
  first cell of the first row), and the last row's missing cells are
  empty. In a table, Text adds to the current cell in the current font,
  size, colour and anchor, and Font, Size, Colour and Anchor work as
  anywhere; text before the table's first Cell command is ignored. U+000A
  breaks the line in a cell. A table ends at an End table command, and
  also at a Paragraph, Rule, Figure or Table command, which then does what
  it does, and at the end of the document. Text after a table starts a new
  paragraph as after a rule. A table with no cells takes its space and
  nothing else. The colour in effect at the Table command is the colour of
  the table's rules. With flag bit 0 the first row is a **header**: its
  text is drawn bold, whatever the bold flag of its font, and a rule is
  drawn under it. A Cell command or End table outside a table does
  nothing.
- **Table limits.** A table holds at most 1,024 cells and a document at
  most 4,096 cells in all its tables. A Cell command that would pass
  either limit, and the text after it up to the end of its table, are
  ignored (Font, Size, Colour and Anchor commands still change the state).
- **Line height** is the host's natural line height for the font and size;
  a line's height is that of the largest text on it. A line break at the end
  of a paragraph's text gives an empty last line.
- **Anchors** mark text and figures a pointer press reports, and that
  `locate` finds (§page.events); the host draws anchored text like any
  other text, in its colour.

**Path commands.** A figure's n bytes are a sequence of path commands, each
an opcode byte followed by its operands. Coordinates are signed 16-bit
numbers in sixteenths of a screen unit from the figure's top left corner;
colours are r, g, b, a (1 byte each, `a` the opacity).

| Opcode | Command | Operands | Effect |
|---|---|---|---|
| `01h` | Fill | r, g, b, a | Ends the path; later paths are filled in this colour, not filled if `a` is 0 |
| `02h` | Stroke | r, g, b, a, width (2 bytes, unsigned, sixteenths) | Ends the path; later paths are stroked in this colour and width, not stroked if `a` or the width is 0 |
| `03h` | Move | x, y | Starts a subpath at (x, y) |
| `04h` | Line | x, y | A straight segment to (x, y) |
| `05h` | Quadratic | x1, y1, x, y | A quadratic Bézier segment to (x, y) with control point (x1, y1) |
| `06h` | Cubic | x1, y1, x2, y2, x, y | A cubic Bézier segment to (x, y) with control points (x1, y1), (x2, y2) |
| `07h` | Close | — | A straight segment back to the subpath's start, which ends the subpath; nothing if no subpath is open |

- At the start of the commands: no fill; stroke in the document's text
  colour at the Figure command, opaque, 16 (one unit) wide; current point
  (0, 0). The path commands end at the end of the n bytes, at an unknown
  opcode or at a command whose operands are cut off; the commands before
  that are used.
- Each segment starts at the current point and ends at its end point, which
  becomes the current point. A segment with no subpath open (at the start,
  after a Close, a Fill or a Stroke) starts a subpath at the current point.
  After a Close the current point is the start of the subpath it closed.
- A **path** is the subpaths between two Fill or Stroke commands (or the
  start or end of the commands). Each path is filled with the non-zero rule,
  then stroked, centred on its outline, with round joins and round ends;
  later paths are drawn over earlier ones, blended by opacity. A path with no
  segment, or with neither a fill nor a stroke, draws nothing.
- **Caps.** Paths are drawn in order and counted against three caps per
  figure; the first path that would take a total above its cap is not drawn,
  and neither is any path after it. A path that draws nothing (no segment,
  or neither a fill nor a stroke) is skipped and counts against no cap.
  (1) At most 256 paths. (2) Area: a path counts the box of all its points
  (each Move's point, each segment's start, control and end points),
  clipped to the figure's box, once if it is filled, and that box grown
  on every side by half the stroke width in sixteenths, rounded up, and
  clipped, once more if it is stroked; each box's area in square screen
  units is its width times its height in sixteenths, divided by 256 and
  rounded up. The total is at most 8 × width × height of the figure and at
  most 4 × 640 × 480 = 1,228,800. (3) Length: a path's length L is, over
  its Line, Quadratic and Cubic commands, the sum of |dx| + |dy| of each
  step from the segment's start through its control points to its end, in
  sixteenths; s is the number of those commands. A Close counts in
  neither, though it draws a segment. A path's count is 2 × L + 4 × 16 × s
  if it is filled, plus 2 × L + 4 × w × s with w its stroke width if it is
  stroked; that sum is divided by 16 and rounded up once, and the total is
  at most 65,536. The caps are counted from the path commands alone, the
  same in every style and fill mode: a figure drawn as outlines (§page.layout) still counts its fills.

## §page.layout Layout and presentation

The host lays the document out with proportional fonts it chooses for the
three faces: shaping (ligatures, joining scripts such as Arabic), the
Unicode bidirectional algorithm within each line, Unicode line breaking at
the rectangle's width less the paragraph's indent, and fallback to its
other fonts for characters the chosen font lacks. Its fonts are those
it loaded when it started (§responsiveness item 5 of `SPEC.md`: it
never looks for one while a machine runs); they cover at least the full
repertoire of `SPEC.md` §terminal.text, and a character none of them
has is drawn as a box. Since layout depends on the fonts, two hosts
with different fonts (one whose user replaced a font or added one) lay
the same document out differently; that is allowed, because what a
program learns of layout reaches it only as events (§page.events),
which are inputs recorded for replay. It draws the background over the
rectangle, then the lines and figures that intersect the rectangle after
scrolling, clipped to it. How the text looks (fonts, hinting, smoothing, how
a line that does not fit is cut) and how figures are smoothed are up to the
host and cannot be observed by the program.

**Tables.** A table is laid out in these steps, with lengths in screen
units (a host measures text with its fonts; the reference layout,
§page.reference, says how it measures):

1. **Padding.** Each cell has 6 units of padding at its left and right
   and 3 at its top and bottom.
2. **Natural widths.** A cell's natural width is the width of its widest
   line when its text is broken only at U+000A, plus 12; a cell with no
   text has 12. A column's natural width is the largest of its cells'.
3. **Column widths.** If the columns' natural widths add up to at most
   the rectangle's width W, each column is its natural width. Otherwise
   the widest columns are narrowed first, all to the same width: each
   column's width is the smaller of its natural width and a cap C, where
   C is the largest width for which these widths add up to at most W; but
   C is never less than 48. (When C is 48 the table may be wider than the
   rectangle: it is cut off at the rectangle's right edge, like a
   figure.)
4. **Rows.** Each cell's text is laid out as a paragraph (wrapped, no
   indent, aligned as its column says, start and end following the
   cell's text direction as a paragraph's do) at its column's width less 12, its
   first line 3 units below the top of its row. A row is as high as the
   tallest text of its cells plus 6 (a cell with no text counts 0). The
   table is as high as its rows.
5. **Placing.** The columns are placed side by side from the rectangle's
   left edge, the first column leftmost, whatever the direction of the
   text in the cells; the rows one below another.
6. **Rules.** Rules take no room: each is a line about one unit thick
   across the table's width, in the table's rule colour, centred on the
   bottom edge of a row. A table with a header (flag bit 0) has a rule
   under its first row, also when that is its only row. Every other row
   but the last has a faint rule under it: drawn at about a third of the
   colour's opacity, so the page's background shows through. There is no
   rule above the first row or under the last (but for a header that is
   the only row).

A line of a cell is a line for the page's events (§page.events): a press
on anchored text in a cell reports its anchor, and `locate` gives the
first line that carries the anchor, lines being taken in order of their
tops (for a table row, its cells' lines from the left to the right).

Figures are drawn as outlines when the vector display is in the phosphor
style or in fill mode 1 (§vector.list; an image without the vector display
has the clean style and fill mode 0): nothing is filled, and a path that is
filled but not stroked is stroked one unit wide in its fill colour.

Cost stays bounded: a document is at most 65,536 bytes and holds at most
4,096 table cells (§page.document); the host lays it out
outside the machine's frames (a host may take several frames, on another
thread, while the machine runs on and the previous layout or nothing is
shown); the machine never waits for a layout and is not charged for it; the
host draws only the lines and figures in view, so scrolling needs no new
layout; and the caps of §page.document bound the work of drawing each figure,
which a host may also do outside the frames and only for the figures near
the view (showing a figure some frames late).

## §page.events Events

The device's events (§events):

| Type | Name | `a` | `b` | `c` |
|---|---|---|---|---|
| `08h` | Page laid out | height bits 0–7 | bits 8–15 | bits 16–23 |
| `09h` | Page anchor | anchor id bits 0–7 | bits 8–15 | bits 3–5: buttons, as in a pointer event; other bits 0 |
| `0Dh` | Page located | anchor id bits 0–7 | bits 8–15 | line height |
| `0Eh` | Page located y | y bits 0–7 | bits 8–15 | bits 16–23 |

- **Page laid out** (`08h`): `a`, `b`, `c` are bits 0–7, 8–15 and 16–23 of
  the document's height in screen units at the rectangle's current width,
  FFFFFFh if it is larger. The host sends it at the start of a frame after
  the one in which the program submitted a document, after a clear (height
  0), and again whenever the height changes because the rectangle's width
  changed. While the rectangle's width is 0 a document is not laid out, and
  the event waits until there is a width. A host may skip intermediate
  layouts when several changes come quickly, and then reports only the
  latest, but it must eventually report the height for the latest document
  and width once they stay unchanged. At most one is sent per frame.
- **Page anchor** (`09h`): sent when a pointer button is pressed while the
  pointer is over text whose anchor is not 0, as the page was last shown:
  `a`, `b` = the anchor id, low then high byte; `c` = the buttons in bits
  3–5, the same bits as in a pointer event, and its other bits 0; also when it is pressed over a figure
  whose anchor is not 0. It follows the pointer event of that press, which
  is sent as usual. "Over text" means within the extent of the text's
  glyphs along the line and within the line's height; "over a figure",
  within its box as shown (cut off at the rectangle's edge).
- **Page located** (`0Dh`) and **page located y** (`0Eh`) answer a
  `locate`, always as this pair, one after the other: `0Dh` has `a`, `b` =
  the anchor id, low then high byte, and `c` = the height in screen units
  of the first line, or figure, of the document that carries the anchor
  (255 if it is larger); `0Eh` has `a`, `b`, `c` = bits 0–7, 8–15 and 16–23
  of y, the distance in screen units from the top of the document to the
  top of that line or figure (FFFFFFh if it is larger). If no text or
  figure carries the anchor, the height is 0 and y is FFFFFFh. Lines are
  in document order, so the first line is the topmost one. A host gives
  whole units: the top rounded down, and a height that reaches the bottom
  rounded up. The two are queued together: if the queue has room for only
  one, neither is queued.
- A locate is answered from the layout of the document current at the call,
  the same layout as the latest height event (at the rectangle's width of
  that layout): at the start of a frame after the one of the call, in the
  same frame as that document's height event (after it) if that is still to
  come, else as soon as the host has the layout. Answers come in the order
  of the calls. A locate whose document is replaced (by a submit or a
  clear, also later in the frame of the call) before it is answered is
  dropped and never answered. While the rectangle's width is 0 the answer
  waits, like the height event; a host keeps at least the 8 latest locates
  that wait and may drop older ones.

A height event describes the document and width as they were at the end of
an earlier frame. A program that submits again before reading it gets
another one later, so the last height event it has read is the height of
the last document only once no newer change is pending; a program should
use it for clamping and showing progress, never for anything that must be
exact. A located anchor, like the height, is a fact about one layout that
the host may show differently later (after a change of width). All these
events are ordinary inputs (§system): recorded with the rest, they make
replay exact although layout differs between hosts.

## §page.reference Reference layout

A host without fonts (such as a headless test runner) lays out the
document with this **reference layout**, which is a pure function of the
document and the width; it is what `terse run` uses, and what makes its
height events reproducible. It is a stand-in for testing and is not what
users see.

- Every character advances 0.5 × size; a wide character (§terminal.cells; every
  code point above U+FFFF counts as wide) 1.0 × size. There is no
  shaping, no kerning and no bidirectional reordering: all text runs left to
  right, and start is left.
- A line is ceil(1.25 × size) high for the largest size on it; an empty line
  has the size in effect at its line break.
- Wrapping is greedy at spaces (U+0020): a word goes on the line if the line
  so far, the spaces before the word and the word fit in the width less the
  indent; otherwise the line ends (the spaces are dropped) and the word
  starts the next one. A word wider than a whole line is split between
  characters, at least one character per line. Spaces at the start of a
  paragraph or after a line break are kept.
- Centre puts a line's leftover width, halved and rounded down, before it;
  end puts all of it there. Paragraph space and indent are as in §page.document; a rule is 8 units high.
- A no-wrap line with ellipsis that is wider than the width less the
  indent loses characters from its end until the rest and an ellipsis
  (U+2026, in the style of the last character dropped) fit, or none is
  left; then the ellipsis is added.
- A figure is a line as high as the figure, placed as in §page.document (the
  leftover width halved and rounded down for centre); a press within its
  box hits its anchor.
- A table is laid out as §page.layout says, with widths in half units:
  a line's width is the sum of its characters' advances, C is the largest
  whole number of half units that fits, and a cell's lines are wrapped,
  aligned as a paragraph's are (greedy wrapping at spaces). Its rules
  are lines of height 0 at the rows' bottom edges.
- A locate gives the y and height of the first line (a figure is a line)
  with a character or figure of that anchor; a table's lines are taken in
  order of y, and lines of the same y from left to right.
- The height is the sum of the spaces, line heights, rules, figures and
  tables.

## §page.determinism Determinism

The device's state is the document bytes (or no document), the rectangle,
the scroll value and the background. Nothing about layout is machine state,
nor are the locates the program made: what the layout means for the
program arrives only as the events of §page.events.

## §page.charges Charges and limits

`locate` is answered, with the base charge of 64; every other call is
deferred, with 4 (§charges). In addition:

| What | Charge |
|---|---|
| Bytes copied between memory and the device: a page `submit` whose bytes were copied (its length in range and its buffer valid) | 1 per 4 bytes, rounded up, per call |

Laying out the page is not charged: the host does it outside the
machine's frames (§page.layout).

Provisional values (§provisional):

- the figure caps (§page.document): 256 paths, the area (8 × the figure's,
  at most 4 × 640 × 480) and the length (65,536); figure sides of 1 to
  1,024 units;
- the page's tables (§page.document, §page.layout): at most 8 columns, 1,024
  cells in a table and 4,096 in a document; padding of 6 and 3 units; the
  least column cap, 48 units; font sizes 6 to 96;
- page documents of 1 to 65,536 bytes (§page.calls);
- 8 locates per frame, and at least the 8 latest waiting kept by a host
  (§page.calls, §page.events).

## §page.unspecified Not yet specified

- On the page: bitmap images, arcs and gradients in figures, links the host
  follows itself, selecting and copying
  text, and fonts beyond the three faces.
