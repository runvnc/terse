/* text.c: a full terminal screen of text (80 x 30 cells) in the phosphor
 * style, to judge how legible stroke text is: a box-drawing frame, prose,
 * every printable ASCII character, Latin-1 letters and signs, and the
 * other marks of the basic repertoire. No input. */

#include "terse.h"

static const char *const lines[] = {
    "  Reading on a vector display",
    "",
    "  A vector monitor draws only what is there: the beam moves from point to",
    "  point and lights the phosphor where it passes, so a letter is a handful",
    "  of strokes and the screen between the letters stays black. Text on such",
    "  a screen was drawn by a character generator, which turned each code into",
    "  a short list of moves for the beam, and it looked like this one.",
    "",
    "  This screen has eighty columns and thirty rows of cells, eight by sixteen",
    "  screen units each. The question it asks is simple: can you read it at",
    "  640 by 480 pixels, and at twice and three times that?",
    "",
    "  ASCII:  !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_",
    "          `abcdefghijklmnopqrstuvwxyz{|}~",
    "  Latin-1: àáâãäå æ ç èéêë ìíîï ð ñ òóôõö ø ùúûü ýÿ þ ß",
    "           ÀÁÂÃÄÅ Æ Ç ÈÉÊË ÌÍÎÏ Ð Ñ ÒÓÔÕÖ Ø ÙÚÛÜ Ý Þ",
    "  Signs:   ¡ ¢ £ ¤ ¥ ¦ § ¨ © ª « ¬ ® ¯ ° ± ² ³ ´ µ ¶ · ¸ ¹ º » ¼ ½ ¾ ¿ × ÷",
    "  Marks:   – — ‘ ’ “ ” • … − \xef\xbf\xbd",
    "  Blocks:  ░▒▓█ ▀▄▌▐ ▖▗▘▙▚▛▜▝▞▟   ╭─╮ ┏━┓ ╔═╗ ┄┅┆ ╱╲╳",
    "                                  ╰─╯ ┗━┛ ╚═╝",
    "",
    "  café, naïve, Ærøskøbing, Straße, señor, garçon: 12 °C, ½ £, ¿qué?",
};

int main(void)
{
    vec_style(VEC_STYLE_PHOSPHOR);
    term_cmd(TERM_CURSOR_OFF);
    /* The frame. */
    term_color(11, 0);
    term_goto(0, 0);
    term_puts("┌");
    for (int i = 1; i < 79; i++)
        term_puts("─");
    term_puts("┐");
    for (int r = 1; r < 29; r++) {
        term_goto(0, r);
        term_puts("│");
        term_goto(79, r);
        term_puts("│");
    }
    term_goto(0, 29);
    term_puts("└");
    for (int i = 1; i < 79; i++)
        term_puts("─");
    term_puts("┘");
    for (unsigned i = 0; i < sizeof lines / sizeof lines[0]; i++) {
        term_goto(1, 2 + i);
        term_color(i == 0 ? 15 : 7, 0);
        term_puts(lines[i]);
    }
    term_goto(3, 26);
    term_color(10, 0);
    term_puts("> the quick brown fox jumps over the lazy dog_");
    for (;;)
        terse_wait_frame();
}
