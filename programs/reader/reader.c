/* reader.c: a feed reader for Terse (machine profile 1, RV32IMC).
 *
 * Shows the timeline of the feeds the host subscribes to (SPEC §content), newest
 * first, the subscribed feeds, a feed's page, search results, a help page
 * and articles (ARTICLE.md: text with headings, lists, preformatted
 * blocks, figures and links), all on the page device (SPEC §page) in the sans
 * face. The terminal shows only the status row (29: key hints, the reading
 * position, the outcome of a request) and, while something is being typed
 * (a feed key, a nickname, a public name, a search), the entry row (28).
 * The rows under the page are blank, in the colour of the page's
 * background (xterm 234, (28, 28, 28)), which is opaque.
 *
 * Started with an argument (a blob ID the shell gave it, SPEC §system), the
 * reader shows that blob as an article at once, as if opened from the
 * timeline (titled by its entry if the timeline has one, else by the ID's
 * short form); Esc then goes to the timeline.
 *
 * The shell: the reader never starts a program or shows one. A program
 * image or a document (a blob that begins with "TERS" or "TDOC") is passed
 * to the shell with `open` (OPEN_BLOB, the blob ID), a program link with
 * OPEN_PROGRAM (key and name), and the status row says "Asked the shell
 * to open it" (the shell may still ask the user, or fail), or "This needs
 * the shell" if `open` returned 0 (no shell, or a request of this machine
 * still waits). That happens on Enter or a click
 * on a document's entry (the timeline and search show the timeline's
 * document entries, kind 4, too), on following a link to such a blob or a
 * program link, and if an article opened from the timeline or the
 * argument turns out to be one (the view it would have been shown in is
 * left, as by Esc).
 *
 * Names: wherever a feed is shown (the timeline's details line, the feeds
 * list, an article's details line, a feed's page, search results) it is
 * its name, then its short key in brackets (the key alone if no name is
 * known). The name is the user's nickname, else the feed's declared name,
 * from the names records (op 09h, one per subscribed feed and per feed with
 * a nickname, read with the subscriptions), else the profile's name among
 * the search results. Names are not unique, so a name is never shown
 * without the key. A feed found by search shows the name it declares, and
 * the user's nickname for it beside the key.
 *
 * Every frame it handles the queued events, rebuilds and submits the page's
 * document if what it shows changed, does a bounded share of the work in
 * hand (turning article bytes into the page document), draws the terminal
 * rows that changed and waits for the next frame.
 *
 * The lists (timeline and feeds). A program never learns where the page put
 * a line, only the height of the whole document, so a list is virtualised:
 * its document holds a header slot and the entries from `top` on that fit
 * on the screen, plus one more (partly shown), and it is submitted again
 * whenever the selection or `top` changes. Every slot has the same shape:
 * one no-wrap paragraph with SLOT_SPACE above it, a line of SLOT_TITLE text,
 * a line break and a line of SLOT_META text, no trailing break. A line's
 * height depends only on face and size, so a list document of s slots is s
 * times one slot high: the height event of a list document gives the slot
 * height e = H / s exactly, and floor(rectangle height / e) - 1 entries are
 * wholly visible. Until a height arrives e is estimated (1.35 x the sizes);
 * a height that is not a whole number of slots, or gives an implausible
 * slot, belongs to another document and is ignored. Titles are cut at the
 * rectangle's edge with an ellipsis (no wrap, ellipsis: SPEC §page.document); control
 * characters and line or paragraph
 * separators in them become spaces, so a slot always has two lines. Each
 * entry is an anchor (its index + 1): a click on it opens it (timeline) or
 * selects it (feeds). The selection is shown by colour. A document's
 * title has a mark before it (U+25C6 "◆" and a space) and its details
 * line begins with its format name ("sketch document"): the first
 * TDOC_HEAD + 48 bytes of the blobs of the documents in view that are in
 * the host's store are got, one at a time while few requests are pending,
 * and the names kept for 32 blobs; "document" until the name is known
 * (or if the head is not valid).
 *
 * The article view: the page's rectangle covers rows 0-28 (y 0-464), x
 * 64-576: 512 units, about 68 characters of 15-unit body text, centred.
 * The document is the title (26 units, bold), a grey line (feed key, entry
 * number, date, size), a rule, then the article, by the article format of
 * ARTICLE.md (blocks, links, figures), shown this way:
 *   - A paragraph: sans 15 units, wrapped; its lines joined with spaces
 *     (the blanks at the start of each line dropped).
 *   - A heading: sans bold 21 units, 18 units of space above it; a
 *     subheading: sans bold 17, 14 above.
 *   - A list item: a paragraph indented 10 units that begins with a bullet
 *     and a space ("• "); 4 units above it after another item. (The page
 *     indents every line of a paragraph alike, so wrapped lines align with
 *     the bullet, not the text.)
 *   - A preformatted block or a fence: monospace 13 units, not wrapped,
 *     its lines (and blank lines, in a fence) as written.
 *   - A figure (W and H 1-4 digits each, 1-1024; a word after a separator
 *     only; points and widths in range as written, before rounding): a
 *     Figure command, centred, of W x H units, or if W is over
 *     512 (the column) 512 x round(H x 512 / W) (at least 1) with every
 *     coordinate and width scaled by 512 / W after rounding to sixteenths,
 *     halves away from zero (a width that comes to 0 is 1; the initial
 *     stroke, 1 unit, is scaled too). Its path string becomes absolute
 *     binary commands: H and V as Line, S as Cubic, T as Quadratic, the
 *     relative forms resolved, a Move before the first segment after a Z
 *     or a word (unless it is an M), colours as RGBA with fg the body text
 *     colour (218, 218, 218), opaque; the document's text colour at the
 *     figure is the body colour, so the initial stroke is fg. A figure
 *     that is malformed, has no closing line, or whose commands do not fit
 *     in the document (or in 65,535 bytes) is shown as its fence would be.
 *   - A table (lines that start with "|"): a Table command (the first
 *     row's cells, at most 8, are the columns; a second line of only
 *     "|-:" and blanks with a "-" is the alignment row: header and
 *     alignment), then a Cell command and the cell's text for each cell,
 *     row by row (missing cells empty, extra ones dropped), then End
 *     table; text 14 units, rules grey. B_FIND cuts each line into cells
 *     as it looks for its LF and keeps them (tc_s, tc_e: at most TC_MAX,
 *     the page's limit; later rows are not shown); "\|" is a "|". Links
 *     and code spans count in cells.
 *   - A code span (a backquote, 1-200 bytes with no backquote or LF, a
 *     backquote) in a paragraph, item, heading or cell: mono, a unit
 *     smaller, in the code colour.
 *   - Every other block has 10 units of space above it. The LF after a
 *     block's last line is dropped; a CR before an LF is left for the device
 *     to ignore, and a lone CR becomes a space (in place, as B_FIND looks
 *     for each line's LF), so it is a blank shown as one. A byte order
 *     mark at the start of the article is skipped.
 *   - Links (only terse: forms: a web address is ordinary text, also as
 *     the target of "[text](target)", which is then shown as written):
 *     terse:<blob ID> (an article),
 *     terse:feed/<key> (a feed; with "@" and one or two relay hints if
 *     they follow and are valid, NETWORK 4.9: the run of letters, digits,
 *     ".-:," and brackets after the "@", less ".,:" at its end, else the
 *     link is the key alone) and terse:program/<key>/<name> (a
 *     program: the name percent-encoded, the bytes other than A-Z a-z 0-9
 *     - . _ ~ as %XX, either case; the name is the run of those characters
 *     and %XX triples, less any "." at its end, and must decode to a
 *     display name or the text is no link), bare (a terse: link shown
 *     shortened to its first and last six digits, "terse:360793…dec189",
 *     a program link with its name decoded after them,
 *     "terse:program/360793…dec189/Sketch pad") or with text
 *     ("[text](target)", shown as the text; a feed link's hints are
 *     shown after the short key, "terse:feed/360793…dec189@relay.example").
 *     A link with text is found at
 *     the colon of its target, by looking back for "](" and its "["; so
 *     that a frame's share of building never ends inside one, it ends
 *     before a "[" on the current line. The first 512 links get anchors 1,
 *     2, ... and the link colour. Tab and Shift+Tab make the next or
 *     previous link the current one, in amber (its Colour command is
 *     changed in place and the document submitted again, keeping the
 *     scroll value), and the status row shows its target (a program link:
 *     program “name” by the feed's name and short key); Enter or a click
 *     follows it. The reader then asks the page where the link's line is
 *     (locate, SPEC §page.calls; the answer comes in a later frame) and scrolls
 *     just enough to show it whole, MARGIN units from the edges. After the
 *     user scrolled, Tab goes to the first link at or below the top of the
 *     view and Shift+Tab to the last above its bottom instead, found by a
 *     binary search over the links' places (one locate a frame; Tabs
 *     pressed meanwhile are counted and applied after it). A Tab search
 *     whose locate is lost (the document was replaced, or the machine was
 *     restored from a snapshot, which drops pending locates) asks again
 *     when the page is next laid out. An article link first gets the
 *     blob's first 4 bytes: a program image or a document goes to the
 *     shell and the article stays; else the link opens that article (if
 *     the 4 bytes could not be got, its page says so at once): the one
 *     shown goes on a back stack of 8 (Esc goes back through it, each to
 *     its scroll value, then to the view the first was opened from); its
 *     title is the timeline's if the host has an entry for the blob (the
 *     timeline buffer is looked at, then the whole timeline read, one
 *     request at a time, before the document is built), else the ID's
 *     short form; one larger than
 *     ART_CAP is not shown. A feed link opens the feed's page (name, keys,
 *     the relays its hints name, subscribed or not; S, which passes the
 *     hints on, U), from which Esc returns to the article.
 *     A program link asks the shell for the program.
 *   - Prefetch: the first 8 distinct article links of the article shown
 *     are got with one-byte gets as they are met while building (the host
 *     fetches and keeps the whole blob), at most 2 at a time and only
 *     while fewer than 4 requests of any kind are pending (program links
 *     never).
 * An article without an entry (or whose entry has the same title) whose
 * first line is a heading is titled by it, and the heading is not shown
 * again. M on the timeline opens the manual's index (manual_id.h, from
 * docs/manual/build), which every host has in its store.
 * The document holds at most 65,536 bytes (48 KiB of article plus
 * formatting); if the article is longer, or the formatting does not fit,
 * it ends where the bytes end, between characters, with a note saying how
 * much is shown. While the article is fetched (or if it cannot be), a
 * short document with its title and a message stands in.
 *
 * Building the document costs about 9 units per byte of prose (more for
 * short lines and paragraphs, links and figures' path strings), so it is
 * spread over frames: each frame
 * spends at most BUILD_UNITS on it, by an estimate of the cost that is
 * never below the real one. The document is submitted at the end of the
 * first frame of building, partial if need be (so the first screenful
 * appears at once), and again, whole, when it is complete; the 48 KiB
 * fixture article takes 13 frames. The program keeps the scroll value
 * itself: Up/Down move one body line (LINE units), PgUp/PgDn/Space the
 * rectangle less a line, the wheel three lines a step, Home/End the ends.
 * It clamps the value to 0 .. height - 464 with the latest height event
 * (08h) and calls `scroll` whenever it changes, also after each submit
 * (which resets it). Until the first height arrives, moves are only kept
 * from going below 0, End is remembered (it also follows the height of the
 * whole document when it comes) and the status row shows no position. The
 * position is 0% at the top and 100% at the end, and not shown when the
 * document fits. The help page scrolls the same way.
 *
 * Feeds: the subscriptions (op 03h: key and highest seq) with their
 * names, how many timeline entries each has (counted by reading the whole
 * timeline, one request a frame) and a "you" mark on the host's own feed
 * (op 08h), and a mark on feeds the host follows through a relay their
 * address's hints name (names flag 08h). S opens the entry row for a feed
 * key or address (typed or pasted: "terse:feed/" optional, the key, then
 * optionally "@" and hints; whitespace is ignored; Enter subscribes, op
 * 05h with the hints), U unsubscribes from
 * the selected feed (op 06h), Enter opens its page. N types a nickname
 * for the selected feed (op 0Ch; empty removes it), P the user's public
 * name, then asks on the status row whether search may find it (Y/N; op
 * 0Bh); both are checked against the display name rules first. T (in
 * every view but the article) publishes a test article: put (02h),
 * publish (07h), identity again (08h) and, if the own feed is not
 * subscribed, subscribe to it, so that the article shows in the timeline.
 * The help page shows the own feed address: the key, then the hints of
 * op 08h (with a buffer), or says that without public relays only people
 * on the same relay find the feed.
 * The host asks the user about each subscribe, unsubscribe, publish,
 * profile and nickname; the status row says what came of it.
 *
 * Search (/ from the timeline or feeds): Enter searches the articles on
 * this host itself (below) and sends two finds at once (op 0Ah: feeds by
 * name, articles by title), and shows a list like the others: a heading
 * slot for each section (with its count or progress, or why it is empty):
 * "In your articles" (title, then smaller the feed and date; an excerpt
 * around the first match in the text, the match in the match colour, or
 * where it matched), "Feeds" (name; short key, "you", "subscribed") and
 * "Articles on your relays" (title; feed, date, size). Headings are not
 * selectable; Enter or a click opens a feed's page or an article (Esc
 * comes back), or passes a document to the shell. Results of this host
 * come in while the list is shown: rows below them move down, and the
 * selection and the top with them, so what is selected stays.
 *
 * In your articles: the search reads the timeline newest first (its own
 * requests, LS_PAGE bytes at a time) and tests each entry's title, then,
 * for an article in the host's store (flag bit 0), reads its first 48 KiB
 * with ranged gets of up to 16 KiB, several at once (while the reader has
 * fewer than LS_PENDING requests pending), into a 64 KiB ring (ls_arena),
 * scans each chunk as it comes, in order, and drops it; the last LS_PAD
 * bytes of a chunk go before the next, so that a match across the two is
 * found. Articles not in the store are searched by title only (never
 * fetched) and counted as skipped; documents by title only. Matching is
 * the relays' (NETWORK 4.7): a run of the query's bytes, A-Z and a-z the
 * same, other bytes exact. An entry's search ends at its text's first
 * match, which gives the excerpt: at most LS_BEFORE bytes before it and
 * LS_EX in all, from the raw text, not cut inside a character, line
 * breaks and other control characters as spaces, runs of spaces as one.
 * At most LS_MAX results; one more stops the search ("more than 64
 * matches"). Changing the search text, opening a result and leaving the
 * view stop it at once (its requests still pending are taken in as they
 * come; a new search starts once they are). The scan (Horspool over
 * folded bytes; for a one-byte query, find2 over both cases) is metered
 * like building an article: LS_UNITS of estimated work a frame
 * (LS_UNITS_BUSY when the list was built in it), less the content
 * delivered to it; a stretch is scanned only if its worst case fits, and
 * what it cost is counted from its windows and comparisons. The progress
 * line under the heading is redrawn at most every LS_EVERY frames unless a
 * result appears on the screen. Opening a result whose text matched opens
 * the article with an anchor (MATCH_ID) on the match, in the match colour
 * (on the first text after it if the match is not shown as text, such as
 * in a link's target or a figure's path); once the document holding it is
 * submitted the reader locates it and scrolls it a quarter of the way down
 * the view (if it is not in view at the top, and the user has not moved
 * yet); if the match is not in the document (beyond what is shown) the
 * status row says the article opens at the top.
 *
 * Memory: everything is a static array in RAM (flat addresses): the 16 KiB
 * timeline buffer, a 16 KiB buffer for counting, a 16 KiB buffer for
 * looking up a linked article's entry, 16 KiB of subscription records,
 * 16 KiB of names records, two 4 KiB search buffers, up to 48 KiB of the
 * article (ART_CAP), the 64 KiB article document, an 8 KiB document for
 * the other views, and for searching this host's articles about 100 KiB:
 * the 64 KiB ring of chunks, an 8 KiB timeline page, 16 entries being
 * searched (232-byte records) and 64 results (record and excerpt). The timeline buffer
 * holds one request's worth of records: 70 entries with 120-byte titles to
 * 147 with empty ones. A figure's path commands are built in place, in the
 * article document where its Figure command will hold them. */

#include <stdint.h>
#include "terse.h"
#include "manual_id.h"

#define COLS TERM_COLS
#define ENTRY_ROW 28
#define STATUS_ROW 29

/* xterm palette indexes (SPEC §terminal.reset). */
#define C_TEXT 252
#define C_DIM 244
#define C_BG 234 /* (28, 28, 28): the page's background */
#define C_BAR_FG 231
#define C_BAR_BG 24
#define C_BAR_DIM 109
#define C_ENTRY_BG 237
#define C_OK 120
#define C_ERROR 210
#define C_INFO 229

/* Timeline records (SPEC §content.feeds): L, flags, then the entry (NETWORK 4.2). */
#define R_FLAGS 1
#define R_FEED 2
#define R_SEQ 34
#define R_TIME 38
#define R_KIND 42
#define R_SIZE 43
#define R_BLOB 46
#define R_REPLY 78
#define R_N 110
#define R_TITLE 111

#define TL_BUF_LEN 16384
#define MAX_RECORDS 148 /* TL_BUF_LEN / 111, the smallest record */
#define MAX_SUBS 455    /* 16,384 / 36 */

#define ART_CAP 49152u
#define CHUNK 16384u

/* The page. Lists: a wide column; the article, help and the empty
 * timeline: a centred column 512 units wide. Both cover rows 0-28. */
#define PAGE_H 464
#define LIST_X 32
#define LIST_W 576
#define RECT_X 64
#define RECT_W 512
#define RECT_H PAGE_H
#define LINE 20 /* one line of body text: what Up and Down move */
#define TITLE_SIZE 26
#define META_SIZE 12
#define BODY_SIZE 15
#define PRE_SIZE 13
#define PRE_INDENT 0
#define PARA_SPACE 10
#define H1_SIZE 21
#define H1_SPACE 18
#define H2_SIZE 17
#define H2_SPACE 14
#define ITEM_INDENT 10
#define ITEM_SPACE 4
/* A figure's coordinates: at most 2047 units either way, in sixteenths. */
#define F_MAX (2047 * 16)
#define NOTE_SIZE 13
#define DOC_CAP 65536u
#define DOC_RESERVE 128 /* kept for the closing note and End */
#define LDOC_CAP 8192u
#define MAX_LINKS 512
/* List slots: sizes, the space above each and the estimate of its height
 * (1.35 x each size, rounded up) until the first height event; heights
 * that give a slot outside E_MIN .. E_MAX are not a list's. */
#define SLOT_TITLE 16
#define SLOT_META 12
#define SLOT_SPACE 8
#define E_GUESS (SLOT_SPACE + 22 + 17)
#define E_MIN (SLOT_SPACE + SLOT_TITLE + SLOT_META)
#define E_MAX (SLOT_SPACE + 3 * (SLOT_TITLE + SLOT_META))
#define MAX_VIS 24
/* Building the document is paid for from BUILD_UNITS estimated budget
 * units a frame, so that a frame that also draws every row and submits the
 * whole document ends well within its budget. The estimates (measured, and
 * rounded up): per byte looked at while finding blocks and while emitting
 * them (scanned a word at a time, and copied), per line, per LF or colon
 * met while emitting, per paragraph, per link and per "terse:" looked at
 * (its 64 digits checked); for a figure's path
 * string, per separator byte, per byte of a number, per token, per segment
 * converted and per figure. */
#define BUILD_UNITS 80000
/* Per byte of text looked through for characters beyond the full repertoire. */
#define W_FULL 3
#define W_FIND 3
#define W_EMIT 7
#define W_LINE 200
#define W_HIT 120
#define W_BLOCK 400
#define W_LINK 900
#define W_TERSE 1200
#define W_PSEP 14
#define W_PBYTE 20
#define W_PTOKEN 100
#define W_PSEG 360
#define W_FIGURE 300
#define W_CODE 700   /* a code span: its Font, Size, Colour and back */
#define W_TSCAN 28   /* a byte of a table's line, cut into cells */
#define W_TROW 500   /* the end of a table's row */
#define W_CELL 450   /* a cell: its Cell command */
#define W_TABLE 400  /* a table's Table and End table commands */
#define TC_MAX 1024  /* a table's cells kept (the page device's limit) */
#define TABLE_SIZE 14
#define CODE_DOWN 1  /* a code span is this much smaller than its text */

/* Keys (USB HID). */
#define KEY_F 0x09
#define KEY_N 0x11
#define KEY_P 0x13
#define KEY_S 0x16
#define KEY_T 0x17
#define KEY_U 0x18
#define KEY_Y 0x1C
#define KEY_F1 0x3A
#define KEY_M 0x10

enum { TIMELINE, FEEDS, HELP, ARTICLE, SEARCH, FEED };
enum { FETCHING, READY, NOT_FOUND, FAILED, TOO_LARGE };
/* Kinds of link (ARTICLE.md). */
enum { LK_ARTICLE, LK_FEED, LK_PROGRAM };
/* A program link: "terse:program/", the key, "/" and the name. */
#define PROG_KEY 14
#define PROG_NAME 79

static uint8_t view;
static uint32_t now; /* Unix time of this frame; 0 if the host has none */
static terse_event ev;

static uint8_t dirty[TERM_ROWS];
/* The row being drawn: its cells, the column of the next character and the
 * colours it is drawn in. */
static uint32_t cells[COLS];
static uint8_t row_at, col;
static uint32_t attr; /* the colours, as in a cell: fg << 16 | bg << 24 */

/* The status message (the outcome of a request), shown instead of the key
 * hints until the next key; msg_n 0: none. */
static char msg[120];
static uint8_t msg_n, msg_fg;

/* --- Timeline ---------------------------------------------------------- */

static content_req tl_req, sub_req, cnt_req, id_req, op_req;
static uint8_t tl_handle, sub_handle, cnt_handle, id_handle, op_handle;
static uint8_t tl_buf[TL_BUF_LEN];
static uint16_t rec_at[MAX_RECORDS];
static uint16_t tl_base, tl_count, tl_total; /* window: tl_base .. +tl_count */
static uint16_t tl_req_base;
static uint8_t tl_loaded, tl_refresh, tl_again;
static uint8_t sel_id[36]; /* feed key and seq of the selection */
static uint8_t sel_id_ok;

/* --- Feeds ------------------------------------------------------------- */

static uint8_t sub_buf[MAX_SUBS * 36];
static uint16_t subs_n, subs_total;
static uint8_t subs_loaded, sub_again;
/* Entries per feed (same index as sub_buf), counted from cnt_pos on. */
static uint16_t counts[MAX_SUBS];
static uint8_t cnt_buf[TL_BUF_LEN];
static uint32_t cnt_pos;
static uint8_t cnt_done, cnt_again;
static uint8_t own_key[32], own_known;

/* The request that changes something (one at a time). */
enum { OP_IDLE, OP_PUT, OP_PUBLISH, OP_IDENT, OP_SELF_SUB, OP_SUB, OP_UNSUB, OP_NICK, OP_PROFILE };
static uint8_t op_state;
static uint8_t op_key[32];
static char test_title[24];
static uint8_t test_tlen;
static uint32_t test_seq, tests;
/* A name being set (nickname or public name) and, for the public name,
 * whether it is to be listed. */
static uint8_t op_name[48], op_nlen, op_listed;
/* The Y/N question about listing the public name is shown. */
static uint8_t asking;

/* Names (op 09h) of the subscribed feeds and of those with a nickname:
 * the records and where each starts, in key order. */
#define NAMES_LEN 16384
static content_req nm_req;
static uint8_t nm_handle, nm_again;
static uint8_t nm_buf[NAMES_LEN];
#define NM_MAX 482 /* 16,384 / 34, the shortest record */
static uint16_t nm_at[NM_MAX], nm_n;

/* The text being typed on row 28: a feed key (the non-blank characters,
 * ASCII, anything else as 0x80; entry_n counts them), or a nickname, a
 * public name or a search (UTF-8; entry_n counts the bytes, also those
 * past ENTRY_CAP). The character of the key that opened the entry is
 * not typed if its text arrives by the next frame (entry_eat). */
#define ENTRY_CAP 240 /* a feed link with two hints is 237 */
enum { E_OFF, E_KEY, E_NICK, E_NAME, E_SEARCH };
static uint8_t entry, entry_buf[ENTRY_CAP];
static uint16_t entry_n;
static uint8_t entry_eat;
static uint32_t entry_eat_until;
static uint8_t nick_key[32]; /* the feed a nickname is typed for */

/* Search (op 0Ah): the query, the two requests (feeds, articles), their
 * results (timeline records) and where each starts, and their status (0
 * while pending). The list's rows: the feeds' heading, the feeds, the
 * articles' heading, the articles. */
#define FIND_LEN 4096
#define FIND_MAX 16
static content_req sf_req, sa_req;
static uint8_t sf_handle, sa_handle, sf_st, sa_st, s_ran;
static uint8_t sf_buf[FIND_LEN], sa_buf[FIND_LEN];
static uint16_t sf_at[FIND_MAX], sa_at[FIND_MAX];
static uint8_t sf_n, sa_n;
static uint8_t s_query[48], s_qlen;
static uint8_t s_back; /* the view search goes back to */

/* Searching the articles on this host (see "In your articles" at the top).
 * The query, folded (A-Z as a-z), and its Horspool tables: ls_skip[c] is
 * how far the window moves when its last byte is c (0: c may be the last
 * byte of a match: the window is compared), ls_shift how far it moves
 * after a comparison; ls_fold maps a byte to its folded form. */
#define LS_MAX 64       /* results kept */
#define LS_EX 120       /* the bytes of the text an excerpt is taken from, at most */
#define LS_BEFORE 24    /* of them before the match, at most */
#define LS_PAD 64       /* room before a chunk for the end of the one before */
#define LS_ARENA 65536u /* the chunks being read and scanned */
#define LS_SPANS 16     /* chunks asked for and not yet scanned */
#define LS_JOBS 16      /* timeline entries taken and not yet searched */
#define LS_PENDING 6    /* the reader's requests pending, at most, after a search request */
#define LS_PAGE 8192    /* the timeline is read this much at a time */
#define LS_REC 232      /* a timeline record: L, flags and at most 229 bytes */
#define LS_NONE 0xFFFFFFFFu
#define MATCH_ID 1000   /* the anchor at the match in an article opened from the results */
static uint8_t ls_nd[48], ls_m, ls_shift, ls_skip[256], ls_fold[256];
/* A result: its timeline record, where the first match in the text is
 * (LS_NONE: none), an excerpt around it (the match at ex_at, ex_len bytes;
 * ex_pre, ex_post: text left out before and after it), and where the
 * query is in the title (t_len 0: not there). */
typedef struct {
    uint8_t rec[LS_REC];
    uint32_t at;
    uint8_t ex[LS_EX];
    uint8_t ex_n, ex_at, ex_len, ex_pre, ex_post, t_at, t_len;
} ls_result;
static ls_result ls_res[LS_MAX];
static uint8_t ls_n;
/* An entry being searched: its record, the bytes of its text to search
 * (0: only the title), those asked for and those scanned, its result (1 +
 * its index, 0 none) and its stage. */
enum { J_TITLE, J_BODY, J_DONE };
typedef struct {
    uint8_t rec[LS_REC];
    uint32_t want, issued, scanned;
    uint8_t res, stage;
} ls_job;
static ls_job ls_jobs[LS_JOBS];
static uint8_t ls_j0, ls_jn, ls_ji; /* the oldest, how many, the next to ask for (from ls_j0) */
/* A chunk asked for: its request, where it is in ls_arena (LS_PAD bytes of
 * room, then the chunk), its offset in the article, the bytes asked, the
 * job (its slot in ls_jobs) and whether it is in. Chunks are scanned in
 * the order they were asked for, which is the jobs' order, so the arena
 * is a ring: a chunk's room is free once it is scanned. */
typedef struct {
    content_req req;
    uint32_t pos, off, len;
    uint8_t handle, job, in;
} ls_span;
static ls_span ls_sp[LS_SPANS];
static uint8_t ls_s0, ls_sn, ls_inflight;
static uint8_t ls_arena[LS_ARENA] __attribute__((aligned(4)));
static uint32_t ls_head;
/* The end of the chunk scanned last (for a match across the boundary and
 * an excerpt's start), and the head chunk's scan: started, the end of the
 * next window (in ls_arena). */
static uint8_t ls_carry[LS_PAD], ls_carry_n, ls_started;
static uint32_t ls_p;
/* The timeline, read LS_PAGE bytes at a time: the records of the page and
 * the next to take, the index of the next page, the timeline's size (and
 * at the start). */
static content_req ls_tl_req;
static uint8_t ls_tl_handle, ls_tl_buf[LS_PAGE];
static uint16_t ls_tl_at[LS_PAGE / 111 + 1], ls_tl_n, ls_tl_i;
static uint32_t ls_tl_pos, ls_tl_total, ls_total, ls_tl_known;
/* The state: idle, running, all searched, stopped (by typing, opening a
 * result or leaving the view), or stopped at LS_MAX results. ls_wait: a
 * new search waits until the requests of the last one are in. */
enum { LS_IDLE, LS_RUN, LS_DONE, LS_STOPPED, LS_FULL };
static uint8_t ls_state, ls_wait;
static uint32_t ls_searched, ls_skipped;
/* Measured: bytes scanned, bytes delivered to the search this frame, the
 * Horspool windows and comparisons of a scan; whether the list document
 * shows the latest progress, and the frame it was built in. */
static uint32_t ls_bytes, ls_delivered, ls_its, ls_cands;
static uint8_t ls_stale;
static uint32_t ls_doc_frame;
/* An article opened from a result: where its match is (LS_NONE: none) and
 * how long, and how far its anchor is: MT_WANT (not in the document
 * yet), MT_PLACED (in it, not yet located), MT_LOCATING, MT_DONE. */
enum { MT_NONE, MT_WANT, MT_PLACED, MT_LOCATING, MT_DONE };
static uint32_t mt_at;
static uint8_t mt_len, mt_state;
static const uint8_t rgb_match[3] = {100, 220, 220};

/* The feed page: its key and the view it goes back to. */
static uint8_t fp_key[32], fp_back;
/* The hints of the feed page's address (from a link), and the own
 * address's (op 08h). */
static uint8_t fp_hints[CONTENT_MAX_HINTS], fp_hlen;
static uint8_t own_hints[CONTENT_MAX_HINTS], own_hlen;
/* The hints given with the subscribe in hand. */
static uint8_t op_hints[CONTENT_MAX_HINTS], op_hlen;

/* --- Lists ------------------------------------------------------------- */

typedef struct {
    uint16_t sel, top;
} listpos;
static listpos tlp, fdp, srp;
static uint16_t slot_e = E_GUESS;
static uint16_t rect_h = PAGE_H;
static uint8_t last_slots; /* slots of the list document shown, 0 if not a list */
static uint8_t page_dirty; /* the view's document must be built and submitted again */
static uint8_t region_x;   /* the rectangle set: 0 none, else its x */
static uint16_t region_h;
static uint8_t ldoc[LDOC_CAP];

/* --- Article ----------------------------------------------------------- */

static content_req art_req;
static uint8_t art_handle, art_state;
static uint8_t art_title[121], art_tlen, art_feed[32], art_reply, art_local;
static uint32_t art_seq, art_time, art_size;
static uint8_t art_blob[32];
static uint16_t art_have, art_want;
static uint8_t art[ART_CAP];
/* art_entry: the article's entry is known (else only its blob ID, from a
 * link); art_link: it was opened by a link (a larger one than ART_CAP is
 * not shown); art_return: the view Esc goes back to after the last
 * article of the back stack. */
static uint8_t art_entry, art_link, art_return;
static int32_t art_restore; /* the scroll value to go back to, or -1 */

/* Looking for the entry of an article opened by a link: the timeline is
 * read from lk_pos on, one request at a time. */
static content_req lk_req;
static uint8_t lk_handle, lk_restart;
static uint32_t lk_pos;
static uint8_t lk_buf[TL_BUF_LEN];

/* The back stack: the articles a link was followed from, oldest first. */
#define BACK_N 8
static uint8_t bk_blob[BACK_N][32], bk_link[BACK_N], bk_n;
static int32_t bk_scroll[BACK_N];

/* Prefetch: the first PF_N distinct article links of the article shown,
 * got one byte at a time (the host fetches and keeps the whole blob), at
 * most PF_SLOTS at once and only while few other requests are pending. */
#define PF_N 8
#define PF_SLOTS 2
#define PF_PENDING 4
static uint8_t pf_ids[PF_N][32], pf_n, pf_next;
static content_req pf_req[PF_SLOTS];
static uint8_t pf_handle[PF_SLOTS], pf_byte[PF_SLOTS];

/* Following an article link: the first bytes of the linked blob are got
 * first (pr_id; pr_from: the article it was followed from), so that a
 * program image or a document is passed to the shell instead of shown. */
static content_req pr_req;
static uint8_t pr_handle, pr_buf[4], pr_id[32], pr_from[32];

/* The request for the shell (terse_open): a blob, or a program. */
static uint8_t open_req[1 + 32 + 1 + 48];

/* The format names of documents in the lists, from the head of their
 * blobs (TDOC_HEAD + 48 bytes), one get at a time, by blob ID: dn_state 0
 * free, 1 asked, 2 known (dn_len 0: no valid name). dn_scan: a list was
 * shown since the last look for documents in it. */
#define DN_N 32
static content_req dn_req;
static uint8_t dn_handle, dn_slot, dn_next, dn_scan, dn_buf[TDOC_HEAD + 48];
static uint8_t dn_id[DN_N][32], dn_state[DN_N], dn_len[DN_N], dn_name[DN_N][48];

/* The document and how far it is built (offsets into art). */
static uint8_t doc[DOC_CAP];
enum { B_START, B_FIND, B_EMIT, B_FINISH, B_DONE };
/* Kinds of line (B_FIND) and of block. */
enum { L_PLAIN, L_INDENT, L_ITEM, L_H1, L_H2, L_FENCE, L_TABLE };
/* (K_TABLE before K_PRE: its text is not preformatted.) */
enum { K_NONE, K_PARA, K_ITEM, K_H1, K_H2, K_TABLE, K_PRE, K_FENCE, K_FIGURE };
/* The text styles of blocks: face, flags, size, colour. */
enum { S_BODY, S_PRE, S_H1, S_H2, S_TABLE };
static uint8_t b_phase;
static uint16_t b_line;    /* B_FIND: start of the next line to look at */
static uint16_t b_scan;    /* B_FIND: where the search for its LF goes on */
static uint8_t b_in_line;  /* B_FIND: 0 at a line start, 1 classifying it, 2 looking for its LF */
static uint8_t b_lkind;    /* B_FIND: the kind of the line */
static uint16_t b_lines;   /* lines in the block so far (a fence: lines between its fences) */
static uint16_t b_start, b_end; /* the block's text: its first byte, the end of its last line */
static uint16_t b_open;    /* the block's first line (a fence: its opening line) */
static uint8_t b_kind, b_prev; /* the block's kind, the previous block's */
static uint8_t b_closed;   /* a fence: its closing line was found */
static uint8_t b_bad;      /* a figure: its opening line is malformed */
static uint8_t b_style;    /* the style the document is in */
static uint8_t b_skip;     /* B_EMIT: blanks at b_emit are dropped */
static uint16_t b_emit;    /* B_EMIT: the next byte to look at */
static uint16_t b_text;    /* B_EMIT: the first byte not yet in the document */
static uint16_t b_ls;      /* B_EMIT: the start of the line being emitted */
static uint16_t b_shown;   /* the article bytes in the document */
static uint32_t b_work;    /* units left for this frame */
static const uint8_t *b_colour; /* the colour of the block's text */
static uint16_t link_at[MAX_LINKS], link_len[MAX_LINKS], links;
/* Each link's kind and where its Colour command is in the document. */
static uint8_t link_kind[MAX_LINKS];
static uint16_t link_doc[MAX_LINKS];
/* The article builder's place in its document, kept while other
 * documents are built in ldoc. */
static uint8_t *a_ptr, *a_limit, a_overflow;

/* A table (K_TABLE): found in B_FIND, line by line, as cells (their
 * first byte and the end of their last non-blank byte, in art), row
 * after row of t_cols cells (empty cells start and end at 0); its
 * header flag and columns' alignment. While a line is cut: where the
 * current piece starts (0: not yet) and ends, whether only blanks
 * followed the last bar (t_bar), whether the byte before was a
 * backslash, whether the line can be an alignment row (t_al: only
 * |-: and blanks; t_dash: a - seen) and the pieces so far (at most 8).
 * B_EMIT: the next cell (t_at) and whether it is open. */
static uint16_t tc_s[TC_MAX], tc_e[TC_MAX], t_n, t_at;
static uint8_t t_cols, t_rows, t_head, t_align[8], t_in, t_full;
static uint16_t t_ps, t_pe, rp_s[8], rp_e[8];
static uint8_t t_bar, t_esc, t_al, t_dash, rp_n, t_pieces;
/* The title taken from the article's first line, a heading (hd_len 0:
 * none), at art[hd_at]. */
static uint16_t hd_at;
static uint8_t hd_len;

/* A figure being converted (B_EMIT of a K_FIGURE block): its size, the
 * width it is scaled down from (0: not scaled), where its path commands
 * go (in the document, after room for a Paragraph and the Figure head),
 * the next byte of its path string, the command whose numbers are being
 * read and how many are, a word whose operands are awaited, the current
 * point, the subpath's start and the last control point (sixteenths, not
 * scaled), and what the last segment was. */
enum { FW_NONE, FW_FILL, FW_STROKE, FW_WIDTH };
static uint16_t f_w, f_h, f_scale, f_pos;
static uint8_t *f_base;
static uint8_t f_cmd, f_argc, f_n, f_word, f_moved, f_need, f_last;
/* f_none: the command letter read last has had no numbers yet (a letter
 * that takes numbers but is given none is malformed). */
static uint8_t f_none;
static int32_t f_arg[6], f_cx, f_cy, f_sx, f_sy, f_qx, f_qy;
static int8_t f_ex[6]; /* the sign of what rounding took off each of f_arg */
static uint8_t f_rgba[4];

/* The scrolled views (article, help). */
static uint8_t doc_state; /* 0: nothing (of this view's) submitted; 1: a part; 2: all of it */
static uint32_t page_h;   /* the latest height (event 08h), if page_h_known */
static uint8_t page_h_known;
static int32_t scroll, scroll_sent;
static uint8_t to_end;    /* End was pressed: stay at the end as the height grows */
static uint16_t link_shown; /* the anchor whose URL the status row shows, 0 none */
static uint16_t cur_link;   /* the current link (Tab), 0 none */
/* Locating links (page_locate): the anchor whose place is awaited (0
 * none), whether it is a step of the search for the first link in view
 * (between sr_lo and sr_hi, at or below y sr_at), and whether the user
 * scrolled since the current link was chosen. */
static uint16_t loc_id, sr_lo, sr_hi;
static uint8_t loc_search, tab_scrolled;
/* The page was laid out while loc_id was awaited: unless its answer comes
 * in the same frame, it was lost (the document was replaced, or the
 * machine restored from a snapshot) and is asked for again. */
static uint8_t loc_again;
static int8_t sr_dir, tab_more; /* tab_more: Tab presses during a search */
static int32_t sr_at;
static page_place place;
#define MARGIN 8 /* kept between a located link and the rectangle's edge */

static const uint8_t rgb_title[3] = {240, 240, 240};
static const uint8_t rgb_meta[3] = {140, 140, 140};
static const uint8_t rgb_reply[3] = {95, 175, 175};
static const uint8_t rgb_rule[3] = {70, 70, 70};
static const uint8_t rgb_body[3] = {218, 218, 218};
static const uint8_t rgb_pre[3] = {190, 205, 165};
static const uint8_t rgb_link[3] = {110, 170, 255};
static const uint8_t rgb_head[3] = {255, 255, 255};
static const uint8_t rgb_entry[3] = {225, 225, 225};
static const uint8_t rgb_sel[3] = {255, 200, 95};
static const uint8_t rgb_sel_meta[3] = {205, 170, 110};
static const uint8_t rgb_you[3] = {120, 210, 140};
static const uint8_t rgb_key[3] = {200, 215, 235};
static const uint8_t rgb_error[3] = {255, 135, 120};
static const uint8_t rgb_cur_link[3] = {255, 200, 95};
static const uint8_t rgb_code[3] = {190, 205, 165};
static const uint8_t rgb_table_rule[3] = {150, 150, 150};

/* --- Text -------------------------------------------------------------- */

/* Counts the printable ASCII bytes (20h-7Eh) at `p`, at most `max`. */
static uint32_t ascii_run(uint32_t max, const uint8_t *p)
{
    uint32_t n = 0;
    while (n < max && (uint8_t)(p[n] - 0x20) < 0x5F)
        n++;
    return n;
}

/* A title (valid UTF-8: the host verified the entry). */
static uint16_t title_char(const uint8_t **pp)
{
    const uint8_t *p = *pp;
    uint8_t b = *p++;
    uint16_t cp;
    if (b < 0x80) {
        cp = b;
    } else if (b < 0xE0) {
        cp = (uint16_t)(b & 0x1F) << 6 | (*p++ & 0x3F);
    } else if (b < 0xF0) {
        cp = (uint16_t)(b & 0x0F) << 12 | (uint16_t)(p[0] & 0x3F) << 6 | (p[1] & 0x3F);
        p += 2;
    } else {
        cp = 0xFFFD;
        p += 3;
    }
    *pp = p;
    return cp;
}

/* Control characters would move the cursor: show them as U+FFFD. */
static uint16_t printable(uint16_t cp)
{
    return cp < 0x20 || cp == 0x7F ? 0xFFFD : cp;
}

static uint8_t width(uint32_t cp)
{
    if (cp < 0x1100)
        return 1;
    /* The common cases first (as in the profile-1 library): CJK, kana,
     * Hangul, fullwidth forms. */
    if ((cp >= 0x3250 && cp <= 0xA48C) || (cp >= 0x3041 && cp <= 0x30FF && cp != 0x3097 && cp != 0x3098)
        || (cp >= 0xAC00 && cp <= 0xD7A3) || (cp >= 0xFF01 && cp <= 0xFF60))
        return 2;
    return term_is_wide(cp) ? 2 : 1;
}

/* Text being put together: a line of the page, a message, the test
 * article. */
static char tb[800];
static uint16_t tb_n;

static void tb_s(const char *s)
{
    while (*s && tb_n < sizeof tb)
        tb[tb_n++] = *s++;
}

static char num[11];

static const char *u32_str(uint32_t v)
{
    char *p = num + sizeof num - 1;
    *p = 0;
    do {
        *--p = '0' + v % 10;
        v /= 10;
    } while (v);
    return p;
}

static void tb_u(uint32_t v)
{
    tb_s(u32_str(v));
}

static void tb_two(uint32_t v)
{
    char s[3] = {'0' + v / 10, '0' + v % 10, 0};
    tb_s(s);
}

static void tb_hex(const uint8_t *b, uint8_t n)
{
    static const char hex[] = "0123456789abcdef";
    char s[3] = {0, 0, 0};
    uint8_t k;
    for (k = 0; k < n; k++) {
        s[0] = hex[b[k] >> 4];
        s[1] = hex[b[k] & 15];
        tb_s(s);
    }
}

/* A feed key's short form: its first and last 6 hex digits. */
static void tb_key(const uint8_t *k)
{
    tb_hex(k, 3);
    tb_s("\xE2\x80\xA6");
    tb_hex(k + 29, 3);
}

static void tb_bytes(const uint8_t *p, uint32_t n)
{
    while (n-- && tb_n < sizeof tb)
        tb[tb_n++] = *p++;
}

/* A name on one line: line and paragraph separators (U+2028, U+2029;
 * names hold no control characters) become spaces. */
static void tb_name(const uint8_t *p, uint8_t n)
{
    uint8_t i;
    for (i = 0; i < n && tb_n < sizeof tb; i++) {
        if (p[i] == 0xE2 && i + 2 < n && p[i + 1] == 0x80 && (p[i + 2] == 0xA8 || p[i + 2] == 0xA9)) {
            tb[tb_n++] = ' ';
            i += 2;
        } else {
            tb[tb_n++] = p[i];
        }
    }
}

/* The name to show for a feed: its nickname or declared name (names of
 * the subscribed feeds, op 09h), else the name of its profile among the
 * search results; its length, 0 if none is known. *nick is set if it is
 * the user's nickname. */
static uint8_t name_flags; /* the names record's flags of the last name_of, 0 if none */
static const uint8_t *name_of(const uint8_t *key, uint8_t *len, uint8_t *nick)
{
    int32_t lo = 0, hi = (int32_t)nm_n - 1, mid, c;
    uint8_t k, *r;
    *nick = 0;
    name_flags = 0;
    while (lo <= hi) {
        mid = (lo + hi) >> 1;
        r = nm_buf + nm_at[mid];
        c = memcmp(r + 2, key, 32);
        if (!c) {
            *len = r[0] - 33;
            name_flags = r[1];
            *nick = r[1] & NAME_NICKNAME;
            return r + 34;
        }
        if (c < 0)
            lo = mid + 1;
        else
            hi = mid - 1;
    }
    for (k = 0; k < sf_n; k++) {
        r = sf_buf + sf_at[k];
        if (!memcmp(r + 2, key, 32)) {
            *len = r[110];
            return r + 111;
        }
    }
    *len = 0;
    return 0;
}

/* A feed as shown everywhere: its name and its short key in brackets, or
 * the short key alone. */
static void tb_feed(const uint8_t *key)
{
    uint8_t n, nick;
    const uint8_t *name = name_of(key, &n, &nick);
    if (n) {
        tb_name(name, n);
        tb_s(" (");
        tb_key(key);
        tb_s(")");
    } else {
        tb_key(key);
    }
}

/* A size: "812 bytes", "6.1 KiB", "1.0 MiB". */
static void tb_size(uint32_t n)
{
    uint32_t tenths;
    const char *unit = " KiB";
    if (n < 1024) {
        tb_u(n);
        tb_s(n == 1 ? " byte" : " bytes");
        return;
    }
    tenths = (n * 10 + 512) / 1024;
    if (tenths >= 10240) {
        tenths = (n / 1024 * 10 + 512) / 1024;
        unit = " MiB";
    }
    tb_u(tenths / 10);
    tb_s(".");
    tb_u(tenths % 10);
    tb_s(unit);
}

static const char *plural(uint32_t n)
{
    return n == 1 ? "" : "s";
}

static const uint8_t month_days[12] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};

/* `t` as a UTC date and time: "YYYY-MM-DD HH:MM". The machine has a clock
 * but no time zone. Every fourth year is a leap year, which holds from 1901
 * to 2099. */
static void tb_date(uint32_t t)
{
    uint32_t days = t / 86400, secs = t % 86400, year, month = 0, leap, len;
    /* 1970-1973 is 365 + 365 + 366 + 365 days. */
    year = 1970 + days / 1461 * 4;
    days %= 1461;
    for (;;) {
        leap = (year & 3) == 0;
        if (days < 365 + leap)
            break;
        days -= 365 + leap;
        year++;
    }
    for (;;) {
        len = month_days[month] + (month == 1 && leap);
        if (days < len)
            break;
        days -= len;
        month++;
    }
    tb_u(year);
    tb_s("-");
    tb_two(month + 1);
    tb_s("-");
    tb_two(days + 1);
    tb_s(" ");
    tb_two(secs / 3600);
    tb_s(":");
    tb_two(secs % 3600 / 60);
}

static uint32_t get32(const uint8_t *p)
{
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

/* The status message: tb, in colour fg. */
static void say(uint8_t fg)
{
    uint16_t n = tb_n > sizeof msg ? sizeof msg : tb_n;
    /* Cut between characters. */
    while (n < tb_n && n && ((uint8_t)tb[n] & 0xC0) == 0x80)
        n--;
    memcpy(msg, tb, n);
    msg_n = n;
    msg_fg = fg;
    dirty[STATUS_ROW] = 1;
}

/* --- Drawing ------------------------------------------------------------ */

static void row_start(uint8_t row, uint8_t f, uint8_t b)
{
    row_at = row;
    attr = (uint32_t)f << 16 | (uint32_t)b << 24;
    col = 0;
}

static void ink(uint8_t f)
{
    attr = (attr & 0xFF000000u) | (uint32_t)f << 16;
}

static void cell(uint32_t ch)
{
    if (col < COLS)
        cells[col++] = ch | attr;
}

/* A character known to be narrow. */
static void put1(uint16_t cp)
{
    cell(cp);
}

/* Any character: a wide one takes its cell and a continuation cell. U+FFFF
 * would be a continuation, so it shows as U+FFFD (as the write call shows
 * it). */
static void put_w(uint16_t cp, uint8_t w)
{
    if (cp == TERM_CONTINUATION)
        cp = 0xFFFD;
    cell(cp);
    if (w == 2)
        cell(TERM_CONTINUATION);
}

static void put(uint16_t cp)
{
    put_w(cp, width(cp));
}

/* `n` printable ASCII bytes; they fit in the row. */
static void put_ascii(const uint8_t *p, uint32_t n)
{
    uint32_t *c = cells + col, a = attr;
    col += n;
    while (n--)
        *c++ = *p++ | a;
}

/* A string from this program: UTF-8, no wide characters. */
static void puts_(const char *s)
{
    const uint8_t *p = (const uint8_t *)s;
    while (*p)
        cell(title_char(&p));
}

static void spaces(uint32_t n)
{
    uint32_t *c = cells + col, a = attr | ' ';
    if (n > COLS - col)
        n = COLS - col;
    col += n;
    while (n--)
        *c++ = a;
}

static void pad_to(uint8_t c)
{
    if (col < c)
        spaces(c - col);
}

/* Clears the rest of the row in the current colours and writes it. */
static void row_end(void)
{
    spaces(COLS - col);
    term_cells_write((uint32_t)row_at * COLS, cells, COLS);
}

static void put_u32(uint32_t v)
{
    puts_(u32_str(v));
}

/* UTF-8 text in at most `room` columns, with an ellipsis if cut. Text
 * never has more columns than bytes. */
static void put_title(const uint8_t *t, uint8_t n, uint8_t room)
{
    const uint8_t *p = t, *q, *end = t + n;
    uint32_t w = 0, cw, b, limit = n <= room ? room : room - 1;
    uint16_t cp;
    while (p < end) {
        b = limit - w;
        if ((uint32_t)(end - p) < b)
            b = end - p;
        b = ascii_run(b, p);
        put_ascii(p, b);
        p += b;
        w += b;
        if (p == end || w == limit)
            break;
        q = p;
        cp = printable(title_char(&q));
        cw = width(cp);
        if (w + cw > limit)
            break;
        put_w(cp, cw);
        w += cw;
        p = q;
    }
    if (p == end)
        return;
    /* Cut: unless just one character is left and it fits after all. */
    q = p;
    cp = printable(title_char(&q));
    if (q == end && w + width(cp) <= room)
        put(cp);
    else
        put1(0x2026);
}

/* --- The document ------------------------------------------------------ */

static void spend(uint32_t units)
{
    b_work = b_work > units ? b_work - units : 0;
}

/* The full repertoire (SPEC §terminal.text, vectors/full-text.txt): what
 * every host with full text draws with its built-in fonts. Other characters
 * show only where the user added a font that has them (ARTICLE.md 1.1). */
static const uint16_t full_ranges[][2] = {
    {0x0020, 0x007E}, {0x00A0, 0x01C3}, {0x01CD, 0x01E3}, {0x01E6, 0x01F0}, {0x01F4, 0x01F6}, {0x01F8, 0x01F9},
    {0x01FC, 0x0221}, {0x0224, 0x0241}, {0x0243, 0x0245}, {0x024C, 0x024D}, {0x0250, 0x02B9}, {0x02BB, 0x02C1},
    {0x02C6, 0x02C9}, {0x02CC, 0x02D3}, {0x02D6, 0x02DE}, {0x02E0, 0x02E9}, {0x02EE, 0x02EE}, {0x02F3, 0x02F3},
    {0x0300, 0x033F}, {0x0343, 0x0343}, {0x0358, 0x0358}, {0x0361, 0x0361}, {0x0374, 0x0377}, {0x037A, 0x037F},
    {0x0384, 0x038A}, {0x038C, 0x038C}, {0x038E, 0x03A1}, {0x03A3, 0x03CE}, {0x03D0, 0x03E1}, {0x03F0, 0x045F},
    {0x0462, 0x0463}, {0x0472, 0x0473}, {0x0490, 0x049B}, {0x04A2, 0x04A5}, {0x04AA, 0x04B3}, {0x04BA, 0x04BB},
    {0x04C0, 0x04C4}, {0x04C7, 0x04C8}, {0x04CB, 0x04CC}, {0x04CF, 0x04F9}, {0x0510, 0x0511}, {0x051A, 0x051D},
    {0x1E00, 0x1E13}, {0x1E18, 0x1E2D}, {0x1E30, 0x1E4D}, {0x1E54, 0x1E63}, {0x1E68, 0x1E79}, {0x1E7C, 0x1E99},
    {0x1E9B, 0x1E9B}, {0x1E9F, 0x1EA1}, {0x1EAC, 0x1EAD}, {0x1EB0, 0x1EB1}, {0x1EB6, 0x1EB9}, {0x1EBC, 0x1EBD},
    {0x1EC6, 0x1EC7}, {0x1ECA, 0x1ECD}, {0x1ED8, 0x1EDD}, {0x1EE0, 0x1EE5}, {0x1EE8, 0x1EEB}, {0x1EEE, 0x1EF5},
    {0x1EF8, 0x1EF9}, {0x1F00, 0x1F15}, {0x1F18, 0x1F1D}, {0x1F20, 0x1F45}, {0x1F48, 0x1F4D}, {0x1F50, 0x1F57},
    {0x1F59, 0x1F59}, {0x1F5B, 0x1F5B}, {0x1F5D, 0x1F5D}, {0x1F5F, 0x1F7D}, {0x1F80, 0x1FB4}, {0x1FB6, 0x1FC4},
    {0x1FC6, 0x1FD3}, {0x1FD6, 0x1FDB}, {0x1FDD, 0x1FEF}, {0x1FF2, 0x1FF4}, {0x1FF6, 0x1FFE}, {0x2000, 0x200A},
    {0x2010, 0x2023}, {0x2026, 0x2026}, {0x202F, 0x2037}, {0x2039, 0x203A}, {0x203C, 0x203F}, {0x2045, 0x2049},
    {0x204B, 0x204B}, {0x205F, 0x205F}, {0x2070, 0x2071}, {0x2074, 0x208E}, {0x2090, 0x209C}, {0x20A0, 0x20B5},
    {0x20B8, 0x20BA}, {0x20BD, 0x20BD}, {0x2102, 0x2102}, {0x2105, 0x2105}, {0x210D, 0x210F}, {0x2115, 0x2117},
    {0x2119, 0x211A}, {0x211D, 0x211D}, {0x2122, 0x2122}, {0x2124, 0x2124}, {0x2126, 0x2126}, {0x212A, 0x212B},
    {0x212E, 0x212E}, {0x2148, 0x2148}, {0x2150, 0x2151}, {0x2153, 0x215F}, {0x2189, 0x2189}, {0x2190, 0x2213},
    {0x2215, 0x2215}, {0x2217, 0x2220}, {0x2223, 0x2223}, {0x2227, 0x222D}, {0x2234, 0x223D}, {0x2241, 0x2269},
    {0x226D, 0x228B}, {0x228D, 0x22A5}, {0x22B2, 0x22B5}, {0x22B8, 0x22B8}, {0x22C2, 0x22C6}, {0x22CD, 0x22D1},
    {0x22DA, 0x22E9}, {0x22EF, 0x22EF}, {0x2300, 0x2306}, {0x2308, 0x2311}, {0x2318, 0x2319}, {0x231C, 0x2321},
    {0x2325, 0x2328}, {0x232B, 0x232B}, {0x2373, 0x2375}, {0x237A, 0x237A}, {0x237D, 0x237D}, {0x239B, 0x23AE},
    {0x23CE, 0x23CF}, {0x2500, 0x25FC}, {0x25FF, 0x2613}, {0x2616, 0x262F}, {0x2638, 0x2647}, {0x2654, 0x267E},
    {0x2680, 0x2689}, {0x2690, 0x2692}, {0x2694, 0x269C}, {0x26A0, 0x26A0}, {0x26B0, 0x26B1}, {0x2701, 0x2704},
    {0x2706, 0x2709}, {0x270C, 0x2727}, {0x2729, 0x274B}, {0x274D, 0x274D}, {0x274F, 0x2752}, {0x2756, 0x2756},
    {0x2758, 0x275E}, {0x2761, 0x2775}, {0x2794, 0x2794}, {0x2798, 0x27AF}, {0x27B1, 0x27BE}, {0xFFFD, 0xFFFD}
};
#define FULL_N (sizeof full_ranges / sizeof full_ranges[0])

static uint8_t in_full(uint32_t cp)
{
    uint32_t lo = 0, hi = FULL_N, m;
    if (cp > 0xFFFF)
        return 0;
    while (lo < hi) {
        m = (lo + hi) / 2;
        if (full_ranges[m][1] < cp)
            lo = m + 1;
        else
            hi = m;
    }
    return lo < FULL_N && full_ranges[lo][0] <= cp;
}

/* Whether the n bytes of UTF-8 at p show a character outside the full
 * repertoire (control characters are not shown; a malformed sequence is
 * U+FFFD, which is in it). */
static uint8_t beyond_full(const uint8_t *p, uint32_t n)
{
    const uint8_t *e = p + n;
    uint32_t cp;
    uint8_t c;
    while (p < e) {
        /* ASCII a word at a time. */
        if (!((uintptr_t)p & 3))
            while (p + 4 <= e && !(*(const uint32_t *)p & 0x80808080u))
                p += 4;
        if (p >= e)
            break;
        c = *p;
        if (c < 0xC0) { /* ASCII, or a stray continuation byte */
            p++;
            continue;
        }
        if (c >= 0xF0 && p + 3 < e)
            return 1; /* above U+FFFF */
        if (c >= 0xE0 && p + 2 < e) {
            cp = (uint32_t)(c & 0x0F) << 12 | (uint32_t)(p[1] & 0x3F) << 6 | (p[2] & 0x3F);
            p += 3;
        } else if (c < 0xE0 && p + 1 < e) {
            cp = (uint32_t)(c & 0x1F) << 6 | (p[1] & 0x3F);
            p += 2;
        } else {
            p++;
            continue;
        }
        if (cp >= 0xA0 && !in_full(cp))
            return 1;
    }
    return 0;
}

/* The article shown holds a character outside the full repertoire: the
 * status row has said so (ARTICLE.md 1.1). */
static uint8_t art_beyond;

static void doc_rgb(const uint8_t *c)
{
    doc_colour(c[0], c[1], c[2]);
}

static inline __attribute__((always_inline)) uint8_t *find2(uint8_t *p, const uint8_t *e, uint8_t a, uint8_t b);

/* The n bytes at p into the document as text (doc_text); in a table, each
 * "\|" as "|". Returns the bytes of p used (all but a cut at the end if
 * the document filled). */
static uint32_t cell_text(const uint8_t *p, uint32_t n)
{
    uint32_t done = 0, k, m;
    if (b_kind != K_TABLE)
        return doc_text(p, n);
    while (done < n) {
        for (k = done; (k = find2((uint8_t *)p + k, p + n, '\\', '\\') - p) < n && !(k + 1 < n && p[k + 1] == '|'); k++)
            ;
        m = doc_text(p + done, k - done);
        if (m < k - done)
            return done + m;
        done = k < n ? k + 1 : k; /* the backslash is dropped */
    }
    return done;
}

/* Article bytes from b_text to `to` into the document as text. The first
 * that hold a character the built-in fonts may lack make the status row
 * say so. */
static void flush_text(uint32_t to)
{
    if (to > b_text && !doc_overflow) {
        if (!art_beyond) {
            spend((to - b_text) * W_FULL);
            if (beyond_full(art + b_text, to - b_text)) {
                art_beyond = 1;
                tb_n = 0;
                tb_s("Some characters here may need a font: terse fonts add <font file>");
                say(C_INFO);
            }
        }
        b_shown = b_text + cell_text(art + b_text, to - b_text);
    }
    b_text = to;
}

/* The same; in an article opened from a search result, the match (or, if
 * its bytes are not shown as text, the first text after it) carries the
 * anchor MATCH_ID and the match colour. */
static void flush(uint32_t to)
{
    uint32_t a, e;
    uint8_t shown;
    if (mt_state != MT_WANT || to <= b_text || mt_at >= to || doc_overflow) {
        flush_text(to);
        return;
    }
    a = mt_at > b_text ? mt_at : b_text;
    e = mt_at + mt_len;
    shown = e > a;
    if (!shown)
        e = to; /* the match is not shown as text: the text after it */
    else if (e > to)
        e = to;
    flush_text(a);
    doc_anchor(MATCH_ID);
    if (shown)
        doc_rgb(rgb_match);
    flush_text(e);
    doc_anchor(0);
    if (shown)
        doc_rgb(b_colour);
    flush_text(to);
    mt_state = MT_PLACED;
}

static void doc_s(const char *s);

static uint8_t is_alnum(uint8_t c)
{
    return (uint8_t)(c - '0') < 10 || (uint8_t)((c | 0x20) - 'a') < 26;
}

static uint8_t hex_value(uint8_t c);

static uint8_t is_lower_hex(uint8_t c)
{
    return (uint8_t)(c - '0') < 10 || (uint8_t)(c - 'a') < 6;
}

/* Whether art[i .. i + 64) are lowercase hex digits not followed by a
 * letter or digit before `end`. */
static uint8_t hex64_at(uint32_t i, uint32_t end)
{
    uint32_t k;
    if (end - i < 64)
        return 0;
    for (k = 0; k < 64; k++)
        if (!is_lower_hex(art[i + k]))
            return 0;
    return i + 64 == end || !is_alnum(art[i + 64]);
}

static uint8_t is_space(uint32_t c);

/* Whether the n bytes at p are a display name (NETWORK 4.6): UTF-8, 1-48
 * bytes, no control characters, no white space at either end. */
static uint8_t valid_name(const uint8_t *p, uint32_t n)
{
    uint32_t i = 0, c = 0, len, min, k;
    uint8_t b;
    if (!n || n > 48)
        return 0;
    while (i < n) {
        b = p[i];
        if (b < 0x80) {
            c = b, len = 1, min = 0;
        } else if (b >= 0xC2 && b < 0xE0) {
            c = b & 0x1F, len = 2, min = 0x80;
        } else if (b >= 0xE0 && b < 0xF0) {
            c = b & 0x0F, len = 3, min = 0x800;
        } else if (b >= 0xF0 && b < 0xF5) {
            c = b & 0x07, len = 4, min = 0x10000;
        } else {
            return 0;
        }
        if (n - i < len)
            return 0;
        for (k = 1; k < len; k++) {
            if ((p[i + k] & 0xC0) != 0x80)
                return 0;
            c = c << 6 | (p[i + k] & 0x3F);
        }
        if (c < min || c > 0x10FFFF || (c >= 0xD800 && c < 0xE000) || c < 0x20 || (c >= 0x7F && c < 0xA0)
            || (!i && is_space(c)))
            return 0;
        i += len;
    }
    return !is_space(c);
}

/* A program link's name: the n bytes at t, percent-encoded (checked by
 * terse_length), decoded into out (48 bytes); its length, 0 if it is not
 * a display name. */
static uint8_t prog_name(const uint8_t *t, uint32_t n, uint8_t *out)
{
    uint32_t i = 0, k = 0;
    while (i < n) {
        if (k == 48)
            return 0;
        if (t[i] == '%') {
            out[k++] = hex_value(t[i + 1]) << 4 | hex_value(t[i + 2]);
            i += 3;
        } else {
            out[k++] = t[i++];
        }
    }
    return valid_name(out, k) ? k : 0;
}

/* A byte a program link's name may hold as itself (RFC 3986's
 * unreserved characters). */
static uint8_t unreserved(uint8_t c)
{
    return is_alnum(c) || c == '-' || c == '.' || c == '_' || c == '~';
}

/* --- Feed addresses (NETWORK 4.9) ----------------------------------------- */

static uint8_t is_digit(uint8_t c);

/* Whether the n bytes at p are a decimal number up to hi, without leading
 * zeros ("0" itself is one). */
static uint8_t dec_ok(const uint8_t *p, uint32_t n, uint32_t hi)
{
    uint32_t v = 0, k;
    if (!n || n > 5 || (n > 1 && p[0] == '0'))
        return 0;
    for (k = 0; k < n; k++) {
        if (!is_digit(p[k]))
            return 0;
        v = v * 10 + p[k] - '0';
    }
    return v <= hi;
}

/* A host name (labels of 1-63 letters, digits and "-", not starting or
 * ending with "-", the last holding a letter) or an IPv4 address. */
static uint8_t host_ok(const uint8_t *p, uint32_t n)
{
    uint32_t k, s = 0, labels = 0;
    uint8_t letter = 0, ip = 1;
    for (k = 0; k <= n; k++) {
        if (k < n && p[k] != '.') {
            if (!is_alnum(p[k]) && p[k] != '-')
                return 0;
            letter |= !is_digit(p[k]) && p[k] != '-';
            continue;
        }
        if (k == s || k - s > 63 || p[s] == '-' || p[k - 1] == '-')
            return 0;
        ip &= dec_ok(p + s, k - s, 255);
        labels++;
        if (k < n)
            letter = 0;
        s = k + 1;
    }
    return letter || (ip && labels == 4);
}

/* The groups (1-4 hex digits, separated by ":") in the n bytes at p, -1 if
 * malformed. */
static int32_t v6_groups(const uint8_t *p, uint32_t n)
{
    uint32_t k, s = 0, g = 0;
    if (!n)
        return 0;
    for (k = 0; k <= n; k++) {
        if (k < n && p[k] != ':') {
            if (hex_value(p[k]) > 15)
                return -1;
            continue;
        }
        if (k == s || k - s > 4)
            return -1;
        g++;
        s = k + 1;
    }
    return g;
}

/* An IPv6 address: 8 groups, or fewer and one "::" for the rest. */
static uint8_t v6_ok(const uint8_t *p, uint32_t n)
{
    uint32_t at = 0;
    int32_t a, b;
    while (at + 1 < n && (p[at] != ':' || p[at + 1] != ':'))
        at++;
    if (at + 1 >= n)
        return v6_groups(p, n) == 8;
    a = v6_groups(p, at);
    b = v6_groups(p + at + 2, n - at - 2);
    return a >= 0 && b >= 0 && a + b <= 7;
}

/* A hint: host[:port] or [IPv6][:port], at most 80 bytes; the port 1-65535. */
static uint8_t hint_ok(const uint8_t *p, uint32_t n)
{
    uint32_t k = 0;
    if (!n || n > 80)
        return 0;
    if (p[0] == '[') {
        for (k = 1; k < n && p[k] != ']'; k++)
            ;
        if (k == n || !v6_ok(p + 1, k - 1))
            return 0;
        k++;
    } else {
        while (k < n && p[k] != ':')
            k++;
        if (!host_ok(p, k))
            return 0;
    }
    return k == n || (p[k] == ':' && k + 1 < n && p[k + 1] != '0' && dec_ok(p + k + 1, n - k - 1, 65535));
}

/* A hint list: one or two hints separated by ",". */
static uint8_t hints_ok(const uint8_t *p, uint32_t n)
{
    uint32_t k = 0;
    if (n > CONTENT_MAX_HINTS)
        return 0;
    while (k < n && p[k] != ',')
        k++;
    return hint_ok(p, k) && (k == n || hint_ok(p + k + 1, n - k - 1));
}

/* The hints of a feed link, after its "@" at art[i - 1] (ARTICLE.md 4.1):
 * the run of letters, digits, ".", "-", ":", "," and brackets (a "]" only
 * after a "["), less every ".", "," and ":" at its end, if that is a hint
 * list; its length, 0 if not (the link is then the key alone). */
static uint32_t art_hints(uint32_t i, uint32_t end)
{
    uint32_t k = i;
    uint8_t open = 0, c;
    while (k < end) {
        c = art[k];
        if (c == '[')
            open = 1;
        else if (c == ']' && open)
            open = 0;
        else if (!is_alnum(c) && c != '.' && c != '-' && c != ':' && c != ',')
            break;
        k++;
    }
    spend((k - i) * W_EMIT);
    while (k > i && (art[k - 1] == '.' || art[k - 1] == ',' || art[k - 1] == ':'))
        k--;
    return hints_ok(art + i, k - i) ? k - i : 0;
}

/* The length of the terse: link at art[i] (a 't'), 0 if none, and its
 * kind; the block ends at `end`. A feed link takes the hints of its
 * address, if valid. */
static uint32_t terse_length(uint32_t i, uint32_t end, uint8_t *kind)
{
    uint32_t s, k;
    uint8_t name[48];
    if (end - i < 70 || memcmp(art + i, "terse:", 6) || (i > b_start && is_alnum(art[i - 1])))
        return 0;
    spend(W_TERSE);
    if (hex64_at(i + 6, end)) {
        *kind = LK_ARTICLE;
        return 70;
    }
    if (end - i >= 75 && !memcmp(art + i + 6, "feed/", 5) && hex64_at(i + 11, end)) {
        *kind = LK_FEED;
        if (end - i > 76 && art[i + 75] == '@' && (s = art_hints(i + 76, end)))
            return 76 + s;
        return 75;
    }
    /* A program: the name is the run of unreserved characters and %XX
     * triples after the key's "/", less any "." at its end. */
    if (end - i > PROG_NAME && !memcmp(art + i + 6, "program/", 8) && hex64_at(i + PROG_KEY, end)
        && art[i + PROG_NAME - 1] == '/') {
        s = k = i + PROG_NAME;
        while (k < end) {
            if (unreserved(art[k]))
                k++;
            else if (art[k] == '%' && end - k >= 3 && hex_value(art[k + 1]) < 16 && hex_value(art[k + 2]) < 16)
                k += 3;
            else
                break;
        }
        spend((k - s) * W_EMIT);
        while (k > s && art[k - 1] == '.')
            k--;
        if (k - s <= 48 * 3 && prog_name(art + s, k - s, name)) {
            *kind = LK_PROGRAM;
            return k - i;
        }
    }
    return 0;
}

/* 64 hex digits into 32 bytes. */
static void unhex(const uint8_t *h, uint8_t *out)
{
    uint8_t k;
    for (k = 0; k < 32; k++)
        out[k] = hex_value(h[2 * k]) << 4 | hex_value(h[2 * k + 1]);
}

static void prefetch_add(const uint8_t *id);

/* A link to the target of `len` bytes at art[i], shown as the `n` bytes
 * at `show` (a link with text), or as the target itself (0): a terse:
 * link shortened to its first and last six digits. The article bytes up
 * to `next` are done. The first MAX_LINKS links are anchors in the link
 * colour (the current link's colour if it is the current one). An article
 * link is also queued for prefetching. */
static void put_link(uint32_t i, uint32_t len, uint8_t kind, const uint8_t *show, uint32_t n, uint32_t next)
{
    uint8_t sb[11 + 6 + 3 + 6 + 1 + CONTENT_MAX_HINTS], anchored = links < MAX_LINKS, name[48];
    uint32_t key_end = kind == LK_FEED ? 75 : 70;
    b_text = next;
    spend(W_LINK + (next - i) * W_EMIT);
    if (doc_overflow)
        return;
    if (kind == LK_ARTICLE) {
        uint8_t id[32];
        unhex(art + i + 6, id);
        prefetch_add(id);
    }
    if (!show) {
        show = art + i;
        n = len;
        if (kind == LK_PROGRAM) {
            /* "terse:program/", six digits, "…", six digits, "/", the
             * name decoded. */
            tb_n = 0;
            tb_bytes(art + i, PROG_KEY + 6);
            tb_s("\xE2\x80\xA6");
            tb_bytes(art + i + PROG_NAME - 7, 7);
            tb_name(name, prog_name(art + i + PROG_NAME, len - PROG_NAME, name));
            show = (const uint8_t *)tb;
            n = tb_n;
        } else {
            /* "terse:" or "terse:feed/", six digits, "…", six digits, and
             * a feed address's "@" and hints. */
            n = key_end - 58;
            memcpy(sb, art + i, n);
            memcpy(sb + n, "\xE2\x80\xA6", 3);
            memcpy(sb + n + 3, art + i + key_end - 6, 6);
            memcpy(sb + n + 9, art + i + key_end, len - key_end);
            show = sb;
            n += 9 + len - key_end;
        }
    }
    if (anchored) {
        link_at[links] = i;
        link_len[links] = len;
        link_kind[links] = kind;
        doc_anchor(++links);
        link_doc[links - 1] = doc_ptr - doc;
        doc_rgb(links == cur_link ? rgb_cur_link : rgb_link);
    }
    if (cell_text(show, n) == n && !doc_overflow)
        b_shown = next;
    if (anchored) {
        doc_anchor(0);
        doc_rgb(b_colour);
    }
}

/* A link with text: `[`, 1-200 bytes of text without `[`, `]` or a line
 * break, `](`, a target that is a whole link, `)`; the target starts at
 * art[at], the block ends at `end`. Its length and kind, and the text's
 * first byte in *ts; 0 if there is none. Only text not yet in the document
 * (from b_text) on the current line (from b_ls) can be the link's. */
static uint32_t text_link(uint32_t at, uint32_t end, uint32_t *ts, uint8_t *kind)
{
    uint32_t k = at, t = at - 2, len;
    if (at < b_text + 3 || art[at - 1] != '(' || art[at - 2] != ']')
        return 0;
    while (k < end && k - at < 76 + 200 && art[k] > ' ' && art[k] < 0x7F && art[k] != ')')
        k++;
    spend((k - at) * W_EMIT);
    if (k >= end || art[k] != ')')
        return 0;
    len = terse_length(at, k, kind);
    if (len != k - at)
        return 0;
    while (t > b_text && t > b_ls && at - 2 - t < 200 && art[t - 1] != '[' && art[t - 1] != ']' && art[t - 1] != '\n'
           && art[t - 1] != '`')
        t--;
    spend((at - t) * W_EMIT);
    if (t == at - 2 || t <= b_text || t <= b_ls || art[t - 1] != '[')
        return 0;
    *ts = t;
    return len;
}

/* The first byte in [p, e) that is `a` or `b`, or e: a word at a time
 * where it can (a word holds a byte equal to `a` if, with x = w ^ a * ONES,
 * (x - ONES) & ~x & HIGHS is not 0). */
#define ONES 0x01010101u
#define HIGHS 0x80808080u
static inline __attribute__((always_inline)) uint8_t *find2(uint8_t *p, const uint8_t *e, uint8_t a, uint8_t b)
{
    uint32_t ma = a * ONES, mb = b * ONES;
    while (p < e && ((uint32_t)p & 3)) {
        if (*p == a || *p == b)
            return p;
        p++;
    }
    while (e - p >= 4) {
        uint32_t w = *(const uint32_t *)p, x = w ^ ma, y = w ^ mb;
        if ((((x - ONES) & ~x) | ((y - ONES) & ~y)) & HIGHS)
            break;
        p += 4;
    }
    while (p < e && *p != a && *p != b)
        p++;
    return p;
}

/* The first byte in [p, e) that is `a`, `b` or `c`, or e: as find2. */
#define HIT(w, m) ((((w) ^ (m)) - ONES) & ~((w) ^ (m)))
static inline __attribute__((always_inline)) uint8_t *find3(uint8_t *p, const uint8_t *e, uint8_t a, uint8_t b, uint8_t c)
{
    uint32_t ma = a * ONES, mb = b * ONES, mc = c * ONES;
    while (p < e && ((uint32_t)p & 3)) {
        if (*p == a || *p == b || *p == c)
            return p;
        p++;
    }
    while (e - p >= 4) {
        uint32_t w = *(const uint32_t *)p;
        if ((HIT(w, ma) | HIT(w, mb) | HIT(w, mc)) & HIGHS)
            break;
        p += 4;
    }
    while (p < e && *p != a && *p != b && *p != c)
        p++;
    return p;
}

/* The first LF in [p, e), or e. */
/* The first byte in [p, e) below 0Eh (an LF or a CR, or a tab or another
 * control character), or e: a word at a time where it can (a word holds a
 * byte below 0Eh if (w - 0Eh * ONES) & ~w & HIGHS is not 0). */
static inline __attribute__((always_inline)) uint8_t *find_ctl(uint8_t *p, const uint8_t *e)
{
    while (p < e && ((uint32_t)p & 3)) {
        if (*p < 0x0E)
            return p;
        p++;
    }
    while (e - p >= 4) {
        uint32_t x = *(const uint32_t *)p;
        if ((x - 0x0E * ONES) & ~x & HIGHS)
            break;
        p += 4;
    }
    while (p < e && *p >= 0x0E)
        p++;
    return p;
}

static uint8_t is_blank(uint8_t c)
{
    return c == ' ' || c == '\t' || c == '\r';
}

static uint8_t is_digit(uint8_t c)
{
    return (uint8_t)(c - '0') < 10;
}

static uint8_t hex_value(uint8_t c)
{
    if (is_digit(c))
        return c - '0';
    c |= 0x20;
    if ((uint8_t)(c - 'a') < 6)
        return c - 'a' + 10;
    return 255;
}

/* Whether art[from .. to) is all blanks. */
static uint8_t blanks_only(uint32_t from, uint32_t to)
{
    spend((to - from) * W_FIND);
    for (; from < to; from++)
        if (!is_blank(art[from]))
            return 0;
    return 1;
}

/* A figure's size on its opening line: 1 to 4 decimal digits at art[*i]
 * (before `end`) for a number 1-1024, ended by a blank or the end, and the
 * blanks after it; 0 if there is none. */
static uint16_t fence_number(uint32_t *i, uint32_t end)
{
    uint32_t k = *i, v = 0;
    for (; k < end && is_digit(art[k]); k++) {
        if (k - *i == 4)
            return 0;
        v = v * 10 + art[k] - '0';
    }
    if (k == *i || v > 1024 || (k < end && !is_blank(art[k])))
        return 0;
    while (k < end && is_blank(art[k]))
        k++;
    *i = k;
    return v;
}

/* The rest of a fence's opening line, art[i .. end): "path W H" makes the
 * block a figure (b_bad if W or H is not right). */
static void fence_info(uint32_t i, uint32_t end)
{
    spend((end - i) * W_FIND);
    while (i < end && is_blank(art[i]))
        i++;
    if (end - i < 4 || memcmp(art + i, "path", 4) || (end - i > 4 && !is_blank(art[i + 4])))
        return;
    b_kind = K_FIGURE;
    i += 4;
    while (i < end && is_blank(art[i]))
        i++;
    f_w = fence_number(&i, end);
    f_h = fence_number(&i, end);
    b_bad = !f_w || !f_h || i < end;
}

/* The kind of block a line starts. */
static const uint8_t block_of[] = {K_PARA, K_PRE, K_ITEM, K_H1, K_H2, K_FENCE, K_TABLE};

/* A table's line starts at art[i] (its '|'): its cutting starts after it. */
static void table_line(uint32_t i)
{
    b_scan = i + 1;
    t_ps = t_pe = 0;
    t_bar = t_esc = t_dash = rp_n = t_pieces = 0;
    t_al = b_lines == 1;
}

/* A piece of a table's line ends: kept if it is one of the first 8. */
static void table_piece(void)
{
    if (rp_n < 8) {
        rp_s[rp_n] = t_ps;
        rp_e[rp_n] = t_ps ? t_pe : 0;
        rp_n++;
    }
    t_pieces++;
    t_ps = 0;
}

/* The end of a table's line: its last piece, then the row (the first
 * row sets the number of columns; a second line that is an alignment row
 * gives the alignment and the header instead). */
static void table_row_end(void)
{
    uint8_t k, n;
    spend(W_TROW);
    if (!(t_bar && t_pieces))
        table_piece();
    if (b_lines == 0) {
        t_cols = rp_n;
        t_rows = t_head = t_n = 0;
        memset(t_align, DOC_START, sizeof t_align);
    }
    if (b_lines == 1 && t_al && t_dash) {
        t_head = DOC_TABLE_HEADER;
        for (k = 0; k < t_cols && k < rp_n; k++) {
            const uint8_t *c = art + rp_s[k];
            n = rp_s[k] ? rp_e[k] - rp_s[k] : 0;
            t_align[k] = n && c[n - 1] == ':' ? (n >= 2 && c[0] == ':' ? DOC_CENTRE : DOC_END) : DOC_START;
        }
        return;
    }
    if (t_n + t_cols > TC_MAX)
        return; /* the page device would not show it */
    for (k = 0; k < t_cols; k++) {
        tc_s[t_n] = k < rp_n ? rp_s[k] : 0;
        tc_e[t_n] = k < rp_n ? rp_e[k] : 0;
        t_n++;
    }
    t_rows++;
}

/* B_FIND in a table's line: cuts it into pieces up to its LF (or `limit`,
 * or as far as this frame's share goes), leaving b_scan where it got. A CR
 * not before an LF becomes a space (ARTICLE.md 1); one that ends the bytes
 * so far (`final` 0) waits for the next byte. */
static void table_scan(uint32_t limit, uint8_t final)
{
    uint32_t i = b_scan, e = limit;
    uint8_t c;
    if (e - i > b_work / W_TSCAN)
        e = i + b_work / W_TSCAN;
again:
    for (; i < e; i++) {
        c = art[i];
        if (c == '\n' || c == '\r')
            break;
        if (c != '|' && c != '-' && c != ':' && !is_blank(c))
            t_al = 0;
        if (c == '-')
            t_dash = 1;
        if (c == '|' && !t_esc) {
            table_piece();
            t_bar = 1;
        } else if (!is_blank(c)) {
            if (!t_ps)
                t_ps = i;
            t_pe = i + 1;
            t_bar = 0;
        }
        t_esc = c == '\\';
    }
    /* A CR, handled out of the loop (a store into art in it would keep the
     * compiler from holding the t_ variables in registers): a blank. */
    if (i < e && art[i] == '\r' && (i + 1 < limit || final)) {
        if (i + 1 == limit || art[i + 1] != '\n')
            art[i] = ' '; /* a lone CR: a space, cut like one */
        else
            t_esc = 0, i++; /* before the LF: a blank never shown */
        goto again;
    }
    spend((i - b_scan) * W_TSCAN);
    if (i == e && e < limit)
        b_work = 0; /* this frame's share is used */
    b_scan = i;
}

/* B_FIND: gathers the next block from art[0 .. limit) (`final`: no more
 * bytes are coming). Returns 1 with b_kind, b_start, b_end, b_lines (and
 * for a fence b_closed, for a figure f_w, f_h and b_bad) set, or 0 if it
 * needs more bytes or this frame's share is used; at the end of the article
 * it moves on to B_FINISH. Resumable: b_in_line is 0 at the start of a
 * line, 1 while classifying it (b_scan: in its leading blanks), 2 looking
 * for its LF (b_scan). A line that starts another block ends the one
 * gathered and is looked at again for the next. */
static uint8_t find_block(uint32_t limit, uint8_t final)
{
    uint8_t *p;
    const uint8_t *e;
    uint32_t q, next;
    uint8_t fence, k, c, c1, c2;
    for (;;) {
        if (!b_work)
            return 0;
        fence = b_kind >= K_FENCE;
        if (b_in_line == 0) {
            if (b_line >= limit) {
                if (!final)
                    return 0;
                if (b_kind)
                    return 1;
                b_phase = B_FINISH;
                return 0;
            }
            spend(W_LINE);
            b_scan = b_line;
            b_in_line = 1;
        }
        if (b_in_line == 1) {
            c = art[b_line];
            q = b_line;
            if (!fence && (is_blank(c) || c == '\n')) {
                p = art + b_scan;
                e = art + limit;
                if ((uint32_t)(e - p) > b_work / W_FIND)
                    e = p + b_work / W_FIND;
                while (p < e && is_blank(*p))
                    p++;
                spend((p - (art + b_scan)) * W_FIND);
                b_scan = q = p - art;
                if (q < limit && is_blank(art[q]))
                    return 0; /* this frame's share is used */
                if (q == limit && !final)
                    return 0;
                if (q == limit || art[q] == '\n') { /* a blank line */
                    b_in_line = 0;
                    b_line = q < limit ? q + 1 : q;
                    if (b_kind)
                        return 1;
                    continue;
                }
                k = c == '\t' || (c == ' ' && q - b_line >= 4 && art[b_line + 1] == ' ' && art[b_line + 2] == ' '
                                  && art[b_line + 3] == ' ')
                        ? L_INDENT
                        : L_PLAIN;
            } else {
                if (limit - b_line < 3 && !final)
                    return 0;
                c1 = b_line + 1 < limit ? art[b_line + 1] : 0;
                c2 = b_line + 2 < limit ? art[b_line + 2] : 0;
                if (c == '`' && c1 == '`' && c2 == '`')
                    k = L_FENCE;
                else if (fence)
                    k = L_PLAIN;
                else if (c == '|')
                    k = L_TABLE;
                else if (c == '#' && c1 == ' ')
                    k = L_H1;
                else if (c == '#' && c1 == '#' && c2 == ' ')
                    k = L_H2;
                else if ((c == '-' || c == '*') && c1 == ' ')
                    k = L_ITEM;
                else
                    k = L_PLAIN;
            }
            b_lkind = k;
            if (!fence) {
                if (b_kind && !(k == L_PLAIN && (b_kind == K_PARA || b_kind == K_ITEM)) && !(k == L_INDENT && b_kind == K_PRE)
                    && !(k == L_TABLE && b_kind == K_TABLE)) {
                    b_in_line = 0; /* this line starts the next block */
                    return 1;
                }
                if (!b_kind) {
                    b_kind = block_of[k];
                    b_start = k == L_PLAIN ? q : k == L_ITEM || k == L_H1 ? b_line + 2 : k == L_H2 ? b_line + 3 : b_line;
                    b_open = b_line;
                    b_lines = b_closed = b_bad = 0;
                }
                if (k == L_TABLE)
                    table_line(b_line);
            }
            b_in_line = 2;
            continue;
        }
        if (b_kind == K_TABLE) {
            table_scan(limit, final);
            if (b_scan < limit && art[b_scan] != '\n')
                return 0; /* this frame's share is used */
            if (b_scan == limit && !final)
                return 0;
            table_row_end();
            b_end = b_scan;
            b_lines++;
            b_line = b_scan < limit ? b_scan + 1 : b_scan;
            b_in_line = 0;
            continue;
        }
        p = art + b_scan;
        e = art + limit;
        if ((uint32_t)(e - p) > b_work / W_FIND)
            e = p + b_work / W_FIND;
        /* A CR not before an LF is a blank shown as a space (ARTICLE.md 1):
         * it becomes one here (the device ignores the CR before an LF). A CR
         * that ends the bytes so far waits for the next byte. */
        for (;;) {
            p = find_ctl(p, e);
            if (p == e || *p == '\n')
                break;
            if (*p == '\r') {
                if (p + 1 == art + limit && !final)
                    break;
                if (p + 1 == art + limit || p[1] != '\n')
                    *p = ' ';
            }
            p++;
        }
        spend((p - (art + b_scan)) * W_FIND);
        b_scan = p - art;
        if (b_scan < limit && *p != '\n')
            return 0; /* this frame's share is used */
        if (b_scan == limit && !final)
            return 0;
        next = b_scan < limit ? b_scan + 1 : b_scan;
        q = b_line;
        b_line = next;
        b_in_line = 0;
        if (fence) {
            if (q == b_open) {
                fence_info(q + 3, b_scan);
                b_start = b_end = next; /* no lines yet: an empty figure's path string */
            } else if (b_lkind == L_FENCE && blanks_only(q + 3, b_scan)) {
                b_closed = 1;
                return 1;
            } else {
                if (!b_lines)
                    b_start = q;
                b_end = b_scan;
                b_lines++;
            }
        } else {
            b_end = b_scan;
            b_lines++;
            if (b_kind == K_H1 || b_kind == K_H2)
                return 1;
        }
    }
}

static const uint8_t style_face[] = {DOC_SANS, DOC_MONO, DOC_SANS, DOC_SANS, DOC_SANS};
static const uint8_t style_flags[] = {0, 0, DOC_BOLD, DOC_BOLD, 0};
static const uint8_t style_size[] = {BODY_SIZE, PRE_SIZE, H1_SIZE, H2_SIZE, TABLE_SIZE};
static const uint8_t *const style_rgb[] = {rgb_body, rgb_pre, rgb_title, rgb_title, rgb_body};

static void set_style(uint8_t s)
{
    if (s != b_style) {
        doc_font(style_face[s], style_flags[s]);
        doc_size(style_size[s]);
        doc_rgb(style_rgb[s]);
        b_style = s;
    }
    b_colour = style_rgb[s];
}

static void figure_start(void);

/* B_EMIT: starts the block's paragraph (a figure: its conversion; a fence
 * with nothing in it: nothing). */
static void start_block(void)
{
    spend(W_BLOCK);
    b_emit = b_text = b_ls = b_start;
    b_skip = b_kind < K_PRE;
    switch (b_kind) {
    case K_FIGURE:
        if (b_closed && !b_bad) {
            figure_start();
            return;
        }
        b_kind = K_FENCE;
        /* fall through: shown as a fence */
    case K_FENCE:
        if (!b_lines) {
            b_end = b_start;
            return;
        }
        /* fall through */
    case K_PRE:
        set_style(S_PRE);
        doc_paragraph(DOC_START, PRE_INDENT, PARA_SPACE, DOC_NOWRAP);
        break;
    case K_H1:
        set_style(S_H1);
        doc_paragraph(DOC_START, 0, H1_SPACE, 0);
        break;
    case K_H2:
        set_style(S_H2);
        doc_paragraph(DOC_START, 0, H2_SPACE, 0);
        break;
    case K_TABLE:
        spend(W_TABLE);
        set_style(S_TABLE);
        doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
        doc_rgb(rgb_table_rule);
        doc_table(t_cols, t_head, t_align);
        doc_rgb(b_colour);
        t_at = t_in = 0;
        b_skip = 0;
        break;
    case K_ITEM:
        set_style(S_BODY);
        doc_paragraph(DOC_START, ITEM_INDENT, b_prev == K_ITEM ? ITEM_SPACE : PARA_SPACE, 0);
        doc_s("\xE2\x80\xA2 ");
        break;
    default:
        set_style(S_BODY);
        doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
    }
}

/* A code span at art[c] (a backquote), the block ending at `end`: 1 to
 * 200 bytes that are neither a backquote nor a LF, then a backquote.
 * Returns 0 if there is none; else puts the text before it and the span
 * (in the mono face, a size smaller, in the code colour; in a table "\|"
 * as "|") into the document, with b_text after it. */
static uint8_t code_span(uint32_t c, uint32_t end)
{
    uint32_t q = c + 1;
    uint8_t s = b_style;
    while (q < end && q - c <= 200 && art[q] != '`' && art[q] != '\n')
        q++;
    spend((q - c) * W_EMIT + W_CODE);
    if (q >= end || art[q] != '`' || q == c + 1)
        return 0;
    flush(c);
    doc_font(DOC_MONO, style_flags[s]);
    doc_size(style_size[s] - CODE_DOWN);
    doc_rgb(rgb_code);
    if (mt_state == MT_WANT && mt_at <= q) { /* the match is in the span: it is anchored */
        doc_anchor(MATCH_ID);
        cell_text(art + c + 1, q - c - 1);
        doc_anchor(0);
        mt_state = MT_PLACED;
    } else {
        cell_text(art + c + 1, q - c - 1);
    }
    doc_font(style_face[s], style_flags[s]);
    doc_size(style_size[s]);
    doc_rgb(b_colour);
    if (!doc_overflow)
        b_shown = q + 1;
    b_text = q + 1;
    return 1;
}

static uint8_t emit_block(void);

/* B_EMIT of a table: its cells, each a Cell command and its text (links
 * and code spans as in a paragraph), then End table. Returns 0 if this
 * frame's share is used. */
static uint8_t table_step(void)
{
    for (; t_at < t_n; t_at++) {
        if (!t_in) {
            spend(W_CELL);
            if (!b_work)
                return 0;
            doc_cell();
            b_emit = b_text = b_ls = tc_s[t_at];
            b_end = tc_e[t_at];
            t_in = 1;
        }
        if (b_end > b_emit && !emit_block())
            return 0;
        t_in = 0;
    }
    doc_table_end();
    if (!doc_overflow)
        b_shown = b_line;
    return 1;
}

/* B_EMIT: the block's text, with LFs as spaces (and the blanks after them
 * dropped) unless it is preformatted, and links marked. Returns 1 when the
 * block is in the document, 0 if this frame's share is used (the text so
 * far is added, cut between characters). */
static uint8_t emit_block(void)
{
    uint32_t end = b_end, i, k;
    uint8_t *p = art + b_emit, *e = art + end, *stop, *q;
    uint8_t pre = b_kind >= K_PRE;
    for (;;) {
        stop = (uint32_t)(e - p) > b_work / W_EMIT ? p + b_work / W_EMIT : e;
        if (b_skip) {
            q = p;
            while (q < stop && is_blank(*q))
                q++;
            spend((q - p) * W_EMIT);
            p = q;
            b_text = p - art;
            if (p >= stop)
                break;
            b_skip = 0;
        }
        q = pre ? find2(p, stop, '\n', ':') : find3(p, stop, '\n', ':', '`');
        spend((q - p) * W_EMIT);
        p = q;
        if (p >= stop)
            break;
        spend(W_HIT);
        if (*p == '`') {
            if (!code_span(p - art, end))
                p++;
            else
                p = art + b_text;
            continue;
        }
        if (*p == '\n') {
            b_ls = p + 1 - art;
            if (!pre) {
                *p = ' ';
                if (p + 1 < e && is_blank(p[1])) {
                    flush(p + 1 - art);
                    b_skip = 1;
                }
            }
        } else {
            /* A link is "terse:" and an ID, "feed/" and a key or
             * "program/", a key and a name; with text, its target after
             * "](", else bare. (Web addresses are not links: ARTICLE.md.) */
            uint32_t c = p - art, at = c - 5, len, ts;
            uint8_t kind = LK_ARTICLE;
            if (c >= b_text + 5 && art[at] == 't' && c + 1 < end && (is_lower_hex(art[c + 1]) || art[c + 1] == 'p')) {
                if (at && art[at - 1] == '(' && (len = text_link(at, end, &ts, &kind)))
                    goto text;
                if ((len = terse_length(at, end, &kind))) {
                    flush(at);
                    put_link(at, len, kind, 0, 0, at + len);
                    p = art + at + len;
                    continue;
                }
            }
            p++;
            continue;
        text:
            flush(ts - 1);
            put_link(at, len, kind, art + ts, at - 2 - ts, at + len + 1);
            p = art + at + len + 1;
            continue;
        }
        p++;
    }
    i = p - art;
    if (i >= end) {
        flush(end);
        return 1;
    }
    /* This frame's share is used: add the text so far, cut before a `[`
     * on this line (which may start a link with text), else after a space
     * (so that no link is cut), or else between characters but before a
     * "terse" cut short (text without spaces, CJK say, may run into a
     * bare link) and not inside a table's "\|". */
    for (k = i; k > b_text && k > b_ls && i - k < 280 && art[k - 1] != '[' && art[k - 1] != '`'; k--)
        ;
    if (k > b_text && k > b_ls && (art[k - 1] == '[' || art[k - 1] == '`')) {
        i = k - 1;
    } else {
        for (k = i; k > b_text && i - k < 64 && art[k - 1] > ' '; k--)
            ;
        if (k > b_text && art[k - 1] <= ' ') {
            i = k;
        } else {
            for (k = i - b_text > 5 ? i - 5 : b_text + 1; k < i; k++)
                if (!memcmp(art + k, "terse", i - k))
                    break;
            if (k < i)
                i = k;
            if (i - 1 > b_text && art[i] == '|' && art[i - 1] == '\\')
                i--;
            while (i > b_text && (art[i] & 0xC0) == 0x80)
                i--;
        }
    }
    flush(i);
    b_emit = i;
    return 0;
}

/* --- Figures ------------------------------------------------------------ */

static uint8_t is_sep(uint8_t c)
{
    return c == ' ' || c == ',' || c == '\n' || c == '\t' || c == '\r';
}

/* A number at *pp (before e): its value in sixteenths of a unit, rounded,
 * halves away from zero, and in *ex the sign of what the rounding took off
 * (1: the number as written is greater, -1: less, 0: neither), since
 * ranges apply to numbers as written (ARTICLE.md 6.1). Returns 0 if there
 * is none or it is longer than 32 bytes. Digits after the sixth decimal do
 * not change the rounding, only (if not all 0) a sign that would be 0. */
static uint8_t read_num(const uint8_t **pp, const uint8_t *e, int32_t *v, int8_t *ex)
{
    const uint8_t *p = *pp;
    uint32_t ip = 0, fr = 0, den = 1, n = 0, up, more = 0;
    int32_t rem;
    uint8_t neg = *p == '-';
    if (e - p > 33)
        e = p + 33;
    if (*p == '-' || *p == '+')
        p++;
    for (; p < e && is_digit(*p); p++, n++)
        if (ip < 100000)
            ip = ip * 10 + *p - '0';
    if (p < e && *p == '.') {
        for (p++; p < e && is_digit(*p); p++, n++) {
            if (den < 1000000) {
                fr = fr * 10 + *p - '0';
                den *= 10;
            } else {
                more |= *p != '0';
            }
        }
    }
    if (!n || p - *pp > 32)
        return 0;
    spend((p - *pp) * W_PBYTE);
    up = (fr * 32 + den) / (2 * den);
    rem = (int32_t)(fr * 16) - (int32_t)(up * den);
    *ex = rem > 0 || (!rem && more) ? 1 : rem < 0 ? -1 : 0;
    if (neg)
        *ex = -*ex;
    ip = ip * 16 + up;
    *v = neg ? -(int32_t)ip : (int32_t)ip;
    *pp = p;
    return 1;
}

/* Whether a coordinate or width rounded to v, *ex the sign of what the
 * rounding took off, is beyond `max` sixteenths (or below -max) as
 * written. */
static uint8_t beyond(int32_t v, int8_t ex, int32_t max)
{
    return v > max || v < -max || (v == max && ex > 0) || (v == -max && ex < 0);
}

/* A colour at *pp: #rgb, #rrggbb, #rrggbbaa, none or fg, followed by a
 * separator or the end. Returns 0 if malformed. */
static uint8_t read_colour(const uint8_t **pp, const uint8_t *e, uint8_t *c)
{
    const uint8_t *p = *pp;
    uint32_t v = 0, n = 0;
    if (*p == '#') {
        for (p++; p < e && hex_value(*p) < 16 && n < 9; p++, n++)
            v = v << 4 | hex_value(*p);
        if (n == 3)
            v = ((v >> 8) * 0x11) << 24 | ((v >> 4 & 15) * 0x11) << 16 | ((v & 15) * 0x11) << 8 | 255;
        else if (n == 6)
            v = v << 8 | 255;
        else if (n != 8)
            return 0;
    } else if (e - p >= 4 && !memcmp(p, "none", 4)) {
        p += 4;
    } else if (e - p >= 2 && p[0] == 'f' && p[1] == 'g') {
        p += 2;
        v = (uint32_t)rgb_body[0] << 24 | (uint32_t)rgb_body[1] << 16 | (uint32_t)rgb_body[2] << 8 | 255;
    } else {
        return 0;
    }
    if (p < e && !is_sep(*p))
        return 0;
    c[0] = v >> 24;
    c[1] = v >> 16;
    c[2] = v >> 8;
    c[3] = v;
    spend((p - *pp) * W_PBYTE);
    *pp = p;
    return 1;
}

/* A coordinate or width in sixteenths as drawn: scaled if the figure is. */
static int32_t scaled(int32_t v)
{
    if (f_scale)
        v = (v * RECT_W + (v < 0 ? -(f_scale / 2) : f_scale / 2)) / f_scale;
    return v;
}

static const char cmd_letter[] = "MLHVCSQTZ";
static const uint8_t cmd_args[] = {2, 2, 1, 1, 6, 4, 4, 2, 0};

/* The segment of f_cmd with the numbers in f_arg, as absolute binary
 * commands. Returns 0 if a point is out of range. */
static uint8_t segment(void)
{
    uint8_t up = f_cmd & 0xDF, i = 0, k, n;
    int32_t ox = f_cmd != up ? f_cx : 0, oy = f_cmd != up ? f_cy : 0, pt[6];
    int8_t ex[6] = {0};
    int16_t s[6];
    spend(W_PSEG);
    if (up == 'H' || up == 'V') {
        pt[0] = up == 'H' ? f_arg[0] + ox : f_cx;
        pt[1] = up == 'V' ? f_arg[0] + oy : f_cy;
        ex[up == 'V'] = f_ex[0];
        n = 2;
    } else {
        if (up == 'S' || up == 'T') {
            /* The reflection of the last control point, if the last
             * segment was of the same family, else the current point. */
            uint8_t same = f_last == (up == 'S' ? 'C' : 'Q');
            pt[0] = same ? 2 * f_cx - f_qx : f_cx;
            pt[1] = same ? 2 * f_cy - f_qy : f_cy;
            i = 2;
        }
        n = i + f_argc;
        for (k = 0; i < n; i++, k++) {
            pt[i] = f_arg[k] + (i & 1 ? oy : ox);
            ex[i] = f_ex[k];
        }
    }
    /* Every point within +-2047 units as written: the current point (exact)
     * plus the numbers as written; reflected points as computed. */
    for (i = 0; i < n; i++) {
        if (beyond(pt[i], ex[i], F_MAX))
            return 0;
        s[i] = scaled(pt[i]);
    }
    if (up == 'M') {
        f_sx = pt[0];
        f_sy = pt[1];
        f_moved = 1;
    } else if (f_need) {
        /* A new subpath from the current point: a later Z returns here. */
        path_move(scaled(f_cx), scaled(f_cy));
        f_sx = f_cx;
        f_sy = f_cy;
    }
    f_need = 0;
    path_points(up == 'M' ? PATH_MOVE : n == 2 ? PATH_LINE : n == 4 ? PATH_QUAD : PATH_CUBIC, n / 2, s);
    f_last = n == 6 ? 'C' : n == 4 ? 'Q' : 0;
    if (n > 2) {
        f_qx = pt[n - 4];
        f_qy = pt[n - 3];
    }
    f_cx = pt[n - 2];
    f_cy = pt[n - 1];
    return 1;
}

static void stroke(int32_t w);

/* B_EMIT of a figure: path commands go after room for a Paragraph and the
 * Figure's head, where the document will hold them; until the figure is
 * complete, doc_ptr does not move, so a partial submit leaves it out and a
 * malformed path leaves nothing behind. */
static void figure_start(void)
{
    uint32_t cap;
    set_style(S_BODY); /* the initial stroke is the text colour: fg */
    f_base = doc_ptr + 5 + DOC_FIGURE_HEAD;
    cap = doc_overflow || f_base > doc_limit ? 0 : doc_limit - f_base;
    path_begin(f_base, cap > 0xFFFF ? 0xFFFF : cap);
    f_pos = b_start;
    f_cmd = f_n = f_word = f_moved = f_need = f_last = f_none = 0;
    f_cx = f_cy = f_sx = f_sy = 0;
    f_scale = f_w > RECT_W ? f_w : 0;
    if (f_scale) { /* the initial stroke's width scales too */
        memcpy(f_rgba, rgb_body, 3);
        f_rgba[3] = 255;
        stroke(16);
    }
}

static void stroke(int32_t w)
{
    int32_t s = scaled(w);
    path_stroke(f_rgba[0], f_rgba[1], f_rgba[2], f_rgba[3], w && !s ? 1 : s);
    f_word = FW_NONE;
}

/* B_EMIT of a figure: converts its path string. Returns 0 if this frame's
 * share is used, 1 when the path is converted, 2 if it is malformed (or
 * does not fit). */
static uint8_t figure_step(void)
{
    const uint8_t *p = art + f_pos, *e = art + b_end, *q, *s;
    int32_t v;
    int8_t x;
    uint8_t c, k;
    for (;;) {
        if (path_overflow)
            return 2;
        if (!b_work) {
            f_pos = p - art;
            return 0;
        }
        q = (uint32_t)(e - p) > b_work / W_PSEP ? p + b_work / W_PSEP : e;
        for (s = p; p < q && is_sep(*p); p++)
            ;
        spend((p - s) * W_PSEP);
        if (p == q && p < e) {
            b_work = 0;
            continue;
        }
        if (p == e) {
            if (f_n || f_none || f_word == FW_FILL || f_word == FW_STROKE)
                return 2;
            if (f_word == FW_WIDTH)
                stroke(16);
            return path_overflow ? 2 : 1;
        }
        spend(W_PTOKEN);
        c = *p;
        if (f_word == FW_FILL || f_word == FW_STROKE) {
            if (!read_colour(&p, e, f_rgba))
                return 2;
            if (f_word == FW_FILL) {
                path_fill(f_rgba[0], f_rgba[1], f_rgba[2], f_rgba[3]);
                f_word = FW_NONE;
            } else {
                f_word = FW_WIDTH;
            }
            continue;
        }
        if (is_digit(c) || c == '.' || c == '-' || c == '+') {
            if (!read_num(&p, e, &v, &x))
                return 2;
            if (f_word == FW_WIDTH) {
                if (v < 0 || beyond(v, x, F_MAX) || (!v && x < 0)) /* 0 to 2047 as written */
                    return 2;
                stroke(v);
                continue;
            }
            if (!f_cmd || (f_cmd & 0xDF) == 'Z')
                return 2;
            f_ex[f_n] = x;
            f_arg[f_n++] = v;
            f_none = 0;
            if (f_n == f_argc) {
                f_n = 0;
                if (!segment())
                    return 2;
                if ((f_cmd & 0xDF) == 'M')
                    f_cmd += 'L' - 'M'; /* more pairs are lines */
            }
            continue;
        }
        if (f_word == FW_WIDTH)
            stroke(16);
        if (f_n || f_none)
            return 2;
        /* A word starts the path string or follows a separator (p[-1] of
         * the path string's first byte is the opening line's LF); a command
         * letter may follow a number directly. */
        if (is_sep(p[-1]) && e - p > 4 && !memcmp(p, "fill", 4) && is_sep(p[4])) {
            p += 4;
            f_word = FW_FILL;
        } else if (is_sep(p[-1]) && e - p > 6 && !memcmp(p, "stroke", 6) && is_sep(p[6])) {
            p += 6;
            f_word = FW_STROKE;
        } else {
            for (k = 0; k < 9 && cmd_letter[k] != (c & 0xDF); k++)
                ;
            if (k == 9 || (k && !f_moved))
                return 2;
            p++;
            f_cmd = c;
            f_argc = cmd_args[k];
            f_none = f_argc != 0;
            if (k == 8) {
                /* Z with no subpath open (after a Z or a word) does nothing
                   and leaves the current point where it is (ARTICLE.md 6.3). */
                spend(W_PSEG);
                if (!f_need) {
                    path_close();
                    f_cx = f_sx;
                    f_cy = f_sy;
                    f_need = 1;
                }
                f_last = 0;
            }
            continue;
        }
        /* A word ends the path so far: what follows starts a new one. */
        f_cmd = 0;
        f_need = 1;
    }
}

/* The converted figure into the document: scaled to the column if wider. */
static void figure_end(void)
{
    uint16_t w = f_w, h = f_h;
    if (f_scale) {
        w = RECT_W;
        h = (f_h * RECT_W + f_scale / 2) / f_scale;
        if (!h)
            h = 1;
    }
    spend(W_FIGURE);
    doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
    doc_figure(w, h, DOC_CENTRE, f_base, path_length(f_base));
    if (!doc_overflow)
        b_shown = b_end;
}

static void send_scroll(void)
{
    if (scroll != scroll_sent) {
        page_scroll(scroll);
        scroll_sent = scroll;
    }
}

/* The article's document as built so far (doc_ptr while building, a_ptr
 * otherwise). */
static void submit_len(uint32_t n)
{
    page_submit(doc, n);
    scroll_sent = 0; /* a submit sets the scroll value to 0 */
    dirty[STATUS_ROW] = 1;
    /* The answer for the document replaced will not come. */
    if (loc_id)
        page_locate(loc_id);
}

static void submit_doc(void)
{
    submit_len(doc_length(doc));
}

/* B_FINISH: the closing note if not all of the article is shown, End, and
 * the whole document to the device. */
static void finish_doc(void)
{
    uint8_t cut = doc_overflow || art_have < art_size;
    doc_limit = doc + DOC_CAP;
    doc_overflow = 0;
    if (cut) {
        doc_paragraph(DOC_START, 0, 16, 0);
        doc_font(DOC_SANS, DOC_ITALIC);
        doc_size(NOTE_SIZE);
        doc_rgb(rgb_meta);
        tb_n = 0;
        tb_s("[\xE2\x80\xA6 the article continues: this reader shows ");
        tb_u(b_shown);
        tb_s(" of its ");
        tb_u(art_size);
        tb_s(" bytes]");
        doc_text(tb, tb_n);
    }
    doc_end();
    submit_doc();
    doc_state = 2;
    b_phase = B_DONE;
}

static void doc_head(void);

/* An article whose first line is a heading ("# " and 1 to 120 bytes) is
 * titled by it, and the heading is not shown again, if it has no entry or
 * its entry has that title. */
static void heading_title(void)
{
    /* A byte order mark first is not shown (ARTICLE.md 1): the article
     * starts after it. */
    uint32_t b = art_have >= 3 && art[0] == 0xEF && art[1] == 0xBB && art[2] == 0xBF ? 3 : 0, i = b + 2, e;
    hd_len = 0;
    b_line = b;
    if (art_have < b + 3 || art[b] != '#' || art[b + 1] != ' ')
        return;
    for (e = b + 2; e < art_have && e < b + 2 + 160 && art[e] != '\n'; e++)
        ;
    if (e == art_have ? art_have < art_want : art[e] != '\n')
        return;
    b_line = e + 1 < art_have ? e + 1 : art_have;
    while (i < e && is_blank(art[i]))
        i++;
    while (e > i && is_blank(art[e - 1]))
        e--;
    if (e == i || e - i > 120 || (art_entry && (art_tlen != e - i || memcmp(art_title, art + i, e - i)))) {
        b_line = b;
        return;
    }
    hd_at = i;
    hd_len = e - i;
}

/* A frame's share of building the document; the first frame of it submits
 * what there is. It starts (with the head) once the article's bytes are
 * coming and its entry, if it is looked for, is known. */
static void build(void)
{
    uint32_t limit = art_have;
    uint8_t final = art_have >= art_want, a = 1;
    if (art_state != READY || b_phase == B_DONE || lk_handle)
        return;
    if (b_phase == B_START) {
        art_beyond = 0;
        heading_title();
        doc_begin(doc, DOC_CAP - DOC_RESERVE);
        doc_head();
        b_phase = B_FIND;
    } else {
        doc_ptr = a_ptr;
        doc_limit = a_limit;
        doc_overflow = a_overflow;
    }
    if (final && art_have < art_size) {
        /* Cut short: end at a character, not in one. */
        uint32_t j = limit;
        while (j && limit - j < 3 && (art[j - 1] & 0xC0) == 0x80)
            j--;
        if (j) {
            uint8_t c = art[j - 1];
            uint32_t need = c >= 0xF0 ? 4 : c >= 0xE0 ? 3 : c >= 0xC0 ? 2 : 1;
            if (limit - (j - 1) < need)
                limit = j - 1;
        }
    }
    b_work = BUILD_UNITS;
    while (b_work && b_phase != B_FINISH) {
        if (doc_overflow) {
            b_phase = B_FINISH;
            break;
        }
        if (b_phase == B_FIND) {
            if (!find_block(limit, final))
                break;
            start_block();
            b_phase = B_EMIT;
        } else {
            uint8_t r = b_kind == K_FIGURE ? figure_step() : b_kind == K_TABLE ? table_step() : emit_block();
            if (!r)
                break;
            if (r == 2) { /* a malformed path: shown as a fence */
                b_kind = K_FENCE;
                start_block();
                continue;
            }
            if (b_kind == K_FIGURE)
                figure_end();
            b_prev = b_kind;
            b_kind = K_NONE;
            b_phase = B_FIND;
        }
    }
    if (doc_overflow || b_phase == B_FINISH)
        finish_doc();
    else if (doc_state == 0) {
        submit_doc();
        doc_state = 1;
    } else {
        a = 0; /* nothing submitted */
    }
    if (a && mt_state == MT_PLACED && !loc_id && !cur_link && !tab_scrolled) {
        /* The match's anchor is in the document submitted: where is it? */
        mt_state = MT_LOCATING;
        loc_id = MATCH_ID;
        loc_again = 0;
        page_locate(MATCH_ID);
    } else if (b_phase == B_DONE && mt_state == MT_WANT) {
        mt_state = MT_NONE;
        tb_n = 0;
        tb_s("The match is not in the text shown, so the article opens at the top");
        say(C_INFO);
    }
    a_ptr = doc_ptr;
    a_limit = doc_limit;
    a_overflow = doc_overflow;
}

/* --- Pages ------------------------------------------------------------- */

static void doc_s(const char *s)
{
    uint32_t n = 0;
    while (s[n])
        n++;
    doc_text(s, n);
}

/* Sets the rectangle (x RECT_X or LIST_X, rows 0 .. h) if it changed. */
static void region(uint8_t x, uint16_t h)
{
    if (region_x != x || region_h != h) {
        page_region(x, 0, x == LIST_X ? LIST_W : RECT_W, h);
        region_x = x;
        region_h = h;
    }
}

static void ldoc_begin(void)
{
    doc_begin(ldoc, LDOC_CAP - 1); /* the last byte for End */
}

static void ldoc_submit(void)
{
    doc_limit = ldoc + LDOC_CAP;
    doc_overflow = 0;
    doc_end();
    page_submit(ldoc, doc_length(ldoc));
}

/* A list document of `slots` slots, in the list's rectangle. */
static void list_submit(uint8_t slots)
{
    region(LIST_X, rect_h);
    ldoc_submit();
    last_slots = slots;
}

/* Any other document, in the centred column. */
static void column_submit(void)
{
    region(RECT_X, PAGE_H);
    ldoc_submit();
    last_slots = 0;
}

/* A slot (see the top): its paragraph and first line's font, colour and
 * anchor; slot_meta ends the first line and starts the second; slot_end
 * ends the anchor. */
static void slot_begin(const uint8_t *rgb, uint8_t flags, uint16_t anchor)
{
    doc_paragraph(DOC_START, 0, SLOT_SPACE, DOC_NOWRAP | DOC_ELLIPSIS);
    doc_font(DOC_SANS, flags);
    doc_size(SLOT_TITLE);
    doc_rgb(rgb);
    if (anchor)
        doc_anchor(anchor);
}

static void slot_meta(const uint8_t *rgb)
{
    doc_text("\n", 1);
    doc_font(DOC_SANS, 0);
    doc_size(SLOT_META);
    doc_rgb(rgb);
}

static void slot_end(uint16_t anchor)
{
    if (anchor)
        doc_anchor(0);
}

/* A title on one line: control characters and line and paragraph
 * separators (U+2028, U+2029) become spaces; no title is "Untitled". */
static void doc_title(const uint8_t *t, uint8_t n)
{
    uint8_t buf[120], k = 0, i, c;
    if (!n) {
        doc_s("Untitled");
        return;
    }
    for (i = 0; i < n; i++) {
        c = t[i];
        if (c < 0x20 || c == 0x7F) {
            c = ' ';
        } else if (c == 0xE2 && i + 2 < n && t[i + 1] == 0x80 && (t[i + 2] == 0xA8 || t[i + 2] == 0xA9)) {
            c = ' ';
            i += 2;
        }
        buf[k++] = c;
    }
    doc_text(buf, k);
}

/* --- Lists ------------------------------------------------------------- */

/* Entries wholly visible below the header slot. */
static uint16_t vis(void)
{
    uint16_t n = rect_h / slot_e;
    n = n >= 2 ? n - 1 : 1;
    return n > MAX_VIS ? MAX_VIS : n;
}

/* Keeps the selection in the list and visible, and the screen filled. */
static void lp_keep(listpos *p, uint16_t total)
{
    uint16_t v = vis();
    if (!total) {
        p->sel = p->top = 0;
        return;
    }
    if (p->sel >= total)
        p->sel = total - 1;
    if (p->sel < p->top)
        p->top = p->sel;
    if (p->sel >= p->top + v)
        p->top = p->sel - v + 1;
    if (p->top + v > total)
        p->top = total > v ? total - v : 0;
}

static void lp_set(listpos *p, uint16_t total, int32_t sel, int32_t top)
{
    listpos old = *p;
    p->sel = sel < 0 ? 0 : sel >= total ? (total ? total - 1 : 0) : sel;
    p->top = top < 0 ? 0 : top > 0xFFFF ? 0xFFFF : top;
    lp_keep(p, total);
    if (old.sel != p->sel || old.top != p->top)
        page_dirty = 1;
}

/* Up, Down, PgUp, PgDn (the selection keeps its place on the screen unless
 * the list cannot scroll further, when it moves instead), Home, End. */
static uint8_t lp_key(listpos *p, uint16_t total, uint8_t code)
{
    int32_t s = p->sel, t = p->top, v = vis(), max = total > v ? total - v : 0, d, nt;
    switch (code) {
    case KEY_UP: s--; break;
    case KEY_DOWN: s++; break;
    case KEY_PAGE_UP:
    case KEY_PAGE_DOWN:
        d = code == KEY_PAGE_UP ? -v : v;
        nt = t + d < 0 ? 0 : t + d > max ? max : t + d;
        if (nt == t) {
            s += d;
        } else {
            s += nt - t;
            t = nt;
        }
        break;
    case KEY_HOME: s = 0; break;
    case KEY_END: s = total; break;
    default: return 0;
    }
    lp_set(p, total, s, t);
    return 1;
}

/* The wheel scrolls three entries a step; the selection stays on screen. */
static void lp_wheel(listpos *p, uint16_t total, int8_t steps)
{
    int32_t v = vis(), max = total > v ? total - v : 0, t = p->top + steps * 3, s = p->sel;
    t = t < 0 ? 0 : t > max ? max : t;
    if (s < t)
        s = t;
    if (s > t + v - 1)
        s = t + v - 1;
    lp_set(p, total, s, t);
}

static uint16_t s_rows(void);

/* A height event in a list view: the slot height, if it is a list's. */
static void list_height(uint32_t h)
{
    uint16_t e, before;
    if (!last_slots || h % last_slots)
        return;
    e = h / last_slots;
    if (e < E_MIN || e > E_MAX || e == slot_e)
        return;
    before = vis();
    slot_e = e;
    if (vis() != before)
        page_dirty = 1;
    lp_keep(&tlp, tl_total);
    lp_keep(&fdp, subs_n);
    lp_keep(&srp, s_rows());
}

/* --- Timeline ------------------------------------------------------------ */

static uint8_t in_window(uint16_t i)
{
    return i >= tl_base && i - tl_base < tl_count;
}

static uint8_t *record(uint16_t i)
{
    return tl_buf + rec_at[i - tl_base];
}

static void record_id(const uint8_t *r, uint8_t *id)
{
    memcpy(id, r + R_FEED, 36); /* the feed key and seq */
}

static void load_timeline(uint16_t first)
{
    tl_req_base = first;
    tl_handle = content_feed(&tl_req, CONTENT_TIMELINE, first, TL_BUF_LEN, tl_buf);
}

static void count_request(void)
{
    cnt_handle = content_feed(&cnt_req, CONTENT_TIMELINE, cnt_pos, TL_BUF_LEN, cnt_buf);
}

/* Counts each feed's entries in the timeline again. */
static void start_count(void)
{
    if (cnt_handle) {
        cnt_again = 1; /* when this one is in: its buffer is in use */
        return;
    }
    memset(counts, 0, sizeof counts);
    cnt_pos = 0;
    cnt_done = 0;
    count_request();
}

static void load_subscriptions(void)
{
    if (sub_handle) {
        sub_again = 1;
        return;
    }
    sub_handle = content_feed(&sub_req, CONTENT_SUBSCRIPTIONS, 0, sizeof sub_buf, sub_buf);
}

static void load_names(void)
{
    if (nm_handle) {
        nm_again = 1;
        return;
    }
    nm_handle = content_names(&nm_req, 0, NAMES_LEN, nm_buf);
}

static void names_done(uint8_t status)
{
    uint32_t at = 0, n = 0;
    nm_handle = 0;
    if (status == STATUS_DONE) {
        while (at < nm_req.length && n < NM_MAX) {
            nm_at[n++] = at;
            at += 1 + nm_buf[at];
        }
        nm_n = n;
        page_dirty = 1;
    }
    if (nm_again) {
        nm_again = 0;
        load_names();
    }
}

/* The subscriptions, their names and the timeline again (after a
 * change). */
static void reload(void)
{
    load_subscriptions();
    load_names();
    if (tl_handle) {
        tl_again = 1; /* reload once this load is in */
        return;
    }
    tl_refresh = 1;
    load_timeline(tl_base);
}

static void timeline_done(uint8_t status)
{
    uint32_t at = 0, n = 0, total, i;
    uint16_t old_total = tl_total;
    uint8_t id[36];
    tl_handle = 0;
    if (status == STATUS_DONE) {
        while (at < tl_req.length && n < MAX_RECORDS) {
            rec_at[n++] = at;
            at += 1 + tl_buf[at];
        }
        total = tl_req.size;
        tl_base = tl_req_base;
        tl_count = n;
        tl_total = total > 0xFFFF ? 0xFFFF : total;
        if (tl_refresh && tl_loaded) {
            /* New entries arrive at index 0 (SPEC §content.feeds): the selection keeps
             * its entry, found by its identity or else moved down by the
             * arrivals. */
            if (tl_total > old_total)
                tlp.sel += tl_total - old_total;
            for (i = 0; sel_id_ok && i < tl_count; i++) {
                record_id(tl_buf + rec_at[i], id);
                if (memcmp(id, sel_id, 36) == 0) {
                    tlp.sel = tl_base + i;
                    break;
                }
            }
        }
        tl_refresh = 0;
        tl_loaded = 1;
        lp_keep(&tlp, tl_total);
        page_dirty = 1;
    }
    if (tl_again) {
        tl_again = 0;
        tl_refresh = 1;
        load_timeline(tl_base);
    }
}

/* Loads the part of the timeline on screen if the window does not hold it. */
static void ensure_window(void)
{
    uint16_t last = tlp.top + vis();
    if (tl_handle || !tl_loaded || tl_total == 0)
        return;
    if (last >= tl_total)
        last = tl_total - 1;
    if (!in_window(tlp.top) || !in_window(last))
        load_timeline(tlp.top > 32 ? tlp.top - 32 : 0);
}

/* The subscription record of `key`, or -1 (records are in key order). */
static int32_t find_sub(const uint8_t *key)
{
    int32_t lo = 0, hi = (int32_t)subs_n - 1, mid, c;
    while (lo <= hi) {
        mid = (lo + hi) >> 1;
        c = memcmp(sub_buf + mid * 36, key, 32);
        if (!c)
            return mid;
        if (c < 0)
            lo = mid + 1;
        else
            hi = mid - 1;
    }
    return -1;
}

static int32_t own_index(void)
{
    return own_known ? find_sub(own_key) : -1;
}

static void subs_done(uint8_t status)
{
    sub_handle = 0;
    if (status == STATUS_DONE) {
        subs_n = sub_req.length / 36;
        subs_total = sub_req.size;
        subs_loaded = 1;
        lp_keep(&fdp, subs_n);
        if (view == FEEDS)
            start_count();
        page_dirty = 1;
    }
    if (sub_again) {
        sub_again = 0;
        load_subscriptions();
    }
}

static void count_done(uint8_t status)
{
    uint32_t at = 0, n = 0;
    int32_t k;
    cnt_handle = 0;
    if (cnt_again) {
        cnt_again = 0;
        start_count();
        return;
    }
    if (status == STATUS_DONE) {
        while (at < cnt_req.length) {
            k = find_sub(cnt_buf + at + R_FEED);
            if (k >= 0)
                counts[k]++;
            n++;
            at += 1 + cnt_buf[at];
        }
        cnt_pos += n;
        if (n && cnt_pos < cnt_req.size) {
            count_request();
            return;
        }
    }
    cnt_done = 1;
    if (view == FEEDS)
        page_dirty = 1;
}

/* --- Documents in the lists ---------------------------------------------- */

/* The cache slot of blob `id`'s format name, or -1. */
static int32_t dn_find(const uint8_t *id)
{
    int32_t k;
    for (k = 0; k < DN_N; k++)
        if (dn_state[k] && !memcmp(dn_id[k], id, 32))
            return k;
    return -1;
}

/* Asks for the head of record r's blob if it is a document in the
 * host's store whose format name is not known; 1 if it did. */
static uint8_t dn_want(const uint8_t *r)
{
    if (r[R_KIND] != ENTRY_DOCUMENT || !(r[R_FLAGS] & TIMELINE_LOCAL) || dn_find(r + R_BLOB) >= 0)
        return 0;
    dn_slot = dn_next;
    dn_next = (dn_next + 1) % DN_N;
    dn_state[dn_slot] = 1;
    memcpy(dn_id[dn_slot], r + R_BLOB, 32);
    dn_handle = content_get(&dn_req, dn_id[dn_slot], 0, sizeof dn_buf, dn_buf);
    return 1;
}

static void dn_done(uint8_t status)
{
    uint8_t n = dn_buf[TDOC_HEAD - 1];
    dn_handle = 0;
    dn_state[dn_slot] = 2;
    dn_len[dn_slot] = 0;
    if (status == STATUS_DONE && dn_req.length >= TDOC_HEAD + n && !memcmp(dn_buf, "TDOC", 4) && !dn_buf[4]
        && valid_name(dn_buf + TDOC_HEAD, n)) {
        memcpy(dn_name[dn_slot], dn_buf + TDOC_HEAD, n);
        dn_len[dn_slot] = n;
    }
    if (view == TIMELINE || view == SEARCH)
        page_dirty = 1;
}

/* An entry's title in a list, a document's with a mark before it. */
static void entry_title(const uint8_t *r)
{
    if (r[R_KIND] == ENTRY_DOCUMENT)
        doc_s("\xE2\x97\x86 ");
    doc_title(r + R_TITLE, r[R_N]);
}

/* A document's format name ("sketch document  ·  ", or "document  ·  "
 * while it is not known) at the start of its details line. */
static void tb_kind(const uint8_t *r)
{
    int32_t k;
    if (r[R_KIND] != ENTRY_DOCUMENT)
        return;
    k = dn_find(r + R_BLOB);
    if (k >= 0 && dn_state[k] == 2 && dn_len[k]) {
        tb_name(dn_name[k], dn_len[k]);
        tb_s(" ");
    }
    tb_s("document  \xC2\xB7  ");
}

/* The header slot: a title and a line of details. */
static void header_slot(const char *title)
{
    slot_begin(rgb_head, DOC_BOLD, 0);
    doc_s(title);
    slot_meta(rgb_meta);
    doc_text(tb, tb_n);
}

/* The friendly page for an empty timeline. */
static void empty_doc(void)
{
    ldoc_begin();
    doc_paragraph(DOC_START, 0, 48, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(TITLE_SIZE);
    doc_rgb(rgb_title);
    doc_s("No articles yet");
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, 0);
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    doc_s("The timeline shows the articles of the feeds this host subscribes to, newest first. "
          "New articles appear here as they arrive.");
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_s("Press F to see your feeds and subscribe to one by its key.");
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_s("Press T to publish a test article of your own; it shows up here.");
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_s("Press M to read the Tersenet manual.");
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_s("Press ? for help, Esc to quit.");
    column_submit();
}

static void timeline_doc(void)
{
    uint16_t i, end, slots = 1;
    uint8_t *r, k, reply, s;
    if (!tl_loaded)
        return;
    if (!tl_total) {
        empty_doc();
        return;
    }
    ldoc_begin();
    tb_n = 0;
    tb_u(tl_total);
    tb_s(" article");
    tb_s(plural(tl_total));
    tb_s("  \xC2\xB7  ");
    tb_u(subs_total);
    tb_s(" feed");
    tb_s(plural(subs_total));
    header_slot("Timeline");
    end = tlp.top + vis() + 1;
    if (end > tl_total)
        end = tl_total;
    for (i = tlp.top; i < end; i++, slots++) {
        s = i == tlp.sel;
        if (!in_window(i)) {
            slot_begin(rgb_meta, 0, 0);
            doc_s("\xE2\x80\xA6");
            slot_meta(rgb_meta);
            doc_s("loading");
            continue;
        }
        r = record(i);
        if (s) {
            record_id(r, sel_id);
            sel_id_ok = 1;
        }
        slot_begin(s ? rgb_sel : rgb_entry, 0, i + 1);
        entry_title(r);
        slot_meta(s ? rgb_sel_meta : rgb_meta);
        tb_n = 0;
        tb_kind(r);
        tb_feed(r + R_FEED);
        tb_s("  \xC2\xB7  ");
        tb_date(get32(r + R_TIME));
        tb_s("  \xC2\xB7  ");
        tb_size(get24(r + R_SIZE));
        for (k = reply = 0; k < 32; k++)
            reply |= r[R_REPLY + k];
        if (reply)
            tb_s("  \xC2\xB7  \xE2\x86\xB3 reply");
        if (!(r[R_FLAGS] & TIMELINE_LOCAL))
            tb_s("  \xC2\xB7  not downloaded");
        if (beyond_full(r + R_TITLE, r[R_N]))
            tb_s("  \xC2\xB7  may need a font"); /* ARTICLE.md 1.1 */
        doc_text(tb, tb_n);
        slot_end(i + 1);
    }
    list_submit(slots);
    dn_scan = 1;
    dirty[STATUS_ROW] = 1; /* the position */
}

static void feeds_doc(void)
{
    uint16_t i, end, slots = 1;
    int32_t own = own_index();
    const uint8_t *r, *name;
    uint32_t seq;
    uint8_t s, n, nick;
    ldoc_begin();
    tb_n = 0;
    tb_u(subs_total);
    tb_s(" subscription");
    tb_s(plural(subs_total));
    if (own_known) {
        tb_s("  \xC2\xB7  your key ");
        tb_key(own_key);
    } else {
        tb_s("  \xC2\xB7  no key of your own yet");
    }
    header_slot("Feeds");
    if (!subs_n) {
        slot_begin(rgb_entry, 0, 0);
        doc_s("No subscriptions yet");
        slot_meta(rgb_meta);
        doc_s("Press S to subscribe to a feed by its key or address, or T to publish a test article");
        slots++;
    }
    end = fdp.top + vis() + 1;
    if (end > subs_n)
        end = subs_n;
    for (i = fdp.top; i < end; i++, slots++) {
        r = sub_buf + i * 36;
        s = i == fdp.sel;
        slot_begin(s ? rgb_sel : rgb_entry, 0, i + 1);
        tb_n = 0;
        name = name_of(r, &n, &nick);
        if (n) {
            tb_name(name, n);
            doc_text(tb, tb_n);
            doc_rgb(s ? rgb_sel_meta : rgb_meta);
            tb_n = 0;
            tb_s("  ");
        }
        tb_key(r);
        doc_text(tb, tb_n);
        if (i == own) {
            doc_rgb(rgb_you);
            doc_s("  you");
        }
        slot_meta(s ? rgb_sel_meta : rgb_meta);
        tb_n = 0;
        if (nick)
            tb_s("your nickname  \xC2\xB7  ");
        if (name_flags & NAME_HINTED)
            tb_s("@ through a relay its address names  \xC2\xB7  ");
        seq = get32(r + 32);
        if (seq) {
            tb_s("highest #");
            tb_u(seq);
        } else {
            tb_s("no entries yet");
        }
        tb_s("  \xC2\xB7  ");
        if (cnt_done) {
            tb_u(counts[i]);
            tb_s(" article");
            tb_s(plural(counts[i]));
            tb_s(" held");
        } else {
            tb_s("counting\xE2\x80\xA6");
        }
        doc_text(tb, tb_n);
        slot_end(i + 1);
    }
    list_submit(slots);
}

/* --- Search ------------------------------------------------------------ */

/* The rows of the search list: the heading of the articles on this host
 * and its results, the feeds' heading and the feeds, the heading of the
 * relays' articles and those articles. */
static uint16_t s_hf(void)
{
    return ls_n + 1;
}

static uint16_t s_ha(void)
{
    return ls_n + sf_n + 2;
}

static uint16_t s_rows(void)
{
    return 3 + ls_n + sf_n + sa_n;
}

/* Whether row r is a section's heading. */
static uint8_t s_heading(uint16_t r)
{
    return r == 0 || r == s_hf() || r == s_ha();
}

/* The section of row r: 0 this host's articles, 1 the feeds, 2 the
 * relays' articles. */
static uint8_t s_section(uint16_t r)
{
    return r < s_hf() ? 0 : r < s_ha() ? 1 : 2;
}

/* The result record of row r (not a heading). */
static const uint8_t *s_record(uint16_t r)
{
    switch (s_section(r)) {
    case 0: return ls_res[r - 1].rec;
    case 1: return sf_buf + sf_at[r - s_hf() - 1];
    default: return sa_buf + sa_at[r - s_ha() - 1];
    }
}

/* The first result row, or 0 if there is none. */
static uint16_t s_first(void)
{
    return ls_n ? 1 : sf_n ? s_hf() + 1 : sa_n ? s_ha() + 1 : 0;
}

/* After a move in direction dir (-1 or 1), the selection is moved off a
 * heading. */
static void s_fix(int8_t dir)
{
    int32_t total = s_rows(), sel = srp.sel, k;
    if (!s_heading(sel))
        return;
    /* The nearest result that way, else the other way, else none. */
    for (k = sel; k >= 0 && k < total && s_heading(k); k += dir)
        ;
    if (k < 0 || k >= total)
        for (k = sel; k >= 0 && k < total && s_heading(k); k -= dir)
            ;
    if (k < 0 || k >= total)
        k = 0;
    lp_set(&srp, total, k, k == 1 ? 0 : srp.top);
}

/* A search's results: where each record starts. */
static uint8_t find_parse(const uint8_t *buf, const content_req *r, uint16_t *at)
{
    uint32_t a = 0, n = 0;
    while (a < r->length && n < FIND_MAX) {
        at[n++] = a;
        a += 1 + buf[a];
    }
    return n;
}

/* --- Search: the articles on this host ------------------------------------
 *
 * The timeline is read LS_PAGE bytes at a time, newest first, and each
 * entry becomes a job: its title is searched (a match there is a result at
 * once), then, if it is an article in the host's store, its text: the
 * first 48 KiB (ART_CAP, what the reader shows), got in chunks of up to
 * 16 KiB with ranged gets into the arena, each scanned as it comes in and
 * then dropped. A match across two chunks is found because the last
 * LS_PAD bytes of a chunk (the query is at most 48) are put before the
 * next. Several chunks are asked for at once, as long as the reader then
 * has at most LS_PENDING requests pending (two of the eight stay free),
 * and they are scanned in the order asked, so results come newest first.
 * A job ends at its text's first match, which gives the excerpt. Articles
 * not in the store are searched by title only (never fetched), and so are
 * documents; they are counted.
 *
 * Matching is the relays' (NETWORK 4.7): the query's bytes in a row, A-Z
 * and a-z the same, every other byte only itself. The scan is Horspool's
 * over folded bytes (a one-byte query: find2 over both cases), metered
 * like building an article: each frame has LS_UNITS of estimated work
 * (fewer when the list was built again in it, less the content delivered
 * to it), and a stretch of text is scanned only if its worst case fits;
 * what was spent is then counted from the windows and comparisons made. */

static uint8_t pending(void);
static uint8_t done_for(uint8_t h, const content_req *r);
static uint8_t not_text(const uint8_t *p, uint32_t n);
static uint32_t frame_n; /* frames run */

#define LS_UNITS 88000      /* a frame's work */
#define LS_UNITS_BUSY 56000 /* in a frame that also built the list */
#define LS_EVERY 12         /* frames between redraws for progress only */
#define W_IT 8              /* a Horspool window (7 instructions) */
#define W_CAND 20           /* a comparison of a window with the query */
#define W_CMP 11            /* and each byte compared */
#define W_ONE 5             /* a byte of a one-byte query's scan */
#define W_LSCALL 80         /* a scan of a stretch */
#define W_LJOB 700          /* taking an entry and starting on it */
#define W_LSPAN 900         /* asking for a chunk, taking it and its carry */
#define W_LRES 2500         /* a result and its excerpt */
#define W_LMIN 4000         /* less work than this left: the rest waits */

static uint32_t ls_work;
static uint8_t ls_c; /* the carry before the head chunk */

static void ls_spend(uint32_t u)
{
    ls_work = ls_work > u ? ls_work - u : 0;
}

static void ls_set_skip(uint8_t c, uint8_t v)
{
    ls_skip[c] = v;
    if ((uint8_t)(c - 'a') < 26)
        ls_skip[c - 0x20] = v;
}

/* The query's tables (ls_nd: the query folded). */
static void ls_tables(void)
{
    uint32_t i, m = s_qlen;
    for (i = 0; i < 256; i++) {
        ls_fold[i] = (uint8_t)(i - 'A') < 26 ? i | 0x20 : i;
        ls_skip[i] = m;
    }
    ls_m = m;
    for (i = 0; i < m; i++)
        ls_nd[i] = ls_fold[s_query[i]];
    for (i = 0; i + 1 < m; i++)
        ls_set_skip(ls_nd[i], m - 1 - i);
    ls_shift = ls_skip[ls_nd[m - 1]];
    ls_set_skip(ls_nd[m - 1], 0);
}

/* The first match of the query that ends in [*pp, lim) (*pp: the end of
 * the first window to look at, which starts ls_m - 1 bytes before it):
 * its first byte, or 0; *pp is where the scan goes on. The work is spent. */
static const uint8_t *ls_find(const uint8_t **pp, const uint8_t *lim)
{
    const uint8_t *p = *pp, *w = 0, *sk = ls_skip, *fd = ls_fold, *nd = ls_nd;
    uint32_t its = 0, cands = 0, m1 = ls_m - 1, k, s;
    if (!m1) {
        uint8_t a = nd[0], b = (uint8_t)(a - 'a') < 26 ? a - 0x20 : a;
        if (p < lim) {
            w = find2((uint8_t *)p, lim, a, b);
            ls_spend((w - p + 1) * W_ONE + W_LSCALL);
            if (w < lim) {
                *pp = w + 1;
                return w;
            }
        }
        *pp = lim;
        return 0;
    }
    /* The hot loop: 7 instructions a window (load, table, add, count, two
     * branches). */
    while (p < lim) {
        do {
            s = sk[*p];
            p += s;
            its++;
        } while (s && p < lim);
        if (s)
            break;
        cands++;
        w = p - m1;
        for (k = 0; k < m1 && fd[w[k]] == nd[k]; k++)
            ;
        if (k == m1)
            break;
        w = 0;
        p += ls_shift;
    }
    ls_its += its;
    ls_cands += cands;
    ls_spend(its * W_IT + cands * (W_CAND + m1 * W_CMP) + W_LSCALL);
    *pp = p;
    return w;
}

/* Stops the search (st: LS_DONE, LS_STOPPED or LS_FULL). Its requests
 * still pending are taken in as they come; a new search waits for them. */
static void ls_halt(uint8_t st)
{
    if (ls_state != LS_RUN)
        return;
    ls_state = st;
    if (view == SEARCH)
        page_dirty = 1;
}

/* A new result for job j (its title, so far); 0 if there are LS_MAX
 * already (the search stops). Rows after this host's results move down
 * one, the selection and the top with them. */
static uint8_t ls_add(ls_job *j)
{
    ls_result *x;
    uint16_t hf = s_hf();
    if (ls_n == LS_MAX) {
        ls_halt(LS_FULL);
        return 0;
    }
    x = &ls_res[ls_n];
    memcpy(x->rec, j->rec, j->rec[0] + 1);
    x->at = LS_NONE;
    x->ex_n = x->t_len = 0;
    ls_n++;
    j->res = ls_n;
    if (srp.sel >= hf)
        srp.sel++;
    if (srp.top && srp.top >= hf)
        srp.top++;
    if (s_heading(srp.sel))
        srp.sel = s_first();
    lp_keep(&srp, s_rows());
    if (view == SEARCH && hf >= srp.top && hf <= srp.top + vis())
        page_dirty = 1; /* on the screen: at once */
    else
        ls_stale = 1;
    ls_spend(W_LRES);
    return 1;
}

/* The excerpt of result x: the text around the match at q, from the text
 * lo .. hi, which is the article's from offset lo_off; at_end: hi is the
 * end of the article. At most LS_BEFORE bytes before the match, LS_EX in
 * all, not cut inside a character; control characters (line breaks, tabs)
 * as spaces, runs of spaces as one. */
static void ls_excerpt(ls_result *x, const uint8_t *q, const uint8_t *lo, const uint8_t *hi, uint32_t lo_off, uint8_t at_end)
{
    const uint8_t *s = q - lo > LS_BEFORE ? q - LS_BEFORE : lo, *e, *k, *qe = q + ls_m;
    uint8_t n = 0, sp = 1, c;
    e = hi - s > LS_EX ? s + LS_EX : hi;
    while (s < q && (*s & 0xC0) == 0x80)
        s++;
    /* Cut before the match: from the start of a word, if one starts there. */
    if (s > lo) {
        for (k = s; k < q && *k > ' '; k++)
            ;
        if (k < q)
            s = k + 1;
    }
    /* The last character whole: else it is left out. */
    for (k = e; k > qe && e - k < 3 && (k[-1] & 0xC0) == 0x80; k--)
        ;
    if (k > qe && k[-1] >= 0xC0 && e - (k - 1) < (k[-1] >= 0xF0 ? 4 : k[-1] >= 0xE0 ? 3 : 2))
        e = k - 1;
    x->ex_pre = lo_off + (s - lo) > 0;
    x->ex_post = e < hi || !at_end;
    x->ex_at = x->ex_len = 0;
    for (; s < e; s++) {
        if (s == q)
            x->ex_at = n;
        if (s == qe)
            x->ex_len = n - x->ex_at;
        c = *s;
        if (c < 0x20 || c == 0x7F)
            c = ' ';
        if (c == ' ' && sp)
            continue;
        sp = c == ' ';
        x->ex[n++] = c;
    }
    if (qe >= e)
        x->ex_len = n - x->ex_at;
    while (n > x->ex_at + x->ex_len && x->ex[n - 1] == ' ')
        n--;
    x->ex_n = n;
}

/* The title of the job at the head. */
static void ls_title(ls_job *j)
{
    const uint8_t *t = j->rec + R_TITLE, *p = t + ls_m - 1, *w;
    uint8_t n = j->rec[R_N];
    if (n < ls_m || !(w = ls_find(&p, t + n)) || !ls_add(j))
        return;
    ls_res[j->res - 1].t_at = w - t;
    ls_res[j->res - 1].t_len = ls_m;
}

/* Room for `need` bytes in the arena, or LS_NONE: after the newest chunk,
 * else at the start, before the oldest. */
static uint32_t ls_alloc(uint32_t need)
{
    uint32_t tail;
    if (!ls_sn)
        return 0;
    tail = ls_sp[ls_s0].pos;
    if (ls_head > tail)
        return LS_ARENA - ls_head >= need ? ls_head : need <= tail ? 0 : LS_NONE;
    return tail - ls_head >= need ? ls_head : LS_NONE;
}

/* Takes timeline records into jobs, and asks for the next page when the
 * one in hand is taken. */
static void ls_fill(void)
{
    ls_job *j;
    const uint8_t *r;
    uint32_t want;
    while (ls_jn < LS_JOBS && ls_tl_i < ls_tl_n) {
        r = ls_tl_buf + ls_tl_at[ls_tl_i++];
        j = &ls_jobs[(ls_j0 + ls_jn) % LS_JOBS];
        memcpy(j->rec, r, r[0] + 1);
        want = 0;
        if (r[R_KIND] == ENTRY_ARTICLE && (r[R_FLAGS] & TIMELINE_LOCAL)) {
            want = get24(r + R_SIZE);
            if (want > ART_CAP)
                want = ART_CAP;
        }
        j->want = want;
        j->issued = j->scanned = 0;
        j->res = 0;
        j->stage = J_TITLE;
        ls_jn++;
        ls_spend(W_LJOB);
    }
    if (ls_tl_i == ls_tl_n && !ls_tl_handle && (!ls_tl_known || ls_tl_pos < ls_tl_total) && pending() < LS_PENDING)
        ls_tl_handle = content_feed(&ls_tl_req, CONTENT_TIMELINE, ls_tl_pos, LS_PAGE, ls_tl_buf);
}

/* A page of the timeline is in. Entries that arrived since the search
 * began are at its start (SPEC §content.feeds): the page then begins with entries
 * already taken, which are passed over (and the new ones not searched). */
static void ls_tl_done(uint8_t st)
{
    uint32_t at = 0, n = 0, d = 0, total;
    ls_tl_handle = 0;
    if (ls_state != LS_RUN || ls_wait)
        return;
    if (st != STATUS_DONE) {
        ls_tl_known = 1;
        ls_tl_total = ls_tl_pos; /* no more */
        return;
    }
    ls_delivered += ls_tl_req.length;
    total = ls_tl_req.size;
    if (ls_tl_known && total > ls_tl_total)
        d = total - ls_tl_total;
    while (at < ls_tl_req.length && n < sizeof ls_tl_at / sizeof ls_tl_at[0]) {
        ls_tl_at[n++] = at;
        at += 1 + ls_tl_buf[at];
    }
    if (!ls_tl_known)
        ls_total = total;
    ls_tl_known = 1;
    ls_tl_total = total;
    ls_tl_n = n;
    ls_tl_i = d < n ? d : n;
    ls_tl_pos = n ? ls_tl_pos + n : total;
    ls_stale = 1;
}

/* Asks for the chunks of the jobs in order, while the reader has fewer
 * than LS_PENDING requests pending and the arena has room. */
static void ls_issue(void)
{
    ls_job *j;
    ls_span *sp;
    uint32_t n, need, pos;
    uint8_t slot, h;
    while (ls_ji < ls_jn) {
        slot = (ls_j0 + ls_ji) % LS_JOBS;
        j = &ls_jobs[slot];
        if (j->stage == J_DONE || j->issued >= j->want) {
            ls_ji++;
            continue;
        }
        if (ls_sn == LS_SPANS || pending() >= LS_PENDING)
            return;
        n = j->want - j->issued;
        if (n > CHUNK)
            n = CHUNK;
        need = (LS_PAD + n + 3) & ~3u;
        pos = ls_alloc(need);
        if (pos == LS_NONE)
            return;
        sp = &ls_sp[(ls_s0 + ls_sn) % LS_SPANS];
        h = content_get(&sp->req, j->rec + R_BLOB, j->issued, n, ls_arena + pos + LS_PAD);
        if (!h)
            return;
        sp->handle = h;
        sp->pos = pos;
        sp->off = j->issued;
        sp->len = n;
        sp->job = slot;
        sp->in = 0;
        ls_head = pos + need;
        ls_sn++;
        ls_inflight++;
        j->issued += n;
        ls_spend(W_LSPAN);
    }
}

/* A content-done event for the search (1 if it was one). */
static uint8_t ls_event(void)
{
    uint8_t k;
    ls_span *sp;
    if (done_for(ls_tl_handle, &ls_tl_req)) {
        ls_tl_done(ev.b);
        return 1;
    }
    for (k = 0; k < LS_SPANS; k++) {
        sp = &ls_sp[k];
        if (!sp->in && done_for(sp->handle, &sp->req)) {
            sp->in = 1;
            sp->handle = 0;
            ls_inflight--;
            if (sp->req.status == STATUS_DONE)
                ls_delivered += sp->req.length;
            return 1;
        }
    }
    return 0;
}

/* Scans the head chunk (in) of job j as far as this frame's work goes;
 * 1 when it is done with (j is J_DONE if it is the last one, or a match
 * was found), 0 if the work is used. */
static uint8_t ls_scan(ls_job *j, ls_span *sp)
{
    uint8_t *data = ls_arena + sp->pos + LS_PAD;
    uint32_t got = sp->req.length, per = W_IT + W_CAND + (ls_m - 1) * W_CMP + W_ONE, c, n;
    const uint8_t *p, *w, *lim, *end;
    ls_result *x;
    if (!ls_started) {
        ls_started = 1;
        ls_spend(W_LSPAN);
        if (sp->req.status != STATUS_DONE) {
            if (!sp->off)
                ls_skipped++; /* not got after all */
            j->stage = J_DONE;
            return 1;
        }
        /* A program image or a document is not an article. */
        if (!sp->off && not_text(data, got)) {
            j->stage = J_DONE;
            return 1;
        }
        c = sp->off ? ls_carry_n : 0;
        memcpy(data - c, ls_carry + LS_PAD - c, c);
        ls_c = c;
        ls_p = data - (c < ls_m - 1u ? c : ls_m - 1u) + ls_m - 1 - ls_arena;
    }
    c = ls_c;
    end = data + got;
    p = ls_arena + ls_p;
    while (p < end) {
        if (ls_work < W_LMIN)
            return 0;
        n = (ls_work - W_LMIN / 2) / per;
        lim = (uint32_t)(end - p) > n ? p + n : end;
        w = ls_find(&p, lim);
        ls_p = p - ls_arena;
        if (w) {
            ls_bytes += w - data + ls_m;
            j->stage = J_DONE;
            if (!j->res && !ls_add(j))
                return 1;
            x = &ls_res[j->res - 1];
            x->at = sp->off + (w - data);
            ls_excerpt(x, w, data - c, end, sp->off - c, sp->off + got >= get24(j->rec + R_SIZE));
            return 1;
        }
    }
    ls_bytes += got;
    j->scanned += got;
    /* The end of this chunk goes before the next. */
    n = got + c < LS_PAD ? got + c : LS_PAD;
    memcpy(ls_carry + LS_PAD - n, end - n, n);
    ls_carry_n = n;
    if (got < sp->len || j->scanned >= j->want)
        j->stage = J_DONE;
    return 1;
}

static void ls_pop_span(void)
{
    ls_s0 = (ls_s0 + 1) % LS_SPANS;
    ls_sn--;
    ls_started = 0;
}

/* Starts a search of the articles on this host for s_query. */
static void ls_start(void)
{
    ls_tables();
    ls_n = 0;
    ls_searched = ls_skipped = ls_total = ls_bytes = ls_its = ls_cands = 0;
    ls_state = LS_RUN;
    ls_wait = 1; /* everything else is set once no request of a search before is pending */
}

/* This frame's share of the search; `busy`: the list was built in this
 * frame. */
static void ls_step(uint8_t busy)
{
    ls_job *j;
    uint32_t d = ls_delivered / 4 + 1, units = busy ? LS_UNITS_BUSY : LS_UNITS;
    ls_delivered = 0;
    if (ls_state != LS_RUN)
        return;
    if (ls_wait) {
        if (ls_inflight || ls_tl_handle)
            return;
        ls_wait = 0;
        ls_j0 = ls_jn = ls_ji = ls_s0 = ls_sn = 0;
        ls_head = 0;
        ls_tl_n = ls_tl_i = 0;
        ls_tl_pos = ls_tl_total = ls_tl_known = 0;
    }
    ls_work = units > d ? units - d : 0;
    for (;;) {
        ls_fill();
        ls_issue();
        if (ls_state != LS_RUN || ls_work < W_LMIN)
            break;
        if (!ls_jn) {
            if (ls_tl_known && ls_tl_i == ls_tl_n && ls_tl_pos >= ls_tl_total && !ls_tl_handle)
                ls_halt(LS_DONE);
            break;
        }
        j = &ls_jobs[ls_j0];
        if (j->stage == J_TITLE) {
            if (j->rec[R_KIND] == ENTRY_ARTICLE && !(j->rec[R_FLAGS] & TIMELINE_LOCAL))
                ls_skipped++;
            j->stage = j->want ? J_BODY : J_DONE;
            ls_title(j);
            continue;
        }
        if (j->stage == J_BODY) {
            /* Its chunks are the first asked for: the head one, once it is
             * in. */
            if (!ls_sn || !ls_sp[ls_s0].in || !ls_scan(j, &ls_sp[ls_s0]))
                break;
            ls_pop_span();
            continue;
        }
        /* Done: its chunks still to come are dropped as they come in. */
        if (ls_sn && ls_sp[ls_s0].job == ls_j0) {
            if (!ls_sp[ls_s0].in)
                break;
            ls_pop_span();
            continue;
        }
        ls_j0 = (ls_j0 + 1) % LS_JOBS;
        ls_jn--;
        if (ls_ji)
            ls_ji--;
        ls_searched++;
        ls_stale = 1;
    }
}

/* --- Search: the list ------------------------------------------------------ */

/* A number with commas between thousands: "1,204". */
static void tb_uc(uint32_t v)
{
    if (v >= 1000) {
        tb_uc(v / 1000);
        tb_s(",");
        tb_s(u32_str(v % 1000 + 1000) + 1);
    } else {
        tb_u(v);
    }
}

/* "340 of 1,204 articles" */
static void tb_of(void)
{
    uint32_t done = ls_searched > ls_total ? ls_total : ls_searched;
    tb_uc(done);
    tb_s(" of ");
    tb_uc(ls_total);
    tb_s(ls_total == 1 ? " article" : " articles");
}

/* The line under the heading of this host's articles. */
static void ls_section_meta(void)
{
    tb_n = 0;
    if (ls_state == LS_RUN && !ls_tl_known) {
        tb_s("searching\xE2\x80\xA6");
        return;
    }
    switch (ls_state) {
    case LS_RUN:
        tb_s("searched ");
        tb_of();
        break;
    case LS_DONE:
        if (!ls_total) {
            tb_s("no articles on this host");
            return;
        }
        if (ls_n) {
            tb_u(ls_n);
            tb_s(" found in ");
        } else {
            tb_s("none found in ");
        }
        tb_uc(ls_total);
        tb_s(ls_total == 1 ? " article" : " articles");
        break;
    case LS_FULL:
        tb_s("more than 64 matches; refine the search  \xC2\xB7  searched ");
        tb_of();
        break;
    default:
        tb_s("stopped after ");
        tb_of();
    }
    if (ls_skipped) {
        tb_s("  \xC2\xB7  ");
        tb_uc(ls_skipped);
        tb_s(" skipped (not downloaded)");
    }
}

/* Title text without control characters or line and paragraph
 * separators (U+2028, U+2029), which become spaces. */
static void title_text(const uint8_t *t, uint8_t n)
{
    uint8_t buf[120], k = 0, i, c;
    for (i = 0; i < n; i++) {
        c = t[i];
        if (c < 0x20 || c == 0x7F) {
            c = ' ';
        } else if (c == 0xE2 && i + 2 < n && t[i + 1] == 0x80 && (t[i + 2] == 0xA8 || t[i + 2] == 0xA9)) {
            c = ' ';
            i += 2;
        }
        buf[k++] = c;
    }
    doc_text(buf, k);
}

/* A result from this host: the title (the match in it in the match
 * colour, unless selected) with the feed and date after it, smaller; then
 * the excerpt, the match in the match colour, or where it matched. */
static void ls_slot(uint16_t i, uint8_t s)
{
    const ls_result *x = &ls_res[i - 1];
    const uint8_t *r = x->rec, *t = r + R_TITLE, *meta = s ? rgb_sel_meta : rgb_meta, *text = s ? rgb_sel_meta : rgb_body;
    slot_begin(s ? rgb_sel : rgb_entry, 0, i + 1);
    if (r[R_KIND] == ENTRY_DOCUMENT)
        doc_s("\xE2\x97\x86 ");
    if (!r[R_N]) {
        doc_s("Untitled");
    } else if (x->t_len && !s) {
        title_text(t, x->t_at);
        doc_rgb(rgb_match);
        title_text(t + x->t_at, x->t_len);
        doc_rgb(rgb_entry);
        title_text(t + x->t_at + x->t_len, r[R_N] - x->t_at - x->t_len);
    } else {
        title_text(t, r[R_N]);
    }
    doc_font(DOC_SANS, 0);
    doc_size(SLOT_META);
    doc_rgb(meta);
    tb_n = 0;
    tb_s("   ");
    tb_kind(r);
    tb_feed(r + R_FEED);
    tb_s("  \xC2\xB7  ");
    tb_date(get32(r + R_TIME));
    doc_text(tb, tb_n);
    slot_meta(text);
    if (x->at != LS_NONE) {
        if (x->ex_pre)
            doc_s("\xE2\x80\xA6");
        doc_text(x->ex, x->ex_at);
        doc_rgb(rgb_match);
        doc_text(x->ex + x->ex_at, x->ex_len);
        doc_rgb(text);
        doc_text(x->ex + x->ex_at + x->ex_len, x->ex_n - x->ex_at - x->ex_len);
        if (x->ex_post)
            doc_s("\xE2\x80\xA6");
    } else {
        doc_rgb(meta);
        if (r[R_KIND] == ENTRY_DOCUMENT)
            doc_s("in the title (of a document, whose data is not searched)");
        else if (!(r[R_FLAGS] & TIMELINE_LOCAL))
            doc_s("in the title (not downloaded: its text was not searched)");
        else
            doc_s("in the title");
    }
    slot_end(i + 1);
}

/* The line under a relay section's heading. */
static void s_section_meta(uint8_t st, uint8_t n)
{
    tb_n = 0;
    if (!st)
        tb_s("searching\xE2\x80\xA6");
    else if (st == STATUS_DONE && n) {
        tb_u(n);
        tb_s(" found");
    }
    else if (st == STATUS_DONE)
        tb_s("none found");
    else if (st == STATUS_REFUSED)
        tb_s("the relays refused the search");
    else
        tb_s("not searched: no relay could be reached");
}

static void search_doc(void)
{
    uint16_t i, end, total = s_rows(), slots = 1;
    const uint8_t *r;
    uint8_t s;
    ls_stale = 0;
    ls_doc_frame = frame_n;
    ldoc_begin();
    tb_n = 0;
    if (!s_ran) {
        tb_s("Type words to look for in your articles, a feed's name or words of a title, then Enter");
    } else {
        tb_s("\xE2\x80\x9C");
        tb_bytes(s_query, s_qlen);
        tb_s("\xE2\x80\x9D  \xC2\xB7  A\xE2\x80\x93Z and a\xE2\x80\x93z match each other, other characters only "
             "themselves, as on the relays");
    }
    header_slot("Search");
    if (!s_ran) {
        list_submit(slots);
        return;
    }
    end = srp.top + vis() + 1;
    if (end > total)
        end = total;
    for (i = srp.top; i < end; i++, slots++) {
        if (s_heading(i)) {
            slot_begin(rgb_head, DOC_BOLD, 0);
            if (!i) {
                doc_s("In your articles");
                slot_meta(rgb_meta);
                ls_section_meta();
            } else {
                doc_s(i == s_hf() ? "Feeds" : "Articles on your relays");
                slot_meta(rgb_meta);
                s_section_meta(i == s_hf() ? sf_st : sa_st, i == s_hf() ? sf_n : sa_n);
            }
            doc_text(tb, tb_n);
            continue;
        }
        s = i == srp.sel;
        if (!s_section(i)) {
            ls_slot(i, s);
            continue;
        }
        r = s_record(i);
        slot_begin(s ? rgb_sel : rgb_entry, 0, i + 1);
        if (s_section(i) == 1)
            doc_title(r + R_TITLE, r[R_N]);
        else
            entry_title(r);
        slot_meta(s ? rgb_sel_meta : rgb_meta);
        tb_n = 0;
        if (s_section(i) == 1) {
            uint8_t nn, nick;
            const uint8_t *nm = name_of(r + R_FEED, &nn, &nick);
            tb_key(r + R_FEED);
            if (nick) {
                tb_s("  \xC2\xB7  your nickname: ");
                tb_name(nm, nn);
            }
            if (own_known && !memcmp(r + R_FEED, own_key, 32))
                tb_s("  \xC2\xB7  you");
            if (find_sub(r + R_FEED) >= 0)
                tb_s("  \xC2\xB7  subscribed");
        } else {
            tb_kind(r);
            tb_feed(r + R_FEED);
            tb_s("  \xC2\xB7  ");
            tb_date(get32(r + R_TIME));
            tb_s("  \xC2\xB7  ");
            tb_size(get24(r + R_SIZE));
        }
        doc_text(tb, tb_n);
        slot_end(i + 1);
    }
    list_submit(slots);
    dn_scan = 1;
}

/* --- The feed page --------------------------------------------------------- */

static void feed_doc(void)
{
    uint8_t n, nick, g;
    const uint8_t *name = name_of(fp_key, &n, &nick);
    int32_t k = find_sub(fp_key);
    ldoc_begin();
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(TITLE_SIZE);
    doc_rgb(rgb_title);
    tb_n = 0;
    if (n)
        tb_name(name, n);
    else
        tb_s("A feed");
    doc_text(tb, tb_n);
    doc_paragraph(DOC_START, 0, 6, 0);
    doc_font(DOC_SANS, 0);
    doc_size(META_SIZE);
    doc_rgb(rgb_meta);
    tb_n = 0;
    if (n) {
        tb_s(nick ? "your nickname for the feed  \xC2\xB7  " : "the name the feed gives itself  \xC2\xB7  ");
        tb_key(fp_key);
    } else {
        tb_s("no name known  \xC2\xB7  ");
        tb_key(fp_key);
    }
    doc_text(tb, tb_n);
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_rgb(rgb_rule);
    doc_rule();
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    doc_s("Feed key:");
    doc_paragraph(DOC_START, 12, 6, DOC_NOWRAP);
    doc_font(DOC_MONO, 0);
    doc_rgb(rgb_key);
    tb_n = 0;
    for (g = 0; g < 8; g++) {
        tb_hex(fp_key + g * 4, 4);
        tb_s(g == 3 ? "\n" : g == 7 ? "" : " ");
    }
    doc_text(tb, tb_n);
    if (fp_hlen) {
        /* The relays the link's address names, one a line. */
        doc_paragraph(DOC_START, 0, 10, 0);
        doc_font(DOC_SANS, 0);
        doc_rgb(rgb_body);
        doc_s("Relays its address names:");
        doc_paragraph(DOC_START, 12, 6, DOC_NOWRAP);
        doc_font(DOC_MONO, 0);
        doc_rgb(rgb_key);
        tb_n = 0;
        for (g = 0; g < fp_hlen; g++)
            if (fp_hints[g] == ',')
                tb_s("\n");
            else
                tb_bytes(fp_hints + g, 1);
        doc_text(tb, tb_n);
        doc_paragraph(DOC_START, 0, 6, 0);
        doc_font(DOC_SANS, 0);
        doc_size(13);
        doc_rgb(rgb_meta);
        doc_s(k >= 0 ? "Subscribing from here passes these on: your host looks for the feed there if your relays do not "
                       "hold it."
                     : "If you subscribe and your relays do not hold this feed, your host connects to these relays to get "
                       "it (the host asks you first). Nothing is fetched from them before that.");
        doc_size(BODY_SIZE);
    }
    doc_paragraph(DOC_START, 0, 14, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    if (own_known && !memcmp(fp_key, own_key, 32)) {
        doc_rgb(rgb_you);
        doc_s(k >= 0 ? "Your own feed. You are subscribed to it." : "Your own feed. You are not subscribed to it.");
    } else if (k >= 0) {
        doc_rgb(rgb_you);
        doc_s("You are subscribed to this feed.");
    } else {
        doc_rgb(rgb_body);
        doc_s("You are not subscribed to this feed.");
    }
    doc_paragraph(DOC_START, 0, 14, 0);
    doc_font(DOC_SANS, 0);
    doc_size(13);
    doc_rgb(rgb_meta);
    doc_s(k >= 0 ? "U: unsubscribe   Esc: back" : "S: subscribe (its articles then arrive in your timeline)   Esc: back");
    column_submit();
}

/* Shows the feed page of `key`; Esc goes back to view `back`. */
static void show(uint8_t v);
static void feed_page(const uint8_t *key, uint8_t back)
{
    memcpy(fp_key, key, 32);
    fp_back = back;
    fp_hlen = 0;
    show(FEED);
    page_dirty = 1;
}

/* --- Help -------------------------------------------------------------- */

static void help_head(const char *s)
{
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(17);
    doc_rgb(rgb_head);
    doc_s(s);
}

static void help_line(const char *keys, const char *what)
{
    doc_paragraph(DOC_START, 12, 4, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(14);
    doc_rgb(rgb_sel);
    doc_s(keys);
    doc_font(DOC_SANS, 0);
    doc_rgb(rgb_body);
    doc_s("   ");
    doc_s(what);
}

static const char *const help_keys[] = {
    0, "Timeline",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn  Home End  wheel", "move the selection",
    "Enter, click", "open the article; a document (\xE2\x97\x86) is passed to the shell to open",
    "/", "search",
    "F", "feeds",
    "? or F1", "this help",
    "T", "publish a test article",
    "M", "the Tersenet manual",
    "Esc", "quit",
    0, "Article",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn Space  Home End  wheel", "scroll",
    "Tab, Shift+Tab", "the next or previous link, in amber, scrolled into view (after you scroll: the first one in view)",
    "Enter, click", "follow the link: terse: links open the article or the feed's page; a link to a program, a program "
                    "image or a document is passed to the shell to open",
    "Esc, Backspace, \xE2\x86\x90", "back to the previous article, then to where you came from",
    0, "Feeds",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn  Home End  wheel, click", "select a feed",
    "Enter", "the feed's page",
    "S", "subscribe: type or paste a feed key or address (key@relay, or a terse:feed/ link), then Enter (Esc "
         "cancels)",
    "U", "unsubscribe from the selected feed",
    "N", "a nickname for the selected feed, which only you see (empty removes it)",
    "P", "your public name, shown beside your key; you choose whether search may find it",
    "/, T, ? or F1", "as in the timeline",
    "Esc, Backspace, \xE2\x86\x90", "back to the timeline",
    0, "Search",
    "Enter", "search the articles on this host, titles and text (the newest first; those not downloaded by title "
             "only), and your relays: listed feeds by name and their articles by title. A\xE2\x80\x93Z and "
             "a\xE2\x80\x93z match each other, other characters only themselves",
    "\xE2\x86\x91 \xE2\x86\x93  wheel, click", "select a result; Enter opens it (an article of yours at its first match; a "
                                       "feed's page, where S subscribes; a document as in the timeline)",
    "typing, Enter on a result, Esc", "stop the search of your articles",
    "Esc", "back",
    0, "Help",
    "T", "publish a test article, and subscribe to your own feed so that it shows in the timeline",
    "\xE2\x86\x91 \xE2\x86\x93  PgUp PgDn  Home End  wheel", "scroll",
    "F", "feeds",
    "Esc, Backspace, \xE2\x86\x90", "back to the timeline",
};

/* The help page: your feed key, then the keys of every view. */
static void help_doc(void)
{
    uint8_t g;
    uint16_t i;
    ldoc_begin();
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(24);
    doc_rgb(rgb_title);
    doc_s("Help");
    help_head("Your feed address");
    doc_font(DOC_SANS, 0);
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    if (own_known) {
        /* Two lines of four groups of eight hex digits, then "@" and the
         * hints, one a line (NETWORK 4.9); an address pasted into the
         * subscribe entry may hold spaces and line breaks. */
        doc_paragraph(DOC_START, 12, 8, DOC_NOWRAP);
        doc_font(DOC_MONO, 0);
        doc_rgb(rgb_key);
        tb_n = 0;
        for (g = 0; g < 8; g++) {
            tb_hex(own_key + g * 4, 4);
            tb_s(g == 3 ? "\n" : g == 7 ? "" : " ");
        }
        for (g = 0; g < own_hlen; g++) {
            if (!g || own_hints[g] == ',')
                tb_s(g ? ",\n" : "\n@");
            if (own_hints[g] != ',')
                tb_bytes(own_hints + g, 1);
        }
        doc_text(tb, tb_n);
        doc_paragraph(DOC_START, 0, 8, 0);
        doc_font(DOC_SANS, 0);
        doc_size(13);
        doc_rgb(rgb_meta);
        if (own_hlen)
            doc_s("Others subscribe to your feed with this address: in this reader, F, then S, then type or paste "
                  "it. The part after the @ names your public relays, so that readers who use other relays find "
                  "your feed too.");
        else
            doc_s("Others subscribe to your feed with this key: in this reader, F, then S, then type or paste it. "
                  "None of your relays is public, so your address has no relay after an @: only people who use the "
                  "same relay as you will find your feed.");
    } else {
        doc_paragraph(DOC_START, 0, 6, 0);
        doc_s("This host has no feed key yet. Press T to publish a test article: that creates one.");
    }
    for (i = 0; i < sizeof help_keys / sizeof help_keys[0]; i += 2) {
        if (help_keys[i])
            help_line(help_keys[i], help_keys[i + 1]);
        else
            help_head(help_keys[i + 1]);
    }
    doc_paragraph(DOC_START, 0, 16, 0);
    doc_font(DOC_SANS, 0);
    doc_size(13);
    doc_rgb(rgb_meta);
    doc_s("Subscribing, unsubscribing, publishing and setting names need your consent: the host asks you with "
          "its own prompt, and the status row says what came of it. Names are chosen by the feeds themselves (or "
          "by you, as nicknames) and are not unique, so the reader shows the short key beside every name.");
    column_submit();
    doc_state = 2;
    page_h_known = 0;
    scroll = scroll_sent = 0;
    to_end = 0;
}

/* --- Article ----------------------------------------------------------- */

/* The title, the details line and the rule. An article known only by
 * its blob ID (a link) is titled with the ID's short form. */
static void doc_head(void)
{
    doc_paragraph(DOC_START, 0, 14, 0);
    doc_font(DOC_SANS, DOC_BOLD);
    doc_size(TITLE_SIZE);
    doc_rgb(rgb_title);
    tb_n = 0;
    if (hd_len)
        tb_bytes(art + hd_at, hd_len);
    else if (!art_entry)
        tb_key(art_blob);
    else if (art_tlen)
        tb_bytes(art_title, art_tlen);
    else
        tb_s("Untitled");
    doc_text(tb, tb_n);
    doc_paragraph(DOC_START, 0, 6, 0);
    doc_font(DOC_SANS, 0);
    doc_size(META_SIZE);
    doc_rgb(rgb_meta);
    tb_n = 0;
    if (art_entry) {
        tb_feed(art_feed);
        tb_s("  \xC2\xB7  #");
        tb_u(art_seq);
        tb_s("  \xC2\xB7  ");
        tb_date(art_time);
        tb_s(" UTC  \xC2\xB7  ");
        tb_size(art_size);
    } else {
        tb_s("terse:");
        tb_key(art_blob);
        tb_s("  \xC2\xB7  not in your timeline");
        if (art_state == READY || art_state == TOO_LARGE) {
            tb_s("  \xC2\xB7  ");
            tb_size(art_size);
        }
    }
    doc_text(tb, tb_n);
    if (art_entry && art_reply) {
        doc_rgb(rgb_reply);
        doc_s("  \xE2\x86\xB3 reply");
    }
    doc_paragraph(DOC_START, 0, 10, 0);
    doc_rgb(rgb_rule);
    doc_rule();
    doc_size(BODY_SIZE);
    doc_rgb(rgb_body);
    b_colour = rgb_body;
    b_style = S_BODY;
}

/* The article's head and a message, while there is no article text. */
static void article_message(const char *text, const uint8_t *rgb, uint8_t back)
{
    ldoc_begin();
    doc_head();
    doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
    doc_rgb(rgb);
    doc_s(text);
    if (back) {
        doc_paragraph(DOC_START, 0, PARA_SPACE, 0);
        doc_rgb(rgb_meta);
        doc_s(bk_n ? "Esc: back to the previous article"
                   : art_return == SEARCH ? "Esc: back to the search" : "Esc: back to the timeline");
    }
    column_submit();
}

/* The page that stands in for the article while it is fetched, or says
 * why it is not shown. */
static void article_page(void)
{
    switch (art_state) {
    case FETCHING:
    case READY:
        article_message(art_entry && art_local ? "Reading the article\xE2\x80\xA6" : "Fetching the article from the relay\xE2\x80\xA6",
                        rgb_meta, 0);
        break;
    case NOT_FOUND:
        article_message("Not found: neither this host nor its relays have this article.", rgb_error, 1);
        break;
    case FAILED:
        article_message("Could not fetch the article (no relay, network error or timeout).", rgb_error, 1);
        break;
    case TOO_LARGE:
        tb_n = 0;
        tb_s("Too large: this article is ");
        tb_size(art_size);
        tb_s(", and this reader shows articles of up to 48 KiB.");
        tb[tb_n] = 0;
        {
            static char m[80];
            memcpy(m, tb, tb_n + 1);
            article_message(m, rgb_error, 1);
        }
        break;
    }
}

static uint32_t max_scroll(void)
{
    return page_h > RECT_H ? page_h - RECT_H : 0;
}

/* Sets the scroll value, kept from going below 0 and, once the height is
 * known, past the end. */
static void scroll_to(int32_t s)
{
    if (s < 0)
        s = 0;
    if (page_h_known && (uint32_t)s > max_scroll())
        s = max_scroll();
    if (s != scroll)
        dirty[STATUS_ROW] = 1;
    scroll = s;
}

static void scroll_by(int32_t delta)
{
    to_end = 0;
    tab_scrolled = 1;
    art_restore = -1;
    scroll_to(scroll + delta);
}

static void page_laid_out(uint32_t h)
{
    /* Before the first submit (a height left from another view) or 0 (no
     * document): not ours. */
    if (!doc_state || !h)
        return;
    page_h = h;
    page_h_known = 1;
    /* Back to an article: its old place, once all of it is laid out. */
    if (art_restore >= 0 && view == ARTICLE && doc_state == 2) {
        scroll = art_restore;
        art_restore = -1;
        tab_scrolled = 1;
    }
    scroll_to(to_end ? (int32_t)max_scroll() : scroll);
    dirty[STATUS_ROW] = 1;
}

static void request_chunk(void)
{
    uint32_t n = art_want - art_have;
    if (n > CHUNK)
        n = CHUNK;
    art_handle = content_get(&art_req, art_blob, art_have, n, art + art_have);
}

/* The entry of record r is the article's. */
static void take_entry(const uint8_t *r)
{
    uint8_t k;
    art_entry = 1;
    art_tlen = r[R_N];
    memcpy(art_title, r + R_TITLE, art_tlen);
    memcpy(art_feed, r + R_FEED, 32);
    memcpy(art_blob, r + R_BLOB, 32);
    art_reply = 0;
    for (k = 0; k < 32; k++)
        art_reply |= r[R_REPLY + k];
    art_local = r[R_FLAGS] & TIMELINE_LOCAL;
    art_seq = get32(r + R_SEQ);
    art_time = get32(r + R_TIME);
    art_size = get24(r + R_SIZE);
}

static void lookup_request(void)
{
    lk_handle = content_feed(&lk_req, CONTENT_TIMELINE, lk_pos, TL_BUF_LEN, lk_buf);
}

/* Starts reading art_blob (its entry known or not); `restore`: the
 * scroll value to go back to, or -1; `state`: FETCHING, or NOT_FOUND or
 * FAILED if that is known already (nothing is requested). */
static void article_start(int32_t restore, uint8_t state)
{
    art_want = art_entry && art_size < ART_CAP ? art_size : ART_CAP;
    art_have = 0;
    art_state = state;
    if (state == FETCHING)
        request_chunk();
    view = ARTICLE;
    b_phase = B_START;
    b_line = b_lines = b_in_line = 0;
    b_kind = b_prev = K_NONE;
    b_shown = links = 0;
    hd_len = 0;
    doc_state = 0;
    page_h = page_h_known = 0;
    scroll = scroll_sent = 0;
    art_restore = restore;
    to_end = 0;
    link_shown = cur_link = 0;
    loc_id = loc_search = tab_scrolled = place.half = tab_more = 0;
    pf_n = pf_next = 0;
    mt_state = MT_NONE;
    dirty[STATUS_ROW] = 1;
    article_page();
}

/* The outcome of an open request, on the status row (instead of the
 * current link). */
static void opened(uint32_t r)
{
    tb_n = 0;
    tb_s(r == 1 ? "Asked the shell to open it" : "This needs the shell");
    say(r == 1 ? C_OK : C_INFO);
    link_shown = 0;
}

/* Asks the shell to open blob `id` (a program image or a document). */
static void open_in_shell(const uint8_t *id)
{
    open_req[0] = OPEN_BLOB;
    memcpy(open_req + 1, id, 32);
    opened(terse_open(open_req));
}

/* Whether the n bytes at p begin a program image or a document. */
static uint8_t not_text(const uint8_t *p, uint32_t n)
{
    return n >= 4 && (!memcmp(p, "TERS", 4) || !memcmp(p, "TDOC", 4));
}

/* Opens the article of record r (timeline or search); Esc goes back to
 * view `from`. A document's entry is passed to the shell instead. */
static void open_record(const uint8_t *r, uint8_t from)
{
    if (r[R_KIND] == ENTRY_DOCUMENT) {
        open_in_shell(r + R_BLOB);
        return;
    }
    if (art_handle)
        return;
    take_entry(r);
    art_link = 0;
    art_return = from;
    bk_n = 0;
    article_start(-1, FETCHING);
}

static void open_blob(const uint8_t *id, uint8_t link, int32_t restore, uint8_t state);

/* The manual's index (built into every host's store); Esc goes back to
 * the timeline. */
static void open_manual(void)
{
    if (art_handle)
        return;
    art_return = TIMELINE;
    bk_n = 0;
    open_blob(manual_index, 0, -1, FETCHING);
}

static void open_article(uint16_t i)
{
    if (i >= tl_total || !in_window(i))
        return;
    open_record(record(i), TIMELINE);
}

/* Opens the article whose blob ID is `id`: its entry is the timeline's
 * if the host has one (the timeline buffer is looked at first, then the
 * whole timeline is read, unless the buffer holds all of it). */
static void open_blob(const uint8_t *id, uint8_t link, int32_t restore, uint8_t state)
{
    uint16_t i;
    memcpy(art_blob, id, 32);
    art_entry = art_tlen = 0;
    art_link = link;
    for (i = 0; i < tl_count; i++)
        if (!memcmp(tl_buf + rec_at[i] + R_BLOB, id, 32)) {
            take_entry(tl_buf + rec_at[i]);
            break;
        }
    if (!art_entry && !(tl_loaded && (!tl_total || (tl_base == 0 && tl_count >= tl_total)))) {
        lk_pos = 0;
        if (lk_handle)
            lk_restart = 1; /* when the one in progress is in */
        else
            lookup_request();
    }
    article_start(restore, state);
}

static void lookup_done(uint8_t status)
{
    uint32_t at = 0, n = 0;
    lk_handle = 0;
    if (lk_restart) {
        lk_restart = 0;
        if (!art_entry)
            lookup_request();
        return;
    }
    if (art_entry)
        return;
    if (status == STATUS_DONE) {
        while (at < lk_req.length) {
            if (!memcmp(lk_buf + at + R_BLOB, art_blob, 32)) {
                take_entry(lk_buf + at);
                if (art_state == FETCHING || art_state == NOT_FOUND || art_state == FAILED)
                    article_page();
                return;
            }
            n++;
            at += 1 + lk_buf[at];
        }
        lk_pos += n;
        if (n && lk_pos < lk_req.size)
            lookup_request();
    }
}

static void article_back(void);

static void article_done(uint8_t status)
{
    uint32_t size;
    art_handle = 0;
    if (status == STATUS_NOT_FOUND || status == STATUS_FAILED || status == STATUS_BAD_REQUEST) {
        art_state = status == STATUS_NOT_FOUND ? NOT_FOUND : FAILED;
        if (view != ARTICLE)
            return;
        if (doc_state == 0) {
            article_page();
        } else {
            tb_n = 0;
            tb_s(art_state == NOT_FOUND ? "Not found: neither this host nor its relays have the rest of the article."
                                        : "Could not fetch the rest of the article.");
            say(C_ERROR);
        }
        dirty[STATUS_ROW] = 1;
        return;
    }
    /* A program image or a document is not shown: the shell is asked to
     * open it, and the view is left. */
    if (!art_have && not_text(art, art_req.length)) {
        art_state = FAILED;
        open_in_shell(art_blob);
        if (view == ARTICLE)
            article_back();
        return;
    }
    size = art_req.size;
    art_size = size;
    /* By a link: an article larger than the reader shows is not shown. */
    if (art_link && size > ART_CAP) {
        art_state = TOO_LARGE;
        if (view == ARTICLE)
            article_page();
        return;
    }
    if (size < art_want)
        art_want = size;
    art_have += art_req.length;
    if (art_have < art_want && art_req.length)
        request_chunk();
    else
        art_want = art_have;
    art_state = READY;
}

/* --- Links ------------------------------------------------------------- */

/* Whether link `id` is in the document as submitted, its Colour command
 * there to be changed. */
static uint8_t link_usable(uint16_t id)
{
    uint32_t at = link_doc[id - 1];
    return id && id <= links && at + 4 <= (uint32_t)(a_ptr - doc) && doc[at] == DOC_OP_COLOUR;
}

static void link_colour(uint16_t id, const uint8_t *rgb)
{
    if (link_usable(id))
        memcpy(doc + link_doc[id - 1] + 1, rgb, 3);
}

/* Makes link `id` the current one (0: none): its colour changes and the
 * document is submitted again (keeping the scroll value) if it is all
 * shown; the status row shows the link. */
static void set_cur_link(uint16_t id)
{
    if (id == cur_link) {
        link_shown = id;
        dirty[STATUS_ROW] = 1;
        return;
    }
    link_colour(cur_link, rgb_link);
    cur_link = id;
    tab_scrolled = 0;
    link_colour(id, rgb_cur_link);
    if (doc_state == 2)
        submit_len(a_ptr - doc);
    link_shown = id;
    dirty[STATUS_ROW] = 1;
}

/* The links in the document as submitted. */
static uint16_t links_in(void)
{
    uint16_t n = 0;
    while (n < links && link_usable(n + 1))
        n++;
    return n;
}

static void locate(uint16_t id)
{
    loc_id = id;
    loc_again = 0;
    page_locate(id);
}

/* Makes link `id` current and asks where it is, to bring it into view. */
static void tab_to(uint16_t id)
{
    loc_id = loc_search = 0;
    set_cur_link(id);
    locate(id);
}

/* Tab and Shift+Tab: the next or previous link in the document; after the
 * user scrolled, the first link at or below the top of the view (Tab) or
 * the last above its bottom (Shift+Tab), found by a binary search over
 * the links' places (the links are in document order, so their y never
 * decreases), one locate a frame. */
static void tab_link(int8_t dir)
{
    uint16_t n = links_in(), id;
    if (!n) {
        tb_n = 0;
        tb_s("No links in this article");
        say(C_INFO);
        return;
    }
    if (loc_id && loc_search) {
        if (tab_more > -100 && tab_more < 100)
            tab_more += dir;
        return;
    }
    /* At the top, the first link in view is the first link. */
    if (tab_scrolled && !(dir > 0 && scroll <= 0)) {
        loc_search = 1;
        tab_more = 0;
        sr_dir = dir;
        sr_lo = 1;
        sr_hi = n + 1;
        sr_at = dir > 0 ? scroll : scroll + RECT_H;
        locate((sr_lo + sr_hi) >> 1);
        return;
    }
    if (dir > 0)
        id = cur_link >= n ? 1 : cur_link + 1;
    else
        id = cur_link <= 1 || cur_link > n ? n : cur_link - 1;
    tab_to(id);
}

/* The place of loc_id has arrived: the next step of the search, or the
 * current link scrolled just enough to be wholly in view, MARGIN units
 * from the edges. */
static void located(void)
{
    uint16_t n, id;
    int32_t s = scroll, top, bottom;
    loc_id = 0;
    if (place.id == MATCH_ID) {
        /* The match of the search: a quarter of the way down the view,
         * unless it is in view at the top or the user has moved since. */
        mt_state = MT_DONE;
        if (!place.height || cur_link || tab_scrolled)
            return;
        if ((int32_t)place.y + place.height + MARGIN > RECT_H) {
            to_end = 0;
            art_restore = -1;
            scroll_to((int32_t)place.y - RECT_H / 4);
        }
        return;
    }
    if (loc_search) {
        if (!place.height || (int32_t)place.y >= sr_at)
            sr_hi = place.id;
        else
            sr_lo = place.id + 1;
        if (sr_lo < sr_hi) {
            locate((sr_lo + sr_hi) >> 1);
            return;
        }
        n = links_in();
        if (sr_dir > 0)
            id = sr_lo <= n ? sr_lo : 1;
        else
            id = sr_lo > 1 ? sr_lo - 1 : n;
        /* The presses that came during the search, in simple order. */
        for (; tab_more > 0; tab_more--)
            id = id >= n ? 1 : id + 1;
        for (; tab_more < 0; tab_more++)
            id = id <= 1 ? n : id - 1;
        tab_to(id);
        return;
    }
    if (!place.height || place.id != cur_link)
        return;
    top = (int32_t)place.y - MARGIN;
    bottom = (int32_t)place.y + place.height + MARGIN;
    if (bottom > s + RECT_H)
        s = bottom - RECT_H;
    if (top < s)
        s = top;
    to_end = 0;
    art_restore = -1;
    scroll_to(s);
}

/* Follows link `id`: an article link gets the first bytes of the blob
 * (probe_done goes on), a feed link opens the feed's page, a program link
 * asks the shell to open the program. */
static void follow(uint16_t id)
{
    const uint8_t *t = art + link_at[id - 1];
    uint8_t target[32];
    switch (link_kind[id - 1]) {
    case LK_ARTICLE:
        if (art_handle || pr_handle) {
            tb_n = 0;
            tb_s(art_handle ? "Wait: this article is still being read" : "Wait: the link is being fetched");
            say(C_INFO);
            return;
        }
        unhex(t + 6, pr_id);
        memcpy(pr_from, art_blob, 32);
        pr_handle = content_get(&pr_req, pr_id, 0, sizeof pr_buf, pr_buf);
        break;
    case LK_FEED:
        unhex(t + 11, target);
        feed_page(target, ARTICLE);
        /* The address's hints, after the "@". */
        if (link_len[id - 1] > 76) {
            fp_hlen = link_len[id - 1] - 76;
            memcpy(fp_hints, t + 76, fp_hlen);
        }
        break;
    case LK_PROGRAM:
        open_req[0] = OPEN_PROGRAM;
        unhex(t + PROG_KEY, open_req + 1);
        open_req[33] = prog_name(t + PROG_NAME, link_len[id - 1] - PROG_NAME, open_req + 34);
        opened(terse_open(open_req));
        break;
    }
}

/* The first bytes of a linked blob are in (or it could not be got): a
 * program image or a document goes to the shell, and the article stays;
 * else the linked article opens (the one shown goes on the back stack).
 * Nothing happens if the article it was followed from is no longer
 * shown. */
static void probe_done(uint8_t status)
{
    pr_handle = 0;
    if (view != ARTICLE || art_handle || memcmp(pr_from, art_blob, 32))
        return;
    if (status == STATUS_DONE && not_text(pr_buf, pr_req.length)) {
        open_in_shell(pr_id);
        return;
    }
    if (bk_n == BACK_N) {
        memmove(bk_blob, bk_blob + 1, sizeof bk_blob - 32);
        memmove(bk_link, bk_link + 1, BACK_N - 1);
        memmove(bk_scroll, bk_scroll + 1, sizeof bk_scroll - sizeof bk_scroll[0]);
        bk_n--;
    }
    memcpy(bk_blob[bk_n], art_blob, 32);
    bk_link[bk_n] = art_link;
    bk_scroll[bk_n] = scroll;
    bk_n++;
    open_blob(pr_id, 1, -1, status == STATUS_DONE ? FETCHING : status == STATUS_NOT_FOUND ? NOT_FOUND : FAILED);
}

/* Esc in an article: the previous article of the back stack, else the
 * view the first one was opened from. */
static void article_back(void)
{
    if (!bk_n) {
        show(art_return);
        return;
    }
    if (art_handle) {
        tb_n = 0;
        tb_s("Wait: this article is still being read");
        say(C_INFO);
        return;
    }
    bk_n--;
    open_blob(bk_blob[bk_n], bk_link[bk_n], bk_scroll[bk_n], FETCHING);
}

/* Back to the article from the feed page: its document is submitted
 * again (or its stand-in shown), at the same place. */
static void article_resume(void)
{
    view = ARTICLE;
    region(RECT_X, PAGE_H);
    page_h_known = 0;
    art_restore = scroll;
    dirty[STATUS_ROW] = 1;
    if (art_state != READY || b_phase == B_START) {
        article_page();
    } else if (b_phase == B_DONE) {
        submit_len(a_ptr - doc);
        doc_state = 2;
    } else if (b_phase != B_START) {
        submit_len(a_ptr - doc);
        doc_state = 1;
    }
}

/* --- Prefetch ------------------------------------------------------------ */

/* Queues an article link's blob (the first PF_N distinct ones, not the
 * article shown). */
static void prefetch_add(const uint8_t *id)
{
    uint8_t k;
    if (pf_n == PF_N || !memcmp(id, art_blob, 32))
        return;
    for (k = 0; k < pf_n; k++)
        if (!memcmp(pf_ids[k], id, 32))
            return;
    memcpy(pf_ids[pf_n++], id, 32);
}

/* Requests pending, of every kind. */
static uint8_t pending(void)
{
    return !!tl_handle + !!sub_handle + !!cnt_handle + !!id_handle + !!op_handle + !!art_handle + !!nm_handle
           + !!lk_handle + !!sf_handle + !!sa_handle + !!pf_handle[0] + !!pf_handle[1] + !!pr_handle + !!dn_handle
           + ls_inflight + !!ls_tl_handle;
}

/* Asks for the format name of the first document on the list shown whose
 * name is not known (one at a time, while few requests are pending; once
 * there is none, until a list is shown again). */
static void dn_step(void)
{
    uint16_t i, end;
    if (!dn_scan || dn_handle || pending() >= PF_PENDING)
        return;
    if (view == TIMELINE) {
        end = tlp.top + vis() + 1;
        for (i = tlp.top; i < end && i < tl_total; i++)
            if (in_window(i) && dn_want(record(i)))
                return;
    } else if (view == SEARCH && s_ran) {
        end = srp.top + vis() + 1;
        for (i = srp.top; i < end && i < s_rows(); i++)
            if (!s_heading(i) && s_section(i) != 1 && dn_want(s_record(i)))
                return;
    }
    dn_scan = 0;
}

/* Sends queued prefetches while few requests are pending. */
static void prefetch_step(void)
{
    uint8_t k;
    for (k = 0; k < PF_SLOTS; k++)
        if (!pf_handle[k] && pf_next < pf_n && pending() < PF_PENDING)
            pf_handle[k] = content_get(&pf_req[k], pf_ids[pf_next++], 0, 1, &pf_byte[k]);
}

/* How far through the page the rectangle is: 0% at the top, 100% at the
 * end (only when the page does not fit). */
static uint32_t percent(void)
{
    int32_t s = scroll;
    uint32_t m = max_scroll();
    if (s < 0)
        s = 0;
    if ((uint32_t)s > m)
        s = m;
    return (uint32_t)s * 100 / m;
}

/* --- Requests that change something ------------------------------------- */

static void op_start(uint8_t state, uint8_t handle)
{
    op_handle = handle;
    op_state = handle ? state : OP_IDLE;
}

static uint8_t op_busy(void)
{
    if (!op_state)
        return 0;
    tb_n = 0;
    tb_s("Busy: the previous request is not answered yet");
    say(C_ERROR);
    return 1;
}

/* Subscribes to (with the hints in op_hints) or unsubscribes from op_key. */
static uint32_t content_subscribe_op(uint8_t op)
{
    if (op == CONTENT_SUBSCRIBE)
        return content_subscribe(&op_req, op_key, op_hints, op_hlen);
    return content_follow(&op_req, op, op_key);
}

static void say_key(const char *before, const uint8_t *key, const char *after, uint8_t fg)
{
    tb_n = 0;
    tb_s(before);
    tb_key(key);
    tb_s(after);
    say(fg);
}

static void published(void)
{
    tb_n = 0;
    tb_s("Published \xE2\x80\x9C");
    tb_s(test_title);
    tb_s("\xE2\x80\x9D as #");
    tb_u(test_seq);
}

/* T: put a short sample article (it differs with the frame and time),
 * then publish it (op_done carries on). */
static void test_publish(void)
{
    int32_t k;
    uint32_t n, frame = terse_frame();
    if (op_busy())
        return;
    k = own_index();
    n = k >= 0 ? get32(sub_buf + k * 36 + 32) + 1 : tests + 1;
    tb_n = 0;
    tb_s("Test article ");
    tb_u(n);
    memcpy(test_title, tb, tb_n);
    test_title[tb_n] = 0;
    test_tlen = tb_n;
    tb_n = 0;
    tb_s("This is a test article, published from the Tersenet reader to check that publishing works. "
         "It was written in frame ");
    tb_u(frame);
    if (now) {
        tb_s(", at ");
        tb_date(now);
        tb_s(" UTC");
    }
    tb_s(".\n\n## Some shape\n\nThe reader splits an article into paragraphs at blank lines and wraps "
         "each one to the width of the page, but keeps\nindented lines as they are:\n\n    frame  ");
    tb_u(frame);
    tb_s("\n    time   ");
    tb_u(now);
    tb_s("\n\n- a list item\n- and a figure:\n\n```path 120 32\nfill #3a6ea5 stroke none M0 4h120v24H0z "
         "fill none stroke #fc6 2 M8 24 40 10 72 20 112 6\n```\n\nThe last paragraph has a link, to "
         "[the manual](terse:");
    tb_hex(manual_index, 32);
    tb_s("), which opens when pressed.\n");
    op_start(OP_PUT, content_put(&op_req, tb, tb_n));
    tb_n = 0;
    tb_s("Publishing \xE2\x80\x9C");
    tb_s(test_title);
    tb_s("\xE2\x80\x9D\xE2\x80\xA6");
    say(C_INFO);
}

static void unsubscribe(void)
{
    if (!subs_n) {
        tb_n = 0;
        tb_s("No feed to unsubscribe from");
        say(C_ERROR);
        return;
    }
    if (op_busy())
        return;
    memcpy(op_key, sub_buf + fdp.sel * 36, 32);
    op_start(OP_UNSUB, content_follow(&op_req, CONTENT_UNSUBSCRIBE, op_key));
    say_key("Unsubscribing from ", op_key, "\xE2\x80\xA6 (the host asks you)", C_INFO);
}

static void op_done(uint8_t st)
{
    uint8_t s = op_state;
    op_handle = 0;
    op_state = OP_IDLE;
    tb_n = 0;
    switch (s) {
    case OP_PUT:
        if (st != STATUS_DONE) {
            tb_s(st == STATUS_REFUSED ? "Could not store the test article: this host does not allow it"
                                      : "Could not store the test article");
            say(C_ERROR);
            break;
        }
        memcpy(op_key, op_req.id, 32);
        op_start(OP_PUBLISH, content_publish(&op_req, op_key, test_title, test_tlen));
        tb_s("Publishing \xE2\x80\x9C");
        tb_s(test_title);
        tb_s("\xE2\x80\x9D\xE2\x80\xA6 (the host asks you)");
        say(C_INFO);
        break;
    case OP_PUBLISH:
        if (st != STATUS_DONE) {
            tb_s(st == STATUS_REFUSED ? "Publishing refused" : "Publishing failed");
            say(C_ERROR);
            break;
        }
        test_seq = op_req.size;
        if (test_seq > tests)
            tests = test_seq;
        published();
        say(C_OK);
        /* Publishing creates the identity if there was none. */
        op_start(OP_IDENT, content_identity_hints(&op_req, own_hints, sizeof own_hints));
        break;
    case OP_IDENT:
        if (st == STATUS_DONE) {
            memcpy(own_key, op_req.id, 32);
            own_hlen = op_req.length;
            own_known = 1;
            page_dirty = 1;
            if (subs_loaded && find_sub(own_key) < 0) {
                op_start(OP_SELF_SUB, content_follow(&op_req, CONTENT_SUBSCRIBE, own_key));
                published();
                tb_s("; subscribing to your own feed so that it shows in the timeline\xE2\x80\xA6");
                say(C_INFO);
                break;
            }
        }
        reload();
        break;
    case OP_SELF_SUB:
        published();
        if (st == STATUS_DONE)
            tb_s(" and subscribed to your own feed");
        else
            tb_s(st == STATUS_REFUSED ? "; subscribing to your own feed was refused, so it is not in the timeline"
                                      : "; subscribing to your own feed failed");
        say(st == STATUS_DONE ? C_OK : C_ERROR);
        reload();
        break;
    case OP_NICK:
        tb_s("Nickname for ");
        tb_key(op_key);
        if (st == STATUS_DONE) {
            if (op_nlen) {
                tb_s(" set to \xE2\x80\x9C");
                tb_bytes(op_name, op_nlen);
                tb_s("\xE2\x80\x9D");
            } else {
                tb_s(" removed");
            }
            say(C_OK);
            load_names();
        } else {
            tb_s(st == STATUS_REFUSED ? ": refused" : ": failed");
            say(C_ERROR);
        }
        break;
    case OP_PROFILE:
        if (st == STATUS_DONE) {
            tb_s("Your public name is now \xE2\x80\x9C");
            tb_bytes(op_name, op_nlen);
            tb_s(op_listed ? "\xE2\x80\x9D, and others can find it by search (#" : "\xE2\x80\x9D, not listed for search (#");
            tb_u(op_req.size);
            tb_s(")");
            say(C_OK);
            /* Setting a profile creates the identity if there was none. */
            if (!id_handle)
                id_handle = content_identity_hints(&id_req, own_hints, sizeof own_hints);
            reload();
        } else {
            tb_s(st == STATUS_REFUSED ? "Setting your public name was refused" : "Setting your public name failed");
            say(C_ERROR);
        }
        break;
    case OP_SUB:
    case OP_UNSUB:
        tb_s(s == OP_SUB ? "Subscrib" : "Unsubscrib");
        if (st == STATUS_DONE) {
            tb_s(s == OP_SUB ? "ed to " : "ed from ");
            tb_key(op_key);
            say(C_OK);
            reload();
        } else {
            tb_s(s == OP_SUB ? "ing to " : "ing from ");
            tb_key(op_key);
            tb_s(st == STATUS_REFUSED ? " refused" : " failed");
            say(C_ERROR);
        }
        break;
    }
}

/* --- Typing on row 28 ---------------------------------------------------- */

/* Opens (mode E_KEY, E_NICK, E_NAME, E_SEARCH) or closes (E_OFF) the
 * entry row; `eat`: the character of the key that opened it. */
static void entry_open(uint8_t mode, uint8_t eat)
{
    entry = mode;
    entry_n = 0;
    entry_eat = eat;
    entry_eat_until = terse_frame() + 1;
    rect_h = mode ? PAGE_H - 16 : PAGE_H;
    lp_keep(&fdp, subs_n);
    lp_keep(&srp, s_rows());
    page_dirty = 1;
    dirty[ENTRY_ROW] = dirty[STATUS_ROW] = 1;
    if (!mode)
        term_cmd(TERM_CURSOR_OFF);
}

static void entry_show(uint8_t on)
{
    entry_open(on ? E_KEY : E_OFF, 0);
}

/* A character typed or pasted. A feed key leaves blanks out; text keeps
 * them, but not line breaks. */
static void entry_text(uint32_t cp)
{
    uint8_t u[4], n, k;
    if (entry_eat) {
        uint8_t eat = entry_eat;
        entry_eat = 0;
        if ((cp | 0x20) == eat && terse_frame() <= entry_eat_until)
            return; /* the text of the key that opened the entry */
    }
    if (entry == E_KEY) {
        if (cp == ' ' || cp == '\t' || cp == '\n' || cp == '\r')
            return;
        if (!entry_n && (cp == 's' || cp == 'S'))
            return; /* the text of the S that opened the entry */
        if (entry_n < ENTRY_CAP)
            entry_buf[entry_n] = cp > 0x20 && cp < 0x7F ? cp : 0x80;
        if (entry_n < 0xFFFF)
            entry_n++;
    } else {
        if (cp == '\n' || cp == '\r')
            return;
        if (cp < 0x80) {
            u[0] = cp;
            n = 1;
        } else if (cp < 0x800) {
            u[0] = 0xC0 | cp >> 6;
            u[1] = 0x80 | (cp & 0x3F);
            n = 2;
        } else if (cp < 0x10000) {
            u[0] = 0xE0 | cp >> 12;
            u[1] = 0x80 | (cp >> 6 & 0x3F);
            u[2] = 0x80 | (cp & 0x3F);
            n = 3;
        } else {
            u[0] = 0xF0 | cp >> 18;
            u[1] = 0x80 | (cp >> 12 & 0x3F);
            u[2] = 0x80 | (cp >> 6 & 0x3F);
            u[3] = 0x80 | (cp & 0x3F);
            n = 4;
        }
        for (k = 0; k < n; k++) {
            if (entry_n < ENTRY_CAP)
                entry_buf[entry_n] = u[k];
            if (entry_n < 0xFFFF)
                entry_n++;
        }
    }
    dirty[ENTRY_ROW] = 1;
    if (msg_n) {
        msg_n = 0;
        dirty[STATUS_ROW] = 1;
    }
    if (entry == E_SEARCH)
        ls_halt(LS_STOPPED); /* the text changed */
}

/* Backspace: one character (a whole UTF-8 sequence in text). */
static void entry_back(void)
{
    if (!entry_n)
        return;
    if (entry == E_SEARCH)
        ls_halt(LS_STOPPED);
    if (entry != E_KEY && entry_n <= ENTRY_CAP)
        while (entry_n > 1 && (entry_buf[entry_n - 1] & 0xC0) == 0x80)
            entry_n--;
    entry_n--;
    dirty[ENTRY_ROW] = 1;
}

/* Enter on a feed key or address (NETWORK 4.9), typed or pasted, blanks
 * left out: "terse:feed/" optional, 64 hex digits (either case), then
 * optionally "@" and a hint list. */
static void entry_submit_key(void)
{
    uint8_t key[32], *p = entry_buf;
    uint16_t i, n = entry_n < ENTRY_CAP ? entry_n : ENTRY_CAP, at, hn = 0;
    tb_n = 0;
    if (!entry_n) {
        tb_s("Type or paste a feed key or address (key@relay), or Esc to cancel");
        say(C_ERROR);
        return;
    }
    if (n >= 11 && !memcmp(p, "terse:feed/", 11))
        p += 11, n -= 11;
    for (at = 0; at < n && p[at] != '@'; at++)
        ;
    for (i = 0; i < at; i++)
        if (hex_value(p[i]) > 15)
            break;
    if (i < at) {
        tb_s("Not a feed key: a key has only the hex digits 0-9 and a-f");
        say(C_ERROR);
        return;
    }
    if (at != 64) {
        tb_s("Not a feed key: ");
        tb_u(at);
        tb_s(" hex digits, but a key has 64");
        say(C_ERROR);
        return;
    }
    if (at < n) {
        hn = n - at - 1;
        if (entry_n > ENTRY_CAP || !hints_ok(p + at + 1, hn)) {
            tb_s("Not a feed address: after @ give one or two relays, host or host:port");
            say(C_ERROR);
            return;
        }
    }
    for (i = 0; i < 32; i++)
        key[i] = hex_value(p[2 * i]) << 4 | hex_value(p[2 * i + 1]);
    if (find_sub(key) >= 0) {
        entry_show(0);
        say_key("Already subscribed to ", key, "", C_INFO);
        return;
    }
    if (op_busy())
        return;
    entry_show(0);
    memcpy(op_key, key, 32);
    op_hlen = hn;
    memcpy(op_hints, p + 65, hn);
    op_start(OP_SUB, content_subscribe_op(CONTENT_SUBSCRIBE));
    say_key("Subscribing to ", key, hn ? "\xE2\x80\xA6 with the relays its address names (the host asks you)"
                                       : "\xE2\x80\xA6 (the host asks you)",
            C_INFO);
}

/* The code point of the UTF-8 sequence at p (valid: typed text). */
static uint32_t utf8_at(const uint8_t *p)
{
    if (p[0] < 0x80)
        return p[0];
    if (p[0] < 0xE0)
        return (uint32_t)(p[0] & 0x1F) << 6 | (p[1] & 0x3F);
    if (p[0] < 0xF0)
        return (uint32_t)(p[0] & 0x0F) << 12 | (uint32_t)(p[1] & 0x3F) << 6 | (p[2] & 0x3F);
    return (uint32_t)(p[0] & 0x07) << 18 | (uint32_t)(p[1] & 0x3F) << 12 | (uint32_t)(p[2] & 0x3F) << 6 | (p[3] & 0x3F);
}

/* The Unicode White_Space property (NETWORK 4.6). */
static uint8_t is_space(uint32_t c)
{
    return (c >= 0x09 && c <= 0x0D) || c == 0x20 || c == 0x85 || c == 0xA0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200A)
           || c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F || c == 0x3000;
}

/* Checks the typed text as a display name (NETWORK 4.6); says what is
 * wrong and returns 0 if it is not one. `what`: "A nickname", "A name". */
static uint8_t name_ok(const char *what)
{
    uint16_t i = 0, last = 0;
    uint32_t c;
    tb_n = 0;
    if (entry_n > 48) {
        tb_s(what);
        tb_s(" has at most 48 bytes of UTF-8; this one has ");
        tb_u(entry_n);
        say(C_ERROR);
        return 0;
    }
    while (i < entry_n) {
        c = utf8_at(entry_buf + i);
        if (c < 0x20 || (c >= 0x7F && c < 0xA0)) {
            tb_s(what);
            tb_s(" cannot hold control characters");
            say(C_ERROR);
            return 0;
        }
        if (!i && is_space(c))
            break;
        last = i;
        i += entry_buf[i] < 0x80 ? 1 : entry_buf[i] < 0xE0 ? 2 : entry_buf[i] < 0xF0 ? 3 : 4;
    }
    if (i < entry_n || is_space(utf8_at(entry_buf + last))) {
        tb_s(what);
        tb_s(" cannot start or end with a space");
        say(C_ERROR);
        return 0;
    }
    return 1;
}

static void entry_submit_nick(void)
{
    if (entry_n && !name_ok("A nickname"))
        return;
    if (op_busy())
        return;
    memcpy(op_key, nick_key, 32);
    op_nlen = entry_n;
    memcpy(op_name, entry_buf, op_nlen);
    entry_open(E_OFF, 0);
    op_start(OP_NICK, content_set_nickname(&op_req, op_key, op_name, op_nlen));
    tb_n = 0;
    tb_s(op_nlen ? "Setting the nickname for " : "Removing the nickname of ");
    tb_key(op_key);
    tb_s("\xE2\x80\xA6 (the host asks you)");
    say(C_INFO);
}

static void entry_submit_name(void)
{
    if (!entry_n) {
        tb_n = 0;
        tb_s("Type the name others will see beside your feed key, or Esc to cancel");
        say(C_ERROR);
        return;
    }
    if (!name_ok("A name") || op_busy())
        return;
    op_nlen = entry_n;
    memcpy(op_name, entry_buf, op_nlen);
    entry_open(E_OFF, 0);
    asking = 1;
    dirty[STATUS_ROW] = 1;
}

/* The answer to "let others find this name by search?" */
static void answer_listed(uint8_t listed)
{
    asking = 0;
    op_listed = listed;
    op_start(OP_PROFILE, content_set_profile(&op_req, op_name, op_nlen, listed));
    tb_n = 0;
    tb_s("Publishing your public name \xE2\x80\x9C");
    tb_bytes(op_name, op_nlen);
    tb_s("\xE2\x80\x9D\xE2\x80\xA6 (the host asks you)");
    say(C_INFO);
}

/* Enter in the search entry: both searches at once. */
static void search_run(void)
{
    tb_n = 0;
    if (!entry_n) {
        tb_s("Type a name or words of a title to search for, or Esc to go back");
        say(C_ERROR);
        return;
    }
    if (entry_n > 48) {
        tb_s("A search has at most 48 bytes of UTF-8; this one has ");
        tb_u(entry_n);
        say(C_ERROR);
        return;
    }
    if (sf_handle || sa_handle) {
        tb_s("Busy: the previous search is not answered yet");
        say(C_ERROR);
        return;
    }
    s_qlen = entry_n;
    memcpy(s_query, entry_buf, s_qlen);
    sf_n = sa_n = 0;
    s_ran = 1;
    ls_start();
    sf_handle = content_find(&sf_req, FIND_FEEDS, s_query, s_qlen, sf_buf, FIND_LEN);
    sa_handle = content_find(&sa_req, FIND_ENTRIES, s_query, s_qlen, sa_buf, FIND_LEN);
    sf_st = sf_handle ? 0 : STATUS_FAILED;
    sa_st = sa_handle ? 0 : STATUS_FAILED;
    if (!sf_handle && !sa_handle) {
        entry_open(E_OFF, 0);
        tb_s("Could not send the search to the relays: too many requests pending");
        say(C_ERROR);
        return;
    }
    srp.sel = srp.top = 0;
    entry_open(E_OFF, 0);
    tb_s("Searching your articles and relays for \xE2\x80\x9C");
    tb_bytes(s_query, s_qlen);
    tb_s("\xE2\x80\x9D\xE2\x80\xA6");
    say(C_INFO);
}

/* A search request is in: when both are, the selection goes to the first
 * result and the status row says how it went. */
static void find_done(uint8_t feeds, uint8_t status)
{
    if (feeds) {
        sf_handle = 0;
        sf_st = status;
        sf_n = status == STATUS_DONE ? find_parse(sf_buf, &sf_req, sf_at) : 0;
    } else {
        sa_handle = 0;
        sa_st = status;
        sa_n = status == STATUS_DONE ? find_parse(sa_buf, &sa_req, sa_at) : 0;
    }
    page_dirty = 1;
    if (sf_handle || sa_handle)
        return;
    /* The first result, unless one of this host's is selected already. */
    if (s_heading(srp.sel)) {
        srp.sel = s_first();
        srp.top = 0;
    }
    lp_keep(&srp, s_rows());
    tb_n = 0;
    if (sf_n || sa_n) {
        tb_u(sf_n);
        tb_s(sf_n == 1 ? " feed and " : " feeds and ");
        tb_u(sa_n);
        tb_s(sa_n == 1 ? " article found on your relays" : " articles found on your relays");
        say(C_OK);
    } else if (sf_st == STATUS_DONE || sa_st == STATUS_DONE) {
        tb_s("Nothing on your relays for \xE2\x80\x9C");
        tb_bytes(s_query, s_qlen);
        tb_s("\xE2\x80\x9D (only listed feeds can be found)");
        say(C_INFO);
    } else if (sf_st == STATUS_REFUSED || sa_st == STATUS_REFUSED) {
        tb_s("Relay search refused (busy relays, or not on this host): your articles only");
        say(C_INFO);
    } else {
        tb_s("No relay could be reached: your articles only");
        say(C_INFO);
    }
}

static void entry_submit(void)
{
    switch (entry) {
    case E_KEY: entry_submit_key(); break;
    case E_NICK: entry_submit_nick(); break;
    case E_NAME: entry_submit_name(); break;
    case E_SEARCH: search_run(); break;
    }
}

/* N in the feeds view: a nickname for the selected feed. */
static void nickname(void)
{
    if (!subs_n) {
        tb_n = 0;
        tb_s("No feed to give a nickname to");
        say(C_ERROR);
        return;
    }
    memcpy(nick_key, sub_buf + fdp.sel * 36, 32);
    entry_open(E_NICK, 'n');
}

/* --- Terminal rows ------------------------------------------------------- */

static void bar_row(uint8_t row)
{
    row_start(row, C_BAR_FG, C_BAR_BG);
}

static void draw_entry_row(void)
{
    uint16_t n = entry_n < ENTRY_CAP ? entry_n : ENTRY_CAP, from = 0, i;
    if (!entry) {
        row_start(ENTRY_ROW, C_TEXT, C_BG);
        row_end();
        return;
    }
    row_start(ENTRY_ROW, C_TEXT, C_ENTRY_BG);
    if (entry == E_KEY) {
        puts_(" Address: ");
        if (n > COLS - 12) {
            from = n - (COLS - 13);
            ink(C_DIM);
            put1(0x2026);
            ink(C_TEXT);
        }
        for (i = from; i < n; i++)
            put1(entry_buf[i] < 0x80 ? entry_buf[i] : 0xFFFD);
    } else {
        if (entry == E_NICK) {
            tb_n = 0;
            tb_s(" Nickname for ");
            tb_key(nick_key);
            tb_s(": ");
            tb[tb_n] = 0;
            puts_(tb);
        } else {
            puts_(entry == E_NAME ? " Your public name: " : " Search: ");
        }
        /* Not cut inside a character. */
        while (n < entry_n && n && (entry_buf[n] & 0xC0) == 0x80)
            n--;
        put_title(entry_buf, n > 255 ? 255 : n, COLS - 1 - col);
    }
    i = col;
    row_end();
    term_goto(i, ENTRY_ROW);
    term_cmd(TERM_CURSOR_ON);
}

/* The status row's account of the current link. */
static void link_status(void)
{
    uint16_t id = link_shown;
    uint32_t n = link_len[id - 1];
    const uint8_t *t = art + link_at[id - 1];
    puts_(" Link ");
    put_u32(id);
    puts_(": ");
    if (link_kind[id - 1] == LK_FEED) {
        uint8_t key[32];
        unhex(t + 11, key);
        tb_n = 0;
        tb_s("terse:feed/");
        tb_key(key);
        tb_bytes(t + 75, n - 75); /* "@" and the hints, if any */
        tb_s("  (a feed)   Enter: open");
    } else if (link_kind[id - 1] == LK_PROGRAM) {
        uint8_t key[32], name[48];
        unhex(t + PROG_KEY, key);
        tb_n = 0;
        tb_s("program \xE2\x80\x9C");
        tb_name(name, prog_name(t + PROG_NAME, n - PROG_NAME, name));
        tb_s("\xE2\x80\x9D by ");
        tb_feed(key);
        tb_s("   Enter: open");
    } else {
        uint8_t id32[32];
        unhex(t + 6, id32);
        tb_n = 0;
        tb_s("terse:");
        tb_key(id32);
        tb_s("  (an article)   Enter: open");
    }
    put_title((const uint8_t *)tb, tb_n, COLS - 1 - col);
}

static void draw_status(void)
{
    bar_row(STATUS_ROW);
    if (view == ARTICLE && link_shown) {
        link_status();
    } else if (asking) {
        puts_(" Let others find \xE2\x80\x9C");
        put_title(op_name, op_nlen, 40);
        puts_("\xE2\x80\x9D by search?   Y: yes   N: no   Esc: cancel");
    } else if (msg_n) {
        spaces(1);
        ink(msg_fg);
        put_title((const uint8_t *)msg, msg_n, COLS - 2);
    } else if (entry == E_KEY) {
        puts_(" Type or paste it (a key, or key@relay)   Enter: subscribe   Esc: cancel");
    } else if (entry == E_NICK) {
        puts_(" Type a nickname (only you see it; empty removes it)   Enter: set   Esc: cancel");
    } else if (entry == E_NAME) {
        puts_(" Type the name others see beside your key   Enter: next   Esc: cancel");
    } else if (entry == E_SEARCH) {
        puts_(" Words in your articles, or a feed's name   Enter: search   Esc: cancel");
    } else if (view == TIMELINE) {
        puts_(" \xE2\x86\x91\xE2\x86\x93 move  Enter open  / search  F feeds  ? help  T test  Esc quit");
        if (tl_total) {
            ink(C_BAR_DIM);
            tb_n = 0;
            tb_u(tlp.sel + 1);
            tb_s("/");
            tb_u(tl_total);
            pad_to(COLS - 1 - tb_n);
            put_title((const uint8_t *)tb, tb_n, tb_n);
        }
    } else if (view == FEEDS) {
        puts_(" \xE2\x86\x91\xE2\x86\x93 move  S subscribe  U unsub  N nickname  P your name  / search  Esc back");
    } else if (view == SEARCH) {
        puts_(" \xE2\x86\x91\xE2\x86\x93 select  Enter open  / new search  Esc back");
    } else if (view == FEED) {
        puts_(find_sub(fp_key) >= 0 ? " U unsubscribe  Esc back" : " S subscribe  Esc back");
    } else {
        if (doc_state && page_h_known && max_scroll()) {
            spaces(1);
            put_u32(percent());
            puts_(view == HELP || doc_state == 2 ? "%" : "%\xE2\x80\xA6");
        }
        if (view == HELP) {
            pad_to(COLS - 48);
            puts_("\xE2\x86\x91\xE2\x86\x93 scroll  T publish a test  F feeds  Esc back");
        } else {
            pad_to(COLS - 55);
            puts_("Tab link  Enter follow  \xE2\x86\x91\xE2\x86\x93 PgUp PgDn  Esc/\xE2\x86\x90 back");
        }
    }
    row_end();
}

/* --- Events and frames ------------------------------------------------- */

static void show(uint8_t v)
{
    if (view == v)
        return;
    if (view == SEARCH)
        ls_halt(LS_STOPPED); /* leaving the view stops the search */
    if (entry)
        entry_open(E_OFF, 0);
    asking = 0;
    if (v == ARTICLE) {
        article_resume();
        return;
    }
    view = v;
    link_shown = 0;
    page_dirty = 1;
    dirty[STATUS_ROW] = 1;
    if (v == FEEDS) {
        lp_keep(&fdp, subs_n);
        start_count();
    }
}

/* Opens the search view (from the timeline or feeds) with the entry. */
static void search_open(void)
{
    if (view != SEARCH)
        s_back = view;
    show(SEARCH);
    entry_open(E_SEARCH, '/');
}

/* Scrolling keys of the article and help. */
static void scroll_key(uint8_t code)
{
    switch (code) {
    case KEY_UP: scroll_by(-LINE); break;
    case KEY_DOWN: scroll_by(LINE); break;
    case KEY_PAGE_UP: scroll_by(-(RECT_H - LINE)); break;
    case KEY_PAGE_DOWN: case KEY_SPACE: scroll_by(RECT_H - LINE); break;
    case KEY_HOME: scroll_by(-scroll); break;
    case KEY_END:
        to_end = 1;
        art_restore = -1;
        tab_scrolled = 1;
        if (page_h_known)
            scroll_to(max_scroll());
        break;
    }
}

/* Opens the selected search result: a feed's page or an article. */
static void search_open_row(uint16_t r)
{
    const uint8_t *rec;
    uint8_t sec;
    if (r >= s_rows() || s_heading(r))
        return;
    rec = s_record(r);
    sec = s_section(r);
    ls_halt(LS_STOPPED); /* opening a result stops the search */
    if (sec == 1) {
        feed_page(rec + R_FEED, SEARCH);
        return;
    }
    open_record(rec, SEARCH);
    /* A result from this host's text opens at its first match. */
    if (!sec && view == ARTICLE && ls_res[r - 1].at != LS_NONE) {
        mt_at = ls_res[r - 1].at;
        mt_len = ls_m;
        mt_state = MT_WANT;
    }
}

/* S and U on the feed page. */
static void feed_follow(uint8_t op)
{
    if ((op == CONTENT_SUBSCRIBE) == (find_sub(fp_key) >= 0)) {
        say_key(op == CONTENT_SUBSCRIBE ? "Already subscribed to " : "Not subscribed to ", fp_key, "", C_INFO);
        return;
    }
    if (op_busy())
        return;
    memcpy(op_key, fp_key, 32);
    op_hlen = op == CONTENT_SUBSCRIBE ? fp_hlen : 0;
    memcpy(op_hints, fp_hints, op_hlen);
    op_start(op == CONTENT_SUBSCRIBE ? OP_SUB : OP_UNSUB, content_subscribe_op(op));
    say_key(op == CONTENT_SUBSCRIBE ? "Subscribing to " : "Unsubscribing from ", op_key, "\xE2\x80\xA6 (the host asks you)",
            C_INFO);
}

/* Returns 1 to quit. */
static uint8_t key(uint8_t code, uint8_t mods)
{
    uint8_t back = code == KEY_ESCAPE || code == KEY_BACKSPACE || code == KEY_LEFT;
    uint8_t plain = !(mods & (MOD_CONTROL | MOD_ALT | MOD_META));
    if (msg_n) {
        msg_n = 0;
        dirty[STATUS_ROW] = 1;
    }
    if (asking) {
        if (code == KEY_Y)
            answer_listed(1);
        else if (code == KEY_N)
            answer_listed(0);
        else if (code == KEY_ESCAPE) {
            asking = 0;
            dirty[STATUS_ROW] = 1;
        }
        return 0;
    }
    if (entry) {
        if (code == KEY_ENTER) {
            entry_submit();
        } else if (code == KEY_ESCAPE) {
            entry_open(E_OFF, 0);
            if (view == SEARCH && !s_ran)
                show(s_back);
        } else if (code == KEY_BACKSPACE) {
            entry_back();
        }
        return 0;
    }
    if (view == ARTICLE) {
        if (code == KEY_TAB) {
            tab_link(mods & MOD_SHIFT ? -1 : 1);
            return 0;
        }
        if (code == KEY_ENTER) {
            if (cur_link)
                follow(cur_link);
            return 0;
        }
        if (link_shown) {
            link_shown = 0;
            dirty[STATUS_ROW] = 1;
        }
        if (back)
            article_back();
        else
            scroll_key(code);
        return 0;
    }
    if (view == FEED) {
        if (back)
            show(fp_back);
        else if (plain && code == KEY_S)
            feed_follow(CONTENT_SUBSCRIBE);
        else if (plain && code == KEY_U)
            feed_follow(CONTENT_UNSUBSCRIBE);
        return 0;
    }
    if (view == SEARCH) {
        uint16_t old = srp.sel;
        if (back) {
            show(s_back);
        } else if (code == KEY_ENTER) {
            search_open_row(srp.sel);
        } else if (lp_key(&srp, s_rows(), code)) {
            s_fix(srp.sel >= old ? 1 : -1);
        }
        return 0;
    }
    if (plain) {
        switch (code) {
        case KEY_F: show(FEEDS); return 0;
        case KEY_F1: show(HELP); return 0;
        case KEY_T: test_publish(); return 0;
        case KEY_S:
            if (view == FEEDS)
                entry_show(1);
            return 0;
        case KEY_U:
            if (view == FEEDS)
                unsubscribe();
            return 0;
        case KEY_N:
            if (view == FEEDS)
                nickname();
            return 0;
        case KEY_P:
            if (view == FEEDS && !op_busy())
                entry_open(E_NAME, 'p');
            return 0;
        }
    }
    if (view == TIMELINE) {
        if (code == KEY_ESCAPE)
            return 1;
        if (plain && code == KEY_M) {
            open_manual();
            return 0;
        }
        if (code == KEY_ENTER)
            open_article(tlp.sel);
        else
            lp_key(&tlp, tl_total, code);
    } else if (back) {
        show(TIMELINE);
    } else if (view == FEEDS) {
        if (code == KEY_ENTER && subs_n)
            feed_page(sub_buf + fdp.sel * 36, FEEDS);
        else
            lp_key(&fdp, subs_n, code);
    } else {
        scroll_key(code);
    }
    return 0;
}

static void anchor(uint16_t id, uint8_t buttons)
{
    if (!id || entry || asking)
        return;
    if (view == ARTICLE) {
        if (id <= links) {
            if (buttons & BUTTON_PRIMARY)
                follow(id);
        }
    } else if (!(buttons & BUTTON_PRIMARY)) {
        return;
    } else if (view == TIMELINE && id <= tl_total) {
        lp_set(&tlp, tl_total, id - 1, tlp.top);
        open_article(id - 1);
    } else if (view == FEEDS && id <= subs_n) {
        lp_set(&fdp, subs_n, id - 1, fdp.top);
    } else if (view == SEARCH && id <= s_rows()) {
        lp_set(&srp, s_rows(), id - 1, srp.top);
        search_open_row(id - 1);
    }
}

/* A completion is for `r` if its handle is `h` and `r` is no longer
 * pending: a request submitted since, which may have been given the same
 * handle, is still pending. */
static uint8_t done_for(uint8_t h, const content_req *r)
{
    return h && h == ev.a && r->status != STATUS_PENDING;
}

/* Returns 1 to quit. */
static uint8_t handle_events(void)
{
    uint32_t cp;
    while (terse_event_pop(&ev)) {
        switch (ev.type) {
        case EV_KEY_DOWN:
            if (key(ev.a, ev.b))
                return 1;
            break;
        case EV_TEXT:
            cp = ev.a | (uint32_t)ev.b << 8 | (uint32_t)(ev.c & 0x1F) << 16;
            if (entry)
                entry_text(cp);
            else if (asking)
                break;
            else if (cp == '?' && view != ARTICLE)
                show(HELP);
            else if (cp == '/' && view != ARTICLE)
                search_open();
            break;
        case EV_WHEEL:
            if (view == TIMELINE) {
                lp_wheel(&tlp, tl_total, (int8_t)ev.a);
            } else if (view == FEEDS) {
                if (!entry)
                    lp_wheel(&fdp, subs_n, (int8_t)ev.a);
            } else if (view == SEARCH) {
                if (!entry) {
                    uint16_t old = srp.sel;
                    lp_wheel(&srp, s_rows(), (int8_t)ev.a);
                    s_fix(srp.sel >= old ? 1 : -1);
                }
            } else if (view == FEED) {
            } else {
                if (link_shown) {
                    link_shown = 0;
                    dirty[STATUS_ROW] = 1;
                }
                scroll_by((int8_t)ev.a * 3 * LINE);
            }
            break;
        case EV_PAGE_LAID_OUT:
            if (view == ARTICLE && loc_id)
                loc_again = 1;
            if (view == TIMELINE || view == FEEDS || view == SEARCH)
                list_height(EV_PAGE_HEIGHT(ev));
            else if (view != FEED)
                page_laid_out(EV_PAGE_HEIGHT(ev));
            break;
        case EV_PAGE_ANCHOR: anchor(EV_ANCHOR_ID(ev), EV_ANCHOR_BUTTONS(ev)); break;
        case EV_PAGE_LOCATED:
        case EV_PAGE_LOCATED_Y:
            if (page_place_event(&ev, &place) && view == ARTICLE && loc_id && place.id == loc_id)
                located();
            break;
        case EV_FEEDS_UPDATED: reload(); break;
        case EV_CONTENT_DONE:
            if (done_for(tl_handle, &tl_req))
                timeline_done(ev.b);
            else if (done_for(art_handle, &art_req))
                article_done(ev.b);
            else if (done_for(sub_handle, &sub_req))
                subs_done(ev.b);
            else if (done_for(cnt_handle, &cnt_req))
                count_done(ev.b);
            else if (done_for(op_handle, &op_req))
                op_done(ev.b);
            else if (done_for(nm_handle, &nm_req))
                names_done(ev.b);
            else if (done_for(lk_handle, &lk_req))
                lookup_done(ev.b);
            else if (done_for(sf_handle, &sf_req))
                find_done(1, ev.b);
            else if (done_for(sa_handle, &sa_req))
                find_done(0, ev.b);
            else if (done_for(pr_handle, &pr_req))
                probe_done(ev.b);
            else if (done_for(dn_handle, &dn_req))
                dn_done(ev.b);
            else if (done_for(pf_handle[0], &pf_req[0]))
                pf_handle[0] = 0;
            else if (done_for(pf_handle[1], &pf_req[1]))
                pf_handle[1] = 0;
            else if (ls_event())
                ;
            else if (done_for(id_handle, &id_req)) {
                id_handle = 0;
                if (ev.b == STATUS_DONE) {
                    memcpy(own_key, id_req.id, 32);
                    own_hlen = id_req.length;
                    own_known = 1;
                    page_dirty = 1;
                }
            }
            break;
        }
    }
    if (loc_again) {
        loc_again = 0;
        if (view == ARTICLE && loc_id)
            page_locate(loc_id);
    }
    return 0;
}

static void render(void)
{
    uint8_t built = page_dirty && view != ARTICLE;
    if (view == TIMELINE)
        ensure_window();
    /* The search's progress, now and then while it runs. */
    if (view == SEARCH && ls_stale && frame_n - ls_doc_frame >= LS_EVERY)
        built = page_dirty = 1;
    if (page_dirty && view != ARTICLE) {
        page_dirty = 0;
        if (view == TIMELINE)
            timeline_doc();
        else if (view == FEEDS)
            feeds_doc();
        else if (view == SEARCH)
            search_doc();
        else if (view == FEED)
            feed_doc();
        else
            help_doc();
    }
    prefetch_step();
    dn_step();
    ls_step(built);
    if (view == ARTICLE)
        build();
    if (view == ARTICLE || view == HELP)
        send_scroll();
    if (dirty[STATUS_ROW]) {
        dirty[STATUS_ROW] = 0;
        draw_status();
    }
    if (dirty[ENTRY_ROW]) {
        dirty[ENTRY_ROW] = 0;
        draw_entry_row();
    }
}

int main(void)
{
    term_cmd(TERM_CURSOR_OFF);
    term_color(C_TEXT, C_BG);
    term_cmd(TERM_CLEAR);
    page_background(28, 28, 28, 255);
    load_subscriptions();
    load_names();
    load_timeline(0);
    id_handle = content_identity_hints(&id_req, own_hints, sizeof own_hints);
    dirty[STATUS_ROW] = 1;
    /* Started with a blob (by the shell): it is shown as an article, and
     * Esc goes to the timeline. */
    if (terse_argument(pr_id) == 1) {
        art_return = TIMELINE;
        open_blob(pr_id, 0, -1, FETCHING);
    }
    for (;; frame_n++) {
        now = terse_time();
        if (handle_events())
            break;
        render();
        terse_wait_frame();
    }
    page_clear();
    page_region(0, 0, 0, 0);
    term_color(C_TEXT, 0);
    term_cmd(TERM_CLEAR);
    term_cmd(TERM_CURSOR_ON);
    return 0;
}
