/* fetch.c - Terse content device demo (SPEC §content).
 *
 * Gets the blob whose ID build.sh writes to message_id.h (the BLAKE3-256 of
 * message.txt), shows its status, size and text, then puts a 256-byte
 * buffer built in RAM and shows the start of the ID the host returns. Esc
 * exits. */

#include <stdint.h>
#include "terse.h"
#include "message_id.h"

#define GET_MAX 4096
#define PUT_SIZE 256

static content_req get_req, put_req;
static uint8_t get_handle, put_handle;
static uint8_t get_data[GET_MAX];
static uint8_t put_data[PUT_SIZE];

static const char *const status_names[] = {
    "pending",
    "done",
    "not found (not in the store or on the relay)",
    "failed (network error or timeout)",
    "bad request",
    "refused (the user or the host did not allow it)",
    "no room",
    "unknown",
};

static void print_hex(const uint8_t *b, uint32_t n)
{
    static const char digits[] = "0123456789abcdef";
    char out[64];
    for (uint32_t i = 0; i < n; i++) {
        out[2 * i] = digits[b[i] >> 4];
        out[2 * i + 1] = digits[b[i] & 15];
    }
    term_write(out, 2 * n);
}

static void print_dec(uint32_t v)
{
    char out[10], *p = out + sizeof out;
    do {
        *--p = '0' + v % 10;
        v /= 10;
    } while (v);
    term_write(p, out + sizeof out - p);
}

/* "status NN name" and a new line: green for done, red otherwise. */
static void print_status(uint8_t status)
{
    term_puts("status ");
    print_hex(&status, 1);
    term_puts(" ");
    term_fg(status == STATUS_DONE ? 10 : 9);
    term_puts(status_names[status < 7 ? status : 7]);
    term_fg(7);
    term_puts("\n");
}

static void start_put(void)
{
    for (uint32_t i = 0; i < PUT_SIZE; i++)
        put_data[i] = i;
    put_req.op = CONTENT_PUT;
    put_req.addr = (uint32_t)put_data;
    put_req.size = PUT_SIZE;
    term_puts("\nPutting 256 bytes built in RAM… ");
    put_handle = content_submit(&put_req);
    if (!put_handle)
        term_puts("submit rejected\n");
}

/* The get finished: status, size and text, then start the put. */
static void get_done(void)
{
    print_status(get_req.status);
    if (get_req.status == STATUS_DONE) {
        term_puts("size: ");
        print_dec(get_req.size);
        term_puts(" bytes. Text:\n\n");
        term_write(get_data, get_req.length);
    }
    start_put();
}

/* The put finished: status and the start of the ID. */
static void put_done(void)
{
    print_status(put_req.status);
    if (put_req.status == STATUS_DONE) {
        term_puts("blob ID: ");
        print_hex(put_req.id, 8);
        term_puts("…\n");
    }
    term_puts("\nEsc: exit.");
}

int main(void)
{
    terse_event ev;
    term_color(15, 4);
    term_puts(" Terse · fetch");
    term_cmd(TERM_CLEAR_EOL);
    term_color(7, 0);

    term_puts("\n\nGetting blob ");
    print_hex(message_id, 8);
    term_puts("… (message.txt)\n");
    get_handle = content_get(&get_req, message_id, 0, GET_MAX, get_data);
    term_puts(get_handle ? "fetching… " : "submit rejected\n");

    for (;;) {
        terse_wait_frame();
        while (terse_event_pop(&ev)) {
            if (ev.type == EV_KEY_DOWN && ev.a == KEY_ESCAPE)
                return 0;
            if (ev.type != EV_CONTENT_DONE)
                continue;
            /* Handles are reused once freed, so each is cleared when its
             * request ends. */
            if (ev.a == get_handle) {
                get_handle = 0;
                get_done();
            } else if (ev.a == put_handle) {
                put_handle = 0;
                put_done();
            }
        }
    }
}
