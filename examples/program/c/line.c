#include "terse.h"

static uint8_t list[64];

int main(void)
{
    int x = 320;
    terse_event e;
    for (;;) {
        while (terse_event_pop(&e)) {
            if (e.type != EV_KEY_DOWN)
                continue;
            if (e.a == KEY_LEFT)
                x -= 10;
            if (e.a == KEY_RIGHT)
                x += 10;
            if (e.a == KEY_ESCAPE)
                return 0;
        }
        vec_begin(list, sizeof list);
        vec_move(320 * VEC_UNIT, 400 * VEC_UNIT);
        vec_line(x * VEC_UNIT, 80 * VEC_UNIT);
        vec_end();
        vec_submit_list(list);
        terse_wait_frame();
    }
}
