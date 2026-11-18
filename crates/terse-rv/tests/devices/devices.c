/* Exercises the profile-2 device calls; prints one "name value" line each
 * (value in hex). crates/terse-rv/tests/devices.rs checks them. */

#include "terse.h"

static char out[16];
static void line(const char *name, uint32_t v)
{
    int k;
    term_puts(name);
    for (k = 0; k < 8; k++)
        out[k] = "0123456789abcdef"[v >> (28 - 4 * k) & 15];
    out[8] = '\n';
    term_write(out, 9);
}

static const uint8_t rom_list[8] = {VEC_OP_MOVE, 1, 0, 2, 0, VEC_OP_END};
static const char rom_text[] = "put from ROM";
static const uint8_t id[32] = {1, 2, 3};
static content_req get, put, bad;
static uint8_t data[16], list[16], rnd[5];
static uint32_t cells[2];
static const uint32_t cells_src[2] = {0x01020041, 0x03040042};
static const uint8_t pal[6] = {1, 2, 3, 4, 5, 6};
static terse_pad pad;

int main(void)
{
    terse_event e;
    uint32_t r, a;
    term_goto(0, 1);
    line("unknown  ", terse_call(0x999, 0, 0, 0));
    line("absent   ", terse_call(0x500, 0, 0, 0));
    line("ev-null  ", terse_call(CALL_NEXT_EVENT, 0, 0, 0));
    line("ev-rom   ", terse_call(CALL_NEXT_EVENT, TERSE_ROM_BASE, 0, 0));
    line("wr-bad   ", terse_call(CALL_TERM_WRITE, 0x200000, 5, 0));
    vec_begin(list);
    vec_move(1, 2);
    vec_disc(3);
    line("vec      ", vec_submit_list(list));
    line("vec-rom  ", vec_submit(rom_list, 6));
    line("vec-zero ", vec_submit(list, 0));
    term_cells_write(2399, cells_src, 2); /* wraps to cell 0 */
    term_cells_read(2399, cells, 2);
    line("cells    ", cells[0] ^ cells[1]);
    line("cellsbad ", terse_call(CALL_TERM_CELLS_READ, 2400, (uint32_t)cells, 1));
    line("palette  ", terse_call(CALL_TERM_PALETTE_SET, 255, (uint32_t)pal, 2));
    line("pad0     ", terse_call(CALL_CONTROLLER, 0, (uint32_t)&pad, 0) | (uint32_t)pad.buttons << 8 | (uint8_t)pad.lx << 16);
    line("pad9     ", terse_call(CALL_CONTROLLER, 9, (uint32_t)&pad, 0) | pad.buttons);
    terse_random(rnd, 5);
    line("random   ", rnd[0] | (uint32_t)rnd[4] << 8);
    a = terse_call2(CALL_TERM_SIZE, 0, 0, &r);
    line("size     ", a | r << 8);
    term_goto(200, 7); /* clamped to column 79 */
    a = terse_call2(CALL_TERM_GET_CURSOR, 0, 0, &r);
    term_goto(0, 20);
    line("cursor   ", a | r << 8);
    line("get      ", content_get(&get, id, 3, 10, data));
    put.op = CONTENT_PUT;
    put.addr = (uint32_t)rom_text;
    put.size = sizeof rom_text - 1;
    line("put      ", content_submit(&put));
    bad.op = CONTENT_GET;
    bad.length = 10;
    bad.addr = TERSE_ROM_BASE; /* a get must deliver to RAM */
    line("bad      ", content_submit(&bad) | (uint32_t)bad.status << 8);
    line("blk-rom  ", terse_call(CALL_CONTENT_SUBMIT, TERSE_ROM_BASE, 0, 0));
    while (terse_event_pop(&e) != EV_CONTENT_DONE)
        terse_wait_frame();
    line("done     ", e.a | (uint32_t)e.b << 8 | get.status << 16);
    line("got      ", get.length | get.size << 16);
    line("data     ", data[0] | (uint32_t)data[9] << 8 | (uint32_t)data[10] << 16);
    while (terse_event_pop(&e) != EV_CONTENT_DONE)
        terse_wait_frame();
    line("put-done ", e.a | (uint32_t)put.status << 8 | (uint32_t)put.id[0] << 16);
    return 7;
}
