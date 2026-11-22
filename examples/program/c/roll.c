#include "terse.h"

#define DICE 240
#define DICE_SEED (DICE << 8 | 0)
#define DICE_ROLL (DICE << 8 | 1)

int main(void)
{
    uint32_t seed;
    if (!terse_has_device(DICE)) {
        term_puts("No dice here: run me with example.dice.\n");
        for (;;)
            terse_wait_frame();
    }
    terse_random(&seed, 4);
    terse_call(DICE_SEED, seed, 0, 0);
    for (;;) {
        for (int frame = 0; frame < 30; frame++)
            terse_wait_frame();
        term_puts("You rolled ");
        term_putc('0' + terse_call(DICE_ROLL, 6, 0, 0));
        term_puts(".\n");
    }
}
