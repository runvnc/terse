# A 32-bit instruction whose second half would lie past the end of ROM.
#include "common.inc"
    START
    j last
    .balign 4
last: .half 0x0513
