#include "terse.h"

static const uint8_t built_in[32];

int main(void)
{
    uint32_t hello = mach_start_named(built_in, "hello", 5);
    mach_place(hello, 0, 0, 640, 480);
    mach_focus(hello);
    for (;;)
        terse_wait_frame();
}
