#include "terse.h"

static uint8_t list[64];

static int near(int a, int b)
{
    return a - b < 12 && b - a < 12;
}

static void put_number(int n)
{
    if (n > 9)
        put_number(n / 10);
    term_putc('0' + n % 10);
}

static void show_score(int score)
{
    term_goto(0, 0);
    term_puts("Score ");
    put_number(score);
}

int main(void)
{
    int x = 320, y = 240, dx = 0, dy = 0;
    int goal_x = 160, goal_y = 120, score = 0;
    terse_event e;
    show_score(score);
    for (;;) {
        while (terse_event_pop(&e)) {
            if (e.type != EV_KEY_DOWN)
                continue;
            if (e.a == KEY_LEFT)
                dx = -2, dy = 0;
            if (e.a == KEY_RIGHT)
                dx = 2, dy = 0;
            if (e.a == KEY_UP)
                dx = 0, dy = -2;
            if (e.a == KEY_DOWN)
                dx = 0, dy = 2;
            if (e.a == KEY_ESCAPE)
                return 0;
        }
        x = (x + dx + 640) % 640;
        y = (y + dy + 480) % 480;
        if (near(x, goal_x) && near(y, goal_y)) {
            show_score(++score);
            goal_x = 40 + terse_random_byte() * 2;
            goal_y = 40 + terse_random_byte() * 3 / 2;
        }
        vec_begin(list, sizeof list);
        vec_move(x * 16, y * 16);
        vec_disc(4 * 16);
        vec_move(goal_x * 16, goal_y * 16);
        vec_circle(10 * 16);
        vec_end();
        vec_submit_list(list);
        terse_wait_frame();
    }
}
