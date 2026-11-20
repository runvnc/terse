# Terse articles — v0 (draft)

**Terse in three layers.** (a) The Terse network, `NETWORK.md`: blobs,
relays, feeds, entries, profiles and names, search. (b) Terse articles,
this document: the article format and what a reader must show for it.
(a) and (b) are **the core**: a native reader that implements them, with
no virtual machine, is a complete, conforming Terse client. (c) The Terse
machine, `SPEC.md` (with its device documents, `spec/`) and `STATE.md`: an optional layer that reuses the core.
Nothing in this document depends on (c); the notes for machine hosts at
the end (section 11) can be skipped by anyone writing a reader.

Status: the first, deliberately small format. Everything here may change
until v1.

A corpus of articles, one per construct with its near misses, and what a
reader must show for each as a plain text dump, is in `vectors/articles/`
(the dump format is in `vectors/README`).

An **article** is a blob (`NETWORK.md` section 1) of UTF-8 text that
reads well as it is, published in a feed as an entry of kind 1
(`NETWORK.md` section 4.2). This document says how a **reader** (any
program that shows articles to a person) must show one: which constructs
the text holds and what each must look like, stated as requirements on
what the person sees, not as a layout algorithm. Any text toolkit that
can wrap proportional text, show a monospaced face and draw filled and
stroked curves can conform.

"Must", "should" and "may" are requirements, recommendations and
permissions on a reader. Bytes are written as characters where they are
ASCII. A **column** is the width at which a reader lays out an article's
text.

## 1. Text

- An article is UTF-8. A reader shows each malformed sequence as the
  replacement character U+FFFD (one or more per sequence) and never
  refuses an article because of one.
- A byte order mark (the bytes `EF BB BF`) at the very start of an
  article is ignored: it is not shown, and the article's first line
  starts after it (so its markers count). Anywhere else U+FEFF is
  ordinary text.
- **Lines** end at LF (`0Ah`); a CR before the LF is ignored by these
  rules. **Blanks** are spaces, tabs and CRs. A line of only blanks is a
  **blank line**. The markers of section 2 count only at the very start
  of a line.
- A tab in shown text is a blank: a reader shows it as at least one
  space. So is a **lone CR**, one that is not before an LF (inside a
  line, or at the end of an article): a reader shows it as a space. A CR
  before an LF is never shown. Other control characters (U+0000–U+001F
  other than tab, CR and LF, and U+007F–U+009F) are not shown.
- Text that matches no rule of this document is **ordinary text**, shown
  as written. This is the fallback for everything: a construct that is
  malformed, or that a reader does not support where this document allows
  that, is shown as the text it is made of (sections 2.5, 4, 6.5).
- Authors should keep articles to a few kilobytes. A reader may show only
  the start of a longer article (the reference reader shows 48 KiB); it
  then says that the rest is not shown.

A blob whose first four bytes are `TERS` (a program image) or `TDOC` (a
document) is not an article (`NETWORK.md` section 1.1): a reader does not
show it as text.

## 2. Blocks

The text is a sequence of blocks. Blank lines separate them, and a block
also ends before any line that it does not hold (by the table below),
which then starts the next block.

| Block | Starts with a line that | Holds |
|---|---|---|
| Heading | starts with `#` and a space | that one line, after the marker |
| Subheading | starts with `##` and a space | that one line, after the marker |
| List item | starts with `-` or `*` and a space | the line after the marker, and the ordinary lines after it |
| Preformatted | starts with four spaces or a tab (an indented line) | that line and the indented lines after it |
| Fence | starts with three backquotes (`` ``` ``) | the lines after it up to a closing line |
| Table | starts with `\|` | that line and the lines after it that start with `\|` |
| Paragraph | is any other non-blank line | that line and the ordinary lines after it |

An **ordinary line** is a non-blank line that starts none of the other
blocks. After a heading, a preformatted block, a fence or a table it
starts a paragraph; after a list item it continues the item. So a line
such as `### x`, `1. x`, `-x` or `  # x` (blanks before the marker) is
ordinary text.

Every block is shown apart from the ones around it (a reader chooses the
spacing), in the order of the text, one below another.

### 2.1 Paragraphs and list items

A paragraph's or list item's lines are joined into one: each line break
becomes a space and the blanks at the start of each line are dropped,
also on its first line. A reader:

- must wrap the joined text to its column, breaking lines between words
  (at spaces, or where its line-breaking rules for the script allow); it
  may break a word that is wider than the whole column anywhere;
- may show a run of blanks inside a line as written or as one space;
- should lay out right-to-left text by the Unicode bidirectional
  algorithm, and should shape scripts that need it, as far as its text
  toolkit can.

A **list item** is shown as an item of a list: set apart from paragraphs
(for example indented) with a mark before its first line (for example a
bullet); `-` and `*` make the same item. Consecutive items form one list,
and a reader should show them closer together than paragraphs. Lists have
one level, and there are no numbered lists.

### 2.2 Headings

A heading and a subheading each hold one line; the blanks after the
marker are dropped. A reader shows a heading's text more prominently than
a subheading's, and a subheading's more prominently than body text (for
example larger and bold), and wraps it like a paragraph if it is wider
than the column. A heading with no text is an empty line.

### 2.3 Preformatted blocks

A preformatted block's lines are shown as written, leading blanks
included, each on a line of its own, in a monospaced face, and not
wrapped. A line wider than the column may be cut off at the column's
edge, or the block may scroll sideways; it must not be wrapped.

### 2.4 Fences

A fence ends at a **closing line**: three backquotes followed only by
blanks. Its lines are shown as written, blank lines included, like a
preformatted block; the opening and closing lines are not shown. A fence
with no closing line runs to the end of the article; one with no lines
shows nothing, except a figure (a well-formed `path` opening line), whose
path string is then empty: an empty box W × H (section 6.1). What follows the backquotes on the opening line (its info
string) is ignored, except for `path` (section 6). Inside a fence a line
that starts with three backquotes but is not a closing line is one of its
lines.

### 2.5 Tables

A table is shown as a grid of cells: one row per line (but the alignment
row), the cells of a column one above another, the columns side by side
in order (the first at the start). Its lines are its rows.

- **Cells.** A row is cut into cells: the `|` that starts the line is
  dropped, then the blanks at the end of the line, then a `|` at its end
  if there is one that is not preceded by a backslash; what is left is
  cut at every `|` that is not preceded by a backslash, and the blanks at
  the start and end of each piece are dropped. Each piece is a cell (so a
  row has at least one). In a cell, `\|` stands for `|`, also in a code
  span; any other backslash is itself.
- **Columns.** The first row's number of cells, at most 8, is the table's
  number of columns: the cells of any row beyond that number are dropped
  (the first row's too), and a row with fewer has empty cells at its end.
- **Alignment row.** If the table's second line is made only of `|`, `-`,
  `:` and blanks and holds at least one `-`, it is an **alignment row**:
  it is not shown, the first row is the table's **header**, and its
  cells give the columns' alignment in order: a cell of two or more bytes
  that starts and ends with `:` centres its column, one that ends with
  `:` and does not start with one (or is only `:`) aligns it to the end,
  any other to the start, which is also the alignment of a column it
  gives none for and of every column of a table without an alignment row.
- **Showing it.** A cell is one line of ordinary text in which links and
  code spans count (sections 4, 5). A reader shows each cell's text
  within its column, aligned as the column says (start and end are left
  and right in left-to-right text; a reader may take them as left and
  right always), and shows the header, if any, apart from the other rows
  (for example bold, with a rule under it). Empty cells are empty.
- **Too wide.** A table whose cells' text does not fit the column on one
  line each may have its columns narrowed and the text of its cells
  wrapped within them. A reader that has narrowed the columns as far as
  it reasonably can (the reference reader stops at 48 units, section 6.2,
  per column) may show the table wider than the column, cut off at the
  column's edge or scrolling sideways.
- A reader may stop showing a table's rows after a limit of its own (the
  reference reader shows at most 1,024 cells of a table); it should say
  that rows are left out.
- A reader that does not lay tables out shows a table's lines as a
  paragraph's (joined into one wrapped line of text, bars included),
  which reads well enough.

## 3. Inline text

### 3.1 Where links and code spans count

Links (section 4) count in every block that shows text: paragraphs, list
items, headings, table cells, preformatted blocks and fences (also a
figure shown as its fence). Code spans (section 5) count in paragraphs,
list items, headings and table cells, not in preformatted blocks or
fences.

### 3.2 Order

Links and code spans are found from the start of each line: where two
would overlap, the one that starts first counts (a link with text starts
at its `[`), and the bytes it covers are not looked at again. Code spans
do not nest, and there is no other inline markup: `*`, `_`, `<` and
the like are ordinary text.

## 4. Links

A link is shown so that the person can tell it from the text around it
(for example in another colour) and can follow it (section 4.4). The only
links are the `terse:` forms below, bare or as the target of a link with
text. Nothing else is a link: in particular `http://`, `https://`,
`mailto:` and other URLs are ordinary text, both bare and as the target
in `[text](target)` (which is then not a link with text: the brackets,
the text and the target are all shown as written).

A letter or digit in these rules is an ASCII letter (`A`–`Z`, `a`–`z`)
or digit (`0`–`9`); hexadecimal digits in link forms are lowercase.

### 4.1 Bare links

| Form | Target |
|---|---|
| `terse:` and 64 hexadecimal digits | An article, by blob ID (`NETWORK.md` section 1) |
| `terse:feed/` and 64 hexadecimal digits, optionally `@` and hints | A feed, by key (`NETWORK.md` section 4.1), with the relay hints of its address (`NETWORK.md` section 4.9) |
| `terse:program/`, 64 hexadecimal digits, `/`, a name | A program, by publisher's feed key (all zero for a program built into a machine host) and name |

- A link is not preceded by a letter or digit. In the first two forms the
  64 digits are not followed by a letter or digit (else the text is not a
  link).
- **Hints.** In a feed link, an `@` right after the 64 digits may start
  the hint list of the feed's address (`NETWORK.md` section 4.9). The
  candidate is the longest run of bytes after the `@` that are letters,
  digits, `.`, `-`, `:`, `,`, `[` or `]` (a `]` only while a `[` of the
  run is not yet closed by one), less every `.`, `,` and `:` at its end
  (all of them, as for a program's name). If the candidate is a valid hint
  list (one or two hints, `NETWORK.md` section 4.9), the link goes on to
  its end; otherwise (an empty candidate, three hints, a bad port, a
  label ending in `-`, ...) the link is the key alone and the `@` and
  what follows are text. So a sentence's punctuation after an address
  is not part of it (`…@relay.example.` and `…@relay.example, and` end
  the link at `example`), a `)` or a blank always ends it, and a `]`
  ends it unless it closes an IPv6 address.
- A reader that does not know hints (one written to an earlier revision
  of this document) sees `terse:feed/` and the key, followed by `@`, which
  is not a letter or digit: the same feed link, then the hints as text.
  Readers of both kinds show the same feed.
- In a program link the name is the run of bytes `A`–`Z`, `a`–`z`,
  `0`–`9`, `-`, `.`, `_`, `~` and `%` followed by two hexadecimal digits
  (either case) that follows the `/`, less every `.` at its end (all of
  them, if there are several: `Pad...` names `Pad`); each `%XX`
  stands for the byte XX, every other byte for itself, and the bytes so
  decoded must be a display name (`NETWORK.md` section 4.6), else the
  text is not a link. So a name's other bytes (a space, non-ASCII
  letters, a final dot) are written as `%XX`.
- A reader shows a bare link as written, or shortened so that it is
  still recognisable (the reference reader shows `terse:` and the first
  and last six digits, `terse:360793…dec189`, a program link's name
  decoded after them, and a feed link's hints as written,
  `terse:feed/360793…dec189@relay.example`).

### 4.2 Links with text

A **link with text** is `[`, the text, `]`, `(`, the target, `)`, all on
one line: the text is 1 to 200 bytes holding no `[`, `]`, backquote or
link (counted as written: in a table cell `\|` is two bytes, though it is
shown as `|`), and the target is a bare link by section 4.1 that is the whole of
what is between the parentheses (so it holds no blank and no `)`; with a
feed link's hints, `[Bob](terse:feed/<key>@relay.example)` is one, and
`[Bob](terse:feed/<key>@relay.example.)` is not, since the final `.` is
not part of the bare link). A
reader shows the text as the link and does not show the target (it may
show the target when the link is selected or pointed at). Brackets that
are not all of a link with text are ordinary text, and the links in them
count as bare links.

### 4.3 Examples

| Text | Is |
|---|---|
| `terse:` + 64 digits | a link to an article |
| `see terse:` + 64 digits + `.` | a link; the final `.` is text |
| `terse:feed/` + 64 digits + `@relay.example.` | a feed link with one hint; the final `.` is text |
| `terse:feed/` + 64 digits + `@a.example,b.example:9000` | a feed link with two hints |
| `terse:feed/` + 64 digits + `@a.example,b.example,c.example` | a feed link (the key alone), then text `@a.example,b.example,c.example` |
| `[Bob](terse:feed/` + 64 digits + `@relay.example)` | a feed link with a hint, shown as "Bob" |
| `xterse:` + 64 digits | text (preceded by a letter) |
| `[Notes](terse:` + 64 digits + `)` | a link shown as "Notes" |
| `[Notes]( terse:` + 64 digits + `)` | text `[Notes]( `, a bare link, text `)` |
| `http://example.org` | text |
| `[Example](https://example.org)` | text, shown as written |

### 4.4 Following a link

A reader lets the person follow a link (for example by clicking it, or
by moving to it with a key and confirming). It must not follow one by
itself; it may fetch an article link's blob ahead of time (prefetch,
`NETWORK.md` section 4.4).

- **Article link.** The reader gets the blob by its ID and verifies it
  (`NETWORK.md` section 2), then shows it as an article. If the blob is
  a program image or a document (section 1), the reader does not show it
  as text: it says that it cannot open it, or hands it to a program
  that can (on a machine host, the shell, section 11). The title shown
  is that of an entry the reader holds for the blob, if any; else the
  reader may use the article's first heading, or a short form of the ID.
  A reader may refuse to open a linked article larger than it shows
  (section 1), saying so.
- **Feed link.** The reader shows the feed: its name with its key
  (`NETWORK.md` section 4.6) and what it holds of it, and may offer to
  subscribe. If the link has hints, a reader that subscribes from it
  subscribes with them (`NETWORK.md` section 4.9) and shows them first,
  since subscribing may make it connect to the relays they name; it
  never connects to them only because the link is shown or followed.
- **Program link.** A reader without the machine says that it cannot run
  programs (it may show the publisher's key and the name). On a machine
  host the reader asks the shell to start it (section 11).

## 5. Code spans

A **code span** is a backquote, 1 to 200 bytes none of which is a
backquote or a line break (counted as written: in a table cell `\|` is
two bytes, though it is shown as `|`), and a backquote, in a paragraph, a list item,
a heading or a table cell. A reader shows the bytes between the
backquotes as they are, in a monospaced face, and does not show the
backquotes; a link in a code span does not count. A backquote that
starts no code span is ordinary text (so is one in a preformatted block
or a fence). In a table cell `\|` in a code span stands for `|`
(section 2.5).

## 6. Figures

A fence whose info string is the word `path` (after optional blanks, and
followed by a blank or the end of the line) is a **figure**. Its opening
line goes on with the width W, blanks and the height H, then only
blanks: each 1 to 4 decimal digits with no sign (leading zeros allowed:
`0100` is 100) for a value from 1 to 1024, the size of the drawing in
units (section 6.2). Its lines hold a **path string**. The figure is
drawn if the fence has a closing line and the opening line and the path
string are well formed by the rules below; otherwise it is shown as a
fence (section 2.4), which is why a figure reads as text too.

### 6.1 Path string

The path string is SVG path data with two additions.

- **Commands** `M` (move), `L` (line), `H` and `V` (horizontal and
  vertical line), `C` and `S` (cubic curve, and smooth cubic), `Q` and
  `T` (quadratic curve, and smooth quadratic), `Z` (close the subpath),
  each followed by its numbers, as in SVG: 2, 2, 1, 1, 6, 4, 4, 2 and 0.
  A lowercase command takes its numbers relative to the current point.
  More numbers after a command's last one repeat the command (after `M`
  or `m`, as `L` or `l`). `S` and `T` reflect the last control point of
  an immediately preceding `C` or `S` (for `S`) or `Q` or `T` (for `T`)
  about the current point (a word between them does not matter), else
  use the current point. Arcs (`A`) are not supported in v0.
- The first command is `M` or `m` (a first `m` is relative to (0, 0)). A
  path string may hold no command at all (only words, or nothing): the
  figure then draws nothing and is an empty box W × H.
- **Words.** `fill <colour>` and `stroke <colour> [<width>]` may stand
  wherever a command may. They set how the subpaths after them are drawn
  (section 6.3). A `stroke` without a width sets the width to 1 (it does
  not keep an earlier width). A colour is `#rgb`, `#rrggbb`, `#rrggbbaa`
  (hexadecimal, either case; `aa` is opacity, `ff` opaque; the shorter
  forms are opaque), `none` (nothing drawn), or `fg` (section 6.3). The
  width is a number from 0 to 2047 units; 0 draws no outline. A word
  ends the path so far (section 6.3): the numbers after a word belong to
  it or to a new command, never repeat the command before it.
- **Numbers**: an optional sign (`+` or `-`), then digits with an
  optional decimal point (`5`, `-0.5`, `.5`, `5.`), at most 32 characters
  with the sign; no exponents. Numbers and commands are separated by any
  number of spaces, tabs, line breaks (LF and CR) and commas, or by
  nothing where that is unambiguous: a number may directly follow a
  command letter or a number (`M10-5`, `L.5.5` is 0.5 and 0.5), and a
  command letter may directly follow a number (`L5 5L6 6`, `L5 5Z`). A
  word is at the start of the path string or after a separator, never
  directly after a number or a command letter (`L5 5fill #f00` is
  malformed; `L5 5 fill #f00` is not). A word and its colour are each
  followed by a separator or the end, so a colour is always after a
  separator too.
- Every number is rounded to the nearest sixteenth of a unit (halves
  away from zero) before it is used, so relative coordinates add up
  exactly. Every point a command reaches, control points included, lies
  within ±2047 units (2047 included), checked before rounding: for an
  absolute command each coordinate as written, for a relative one the
  current point plus the number as written (so `L2047.01 0` is out of
  range although it rounds to 2047, and `L2046.99 0` is within it). A
  reflected control point of `S` or `T` is checked as computed from the
  rounded points it reflects. A stroke width is checked the same way, as
  written (`2047.001` and `-0.001` are out of range, `-0` is 0).
- Anything else (another letter, a number where none is expected, a
  command without all its numbers, a colour or width that is malformed
  or out of range, a word with no colour) makes the path string
  malformed.

### 6.2 Coordinates and size

Coordinates are in **units** from the figure's top left corner, x to
the right and y down. Units are relative to the reading column: a column
of the **recommended width**, about 68 characters of body text a line
(what the reference reader shows, and so what authors see), holds 512
units. The figure is a box W units wide and H high, a block of its own;
the reader places it in the column as it likes (the reference reader
centres it). A unit is the same length across and down, and the same
for every figure of an article.

- At a figure's **natural size**, a unit is a 512th of a column of the
  recommended width in the reader's body text. A reader whose column
  has the recommended width shows figures at their natural size: a unit
  is its column's width over 512.
- A reader whose column is narrower scales every figure by its column's
  width over the recommended width (a unit is its column's width over
  512), uniformly: coordinates, stroke widths, W and H alike, so that a
  figure takes the same share of the column as in the reference reader.
- A reader never shows a figure larger than its natural size: a column
  wider than recommended does not make figures larger. Only a reader
  that scales its text up (zoomed in, or with a larger body text) scales
  figures up, by the same factor (which makes their natural size larger
  in the same proportion).
- A figure wider than the column may be scaled down further to fit,
  uniformly (widths included, height in proportion); a reader may
  instead show it at full size, cut off at the column's edge or
  scrolling sideways. It must not be stretched in one direction only.

### 6.3 Drawing

- Each segment starts at the current point and ends at its end point,
  which becomes the current point; at the start the current point is
  (0, 0). A segment with no subpath open (at the start, after a `Z` or a
  word) starts a new subpath at the current point; `M` starts one at its
  point. `Z` draws a straight segment back to the subpath's start, closes
  the subpath and makes its start the current point. A `Z` with no
  subpath open (at the start, after another `Z` or after a word) does
  nothing: it draws nothing and does not move the current point.
- A **path** is the subpaths between two words (or the start or end of
  the path string). Each path is drawn with the fill and stroke in force
  when it starts: first filled, with the non-zero winding rule (every
  subpath counted as closed by a straight line back to its start), then
  stroked: an outline of the stroke width centred on its segments, with
  round joins and round ends; a subpath that is closed (`Z`) is stroked
  all the way round. A path with no segment draws nothing.
- At the start of the path string the fill is `none` and the stroke is
  `fg`, 1 unit wide. A colour `none` (or any colour with opacity 0)
  draws nothing; a stroke width of 0 draws no outline.
- `fg` is the colour of the reader's body text, opaque. Every other
  colour is as written: a reader does not change it, whatever its own
  background. A figure has no background of its own: where nothing is
  drawn, the column shows through.
- Paths are drawn in order, later ones over earlier ones, blended by
  opacity (source over). Drawing is clipped to the figure's box.
- A reader may smooth (antialias) edges as it likes. It may set a limit
  on the work of drawing one figure; when a figure goes beyond it, the
  reader draws its paths in order up to a path it stops at, and none of
  that path or after it (the reference reader has such limits,
  section 11).

### 6.4 Example

    ```path 200 60
    fill #2b3a55 stroke none M0 0 H200 V60 H0 Z
    fill #98c379 stroke fg 2 M20 50 L60 10 L100 50 Z
    fill none stroke #e5c07b 3 M120 50 Q150 0 180 50
    ```

A dark panel 200 × 60, a green triangle outlined 2 units wide in the
text colour, and a yellow curve 3 units wide, not filled.

### 6.5 Fallback

A figure that is malformed or has no closing line is shown as its
fence's lines. A reader that does not draw figures shows every figure
that way.

## 7. What a reader must do, in short

- Read the text as UTF-8 (section 1) and split it into blocks
  (section 2).
- Show paragraphs, list items, headings, preformatted blocks, fences and
  code spans as sections 2 and 5 say.
- Show tables as grids (section 2.5), or as paragraphs.
- Draw figures (section 6), or show them as fences.
- Recognise exactly the links of section 4, show them as links and let
  the person follow them; show everything else as ordinary text.

## 8. What a reader need not do

- Use any particular font, size, colour, spacing or margin: the
  reference reader's (section 11) are one choice.
- Lay out tables or draw figures: their fallbacks (sections 2.5, 6.5)
  conform.
- Lay out text exactly as another reader does: line breaks, the height
  of an article and where a link ends up on the screen differ between
  readers, and nothing in the format depends on them.
- Shape complex scripts or reorder bidirectional text beyond what its
  toolkit does.
- Shorten bare links, offer keys to move between links, prefetch linked
  articles, or show the targets of links with text.
- Use a feed link's hints (it still recognises them as part of the link,
  section 4.1).
- Open program links or documents (section 4.4).
- Show more of a long article, more rows of a long table, or more of a
  costly figure than its own limits allow (sections 1, 2.5, 6.3), as
  long as it says what it left out of an article or table.
- Recognise any markup not in this document: such text is shown as
  written.

## 9. Titles

An article's title is the title of its entry (`NETWORK.md` section 4.2),
not part of its text. A reader that shows an article it holds no entry
for (one reached by a link) may use the article's first line as the
title if it is a heading, and then need not show that heading again
(the reference reader does so).

## 10. Not yet specified

- Inline markup beyond code spans (emphasis, strong).
- Images that are not drawn from paths, and arcs in figure paths.
- Numbered and nested lists.

## 11. For machine hosts

This section concerns only hosts that run the Terse machine (`SPEC.md`).
A reader implementer can skip it.

- The built-in reader, `programs/reader`, is a machine program that
  shows articles by this document through the page device (`spec/page.md` §page); it is the **reference reader** that the sections above
  cite. Its choices: a 512-unit column, body text 15 units (sans),
  headings 21 and subheadings 17 (bold), preformatted text, fences and
  code spans in monospace, list items indented 10 units behind a bullet,
  figures centred at one screen unit per unit (natural size: the column
  has the recommended width, section 6.2) and scaled down to the column
  if wider, tables laid out by the page
  device (`spec/page.md` §page.layout: columns narrowed to no less than 48
  units, then cut off at the column's edge), at most 48 KiB of an
  article and 1,024 cells of a table, and the page device's figure caps
  (`spec/page.md` §page.document).
- One unit of section 6.2 is one screen unit of the machine (`SPEC.md` §screen), and the reader's `fg` is its body text colour.
- Following a link to a program image or a document, or a program link,
  the reader asks the shell to open it (`SPEC.md` §system, `open`);
  without a shell it says that this needs the shell.
