# print.asm - a separate module, assembled on its own and joined to hello
# by the linker. Only `print` is exported.

    .include "terse.inc"

    register string    = arg0
    register length    = arg1
    register scan      = temp0
    register character = temp1

    .text
    .globl print

# print: write the zero-terminated string at `string` (arg0) to the terminal.
print:
    copy                scan, string
find_end:
    load_byte_unsigned  character, 0(scan)
    add_constant        scan, scan, 1
    branch_if_not_zero  character, find_end
    subtract            length, scan, string
    add_constant        length, length, -1      # without the terminator
    terse               TERM_WRITE              # string, length
    return
