/* conform-perf.c - the machine host's responsiveness requirements that a
 * program can see (PERFORMANCE.md, part 2), checked by the host's own
 * frame loop.
 *
 * 1. A put of 256 bytes (not timed: with relays it waits for them), then a
 *    get of the blob just put, which the host now holds: content-done
 *    within 2 frames of the submit (spec/content.md §content.submit).
 * 2. A timeline request, which needs no network: within 2 frames.
 * 3. The frame rate: frames counted between changes of the clock
 *    (unix_time) over four whole seconds: 59 to 61 each (SPEC §frames: 60
 *    frames a second, no frames run to catch up). A headless run, whose
 *    clock stands still, says so and checks nothing.
 *
 * Run it in the host's real frame loop (a window, a self-test) for the
 * timings to mean anything; headless, every request completes in the
 * next frame by definition. Esc exits. */

#include <stdint.h>
#include "terse.h"

#define MAX_FRAMES 2
#define SECONDS 4
/* Frames to wait for the clock to change before deciding it stands still. */
#define CLOCK_WAIT 130

static content_req put_req, get_req, tl_req;
static uint8_t put_handle, get_handle, tl_handle;
static uint8_t put_data[256];
static uint8_t back[256];
static uint8_t records[256];
static uint32_t get_at, tl_at;
static uint8_t fails;

static void print_dec(uint32_t v)
{
    char out[10], *p = out + sizeof out;
    do {
        *--p = '0' + v % 10;
        v /= 10;
    } while (v);
    term_write(p, out + sizeof out - p);
}

static void verdict(int pass)
{
    term_fg(pass ? 10 : 9);
    term_puts(pass ? "PASS" : "FAIL");
    term_fg(7);
    term_puts("\n");
    if (!pass)
        fails++;
}

/* "<what>: done in N frames  PASS" (or the status if it failed). */
static void timed(const char *what, uint8_t status, uint32_t frames)
{
    term_puts(what);
    if (status != STATUS_DONE) {
        term_puts(": status ");
        print_dec(status);
        term_puts("  ");
        verdict(0);
        return;
    }
    term_puts(": done in ");
    print_dec(frames);
    term_puts(frames == 1 ? " frame (at most 2)  " : " frames (at most 2)  ");
    verdict(frames <= MAX_FRAMES);
}

int main(void)
{
    terse_event ev;
    term_color(15, 4);
    term_puts(" Tersenet · conform-perf");
    term_cmd(TERM_CLEAR_EOL);
    term_color(7, 0);
    term_puts("\n\n");

    for (uint32_t i = 0; i < sizeof put_data; i++)
        put_data[i] = (uint8_t)(i * 7 + 3);
    put_handle = content_put(&put_req, put_data, sizeof put_data);
    tl_at = terse_frame();
    tl_handle = content_feed(&tl_req, CONTENT_TIMELINE, 0, sizeof records, records);
    uint8_t content_left = 2;

    /* The clock: the frame and time of the last change seen. */
    uint32_t start = terse_frame(), last_time = terse_time(), changes = 0, last_change = 0;
    uint32_t per_second[SECONDS];
    uint8_t clock_done = 0;

    for (;;) {
        terse_wait_frame();
        uint32_t now = terse_frame();
        while (terse_event_pop(&ev)) {
            if (ev.type == EV_KEY_DOWN && ev.a == KEY_ESCAPE)
                return 0;
            if (ev.type != EV_CONTENT_DONE)
                continue;
            if (ev.a == put_handle) {
                put_handle = 0;
                if (put_req.status != STATUS_DONE) {
                    term_puts("held get: not checked (the put was refused or failed)\n");
                    content_left--;
                    continue;
                }
                get_at = now;
                get_handle = content_get(&get_req, put_req.id, 0, sizeof back, back);
                if (!get_handle) {
                    term_puts("held get: submit rejected  ");
                    verdict(0);
                    content_left--;
                }
            } else if (ev.a == get_handle) {
                get_handle = 0;
                uint8_t ok = get_req.status;
                for (uint32_t i = 0; ok == STATUS_DONE && i < sizeof back; i++)
                    if (back[i] != put_data[i])
                        ok = STATUS_FAILED;
                timed("held get (the blob just put)", ok, now - get_at);
                content_left--;
            } else if (ev.a == tl_handle) {
                tl_handle = 0;
                timed("timeline", tl_req.status, now - tl_at);
                content_left--;
            }
        }
        if (!clock_done) {
            uint32_t t = terse_time();
            if (t != last_time) {
                if (changes > 0 && changes <= SECONDS)
                    per_second[changes - 1] = now - last_change;
                changes++;
                last_change = now;
                last_time = t;
            }
            if (changes == 0 && now - start >= CLOCK_WAIT) {
                term_puts("frame rate: the clock stands still (a headless run): not checked\n");
                clock_done = 1;
            } else if (changes > SECONDS) {
                term_puts("frame rate:");
                uint8_t pass = 1;
                for (int i = 0; i < SECONDS; i++) {
                    term_puts(" ");
                    print_dec(per_second[i]);
                    if (per_second[i] < 59 || per_second[i] > 61)
                        pass = 0;
                }
                term_puts(" frames a second (59 to 61)  ");
                verdict(pass);
                clock_done = 1;
            }
        }
        if (clock_done == 1 && !content_left) {
            term_puts(fails ? "\nSome requirements failed." : "\nEvery requirement met.");
            term_puts(" Esc: exit.");
            clock_done = 2;
        }
    }
}
