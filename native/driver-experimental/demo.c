/* demo.c: a Tersenet program that uses an invented device, example.dice
 * (dice.c, spec/experimental.md), when the host grants it, and says so
 * on the terminal when it does not. Build it with the device's name:
 *
 *     terse build demo.c --optional 240:0=example.dice
 *     terse-bus demo.terse --driver example.dice=$PWD/dice --frames 120 --dump
 *     terse run demo.terse --frames 10 --dump         (no dice here)
 *
 * Slot 240 is only this image's handle for the device: the name in the
 * image is what a host matches to a driver. Another image may call the
 * same device on another slot, and both may run at once in a shell:
 *
 *     terse build demo.c --cflag -DDICE=241 --optional 241:0=example.dice */

#include "terse.h"

#ifndef DICE
#define DICE 240                        /* the slot --optional gave it */
#endif
#define DICE_SEED (DICE << 8 | 0)
#define DICE_ROLL (DICE << 8 | 1)
#define DICE_HISTORY (DICE << 8 | 2)

static void put_num(uint32_t v)
{
    char b[11];
    int i = 10;
    b[i] = 0;
    do b[--i] = '0' + v % 10; while (v /= 10);
    term_puts(b + i);
}

static void put_hex(uint32_t v)
{
    char b[9];
    for (int i = 0; i < 8; i++) b[i] = "0123456789ABCDEF"[v >> (28 - 4 * i) & 15];
    b[8] = 0;
    term_puts(b);
}

int main(void)
{
    uint32_t version = terse_device(DICE);
    term_color(15, 4);
    term_puts(" example.dice: an experimental device \n");
    term_color(7, 0);
    if (version == TERSE_ERR_UNKNOWN) {
        /* Not granted: this host has no driver for it (terse run never
         * has one). Everything else works as usual. */
        term_puts("\nNot granted here: no driver for example.dice.\n");
        term_puts("Run me on terse-bus with its driver to roll dice.\n");
        term_puts("A call to it now gives ");
        put_hex(terse_call(DICE_ROLL, 6, 0, 0));
        term_puts(".\n");
        for (;;) terse_wait_frame();
    }
    term_puts("\nGranted, version ");
    put_num(version);
    term_puts(", on slot ");
    put_num(DICE);
    term_puts(".\n");

    /* Chance from the machine's own random bytes, which the host
     * records: a replay rolls the same dice. (A headless run gives
     * zeros, so its dice are always the same.) */
    uint32_t seed;
    terse_random(&seed, 4);
    terse_call(DICE_SEED, seed, 0, 0);
    term_puts("roll(300) = ");
    put_hex(terse_call(DICE_ROLL, 300, 0, 0));     /* more sides than 255: a bad argument */
    term_puts("\n\n");

    for (uint32_t turn = 1;; turn++) {
        for (int f = 0; f < 30; f++) terse_wait_frame();
        uint32_t a = terse_call(DICE_ROLL, 6, 0, 0), b = terse_call(DICE_ROLL, 6, 0, 0), c = terse_call(DICE_ROLL, 6, 0, 0);
        term_puts("turn ");
        put_num(turn);
        term_puts(": ");
        put_num(a); term_puts(" + "); put_num(b); term_puts(" + "); put_num(c);
        term_puts(" = ");
        put_num(a + b + c);
        uint8_t last[8];
        uint32_t rolls = terse_call(DICE_HISTORY, (uint32_t)last, 8, 0);
        term_puts("   rolls ");
        put_num(rolls);
        term_puts(", last 8:");
        for (int i = 0; i < 8; i++) { term_puts(" "); put_num(last[i]); }
        term_puts("\n");
    }
}
