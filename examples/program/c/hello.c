#include "terse.h"

int main(void)
{
    term_color(15, 4);
    term_puts(" Hello from hello! ");
    term_color(7, 0);
    term_puts("\n\nPress Esc to exit.\n");

    for (;;) {
        terse_event e;
        terse_wait_frame();
        while (terse_event_pop(&e)) {
            if (e.type == EV_KEY_DOWN && e.a == KEY_ESCAPE)
                return 0;
        }
    }
}
