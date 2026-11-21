/* hello.c: hello.S in C, with programs/lib (terse.h, terse.c, crt0.S).
 *
 * Shows a title bar, some UTF-8 text, palette entries 0-15 written straight
 * into cell memory, and a frame counter; echoes typed text; Esc exits. The
 * same screen as hello.S (expected-30-frames.txt). Build it with
 *     terse build hello.c
 * (hello.terse is built from hello.S by build.sh). */

#include "terse.h"

static const struct {
    uint8_t row, col, fg;
    const char *text;
} texts[] = {
    {2, 2, 11, "héllo, wörld — привет, мир"},
    {3, 2, 7, "Ελληνικά · кириллица · ┌─┬─┐ · ♥ ★ ✓"},
    {4, 2, 8, "Type to echo text below. Enter: new line, Backspace, Esc: exit."},
    {6, 2, 7, "Palette entries 0-15, written with cells_write:"},
    {9, 2, 7, "frame"},
};

static uint32_t cell(uint16_t ch, uint8_t fg, uint8_t bg)
{
    return ch | (uint32_t)fg << 16 | (uint32_t)bg << 24;
}

static uint16_t hex_digit(uint32_t n)
{
    return "0123456789ABCDEF"[n & 15];
}

int main(void)
{
    uint32_t cells[48];

    /* Title bar: white on blue across the whole of row 0. */
    term_color(15, 4);
    term_puts(" Tersenet · hello");
    term_cmd(TERM_CLEAR_EOL);

    for (unsigned i = 0; i < sizeof texts / sizeof texts[0]; i++) {
        term_goto(texts[i].col, texts[i].row);
        term_color(texts[i].fg, 0);
        term_puts(texts[i].text);
    }

    /* Palette row: three cells per entry, background = entry; white text
     * on entries 0-6, black on the rest. */
    for (uint32_t e = 0; e < 16; e++) {
        uint8_t fg = e < 7 ? 15 : 0;
        cells[3 * e] = cells[3 * e + 2] = cell(' ', fg, e);
        cells[3 * e + 1] = cell(hex_digit(e), fg, e);
    }
    term_cells_write(7 * 80 + 2, cells, 48);

    /* Prompt; typed text appears after it. */
    term_goto(0, 11);
    term_color(10, 0);
    term_puts("> ");
    term_fg(15);

    for (;;) {
        terse_event e;
        uint32_t n;

        terse_wait_frame();
        /* The frame counter as eight hex digits, yellow on black. */
        n = terse_frame();
        for (int i = 0; i < 8; i++)
            cells[i] = cell(hex_digit(n >> (28 - 4 * i)), 14, 0);
        term_cells_write(9 * 80 + 8, cells, 8);

        while (terse_event_pop(&e)) {
            if (e.type == EV_TEXT)
                term_put_cp(EV_TEXT_CP(e));
            else if (e.type == EV_KEY_DOWN && e.a == KEY_ESCAPE)
                return 0;
            else if (e.type == EV_KEY_DOWN && e.a == KEY_ENTER)
                term_puts("\n> ");
            else if (e.type == EV_KEY_DOWN && e.a == KEY_BACKSPACE)
                term_write("\b \b", 3);
        }
    }
}
