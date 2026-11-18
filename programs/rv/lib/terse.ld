/* Linker script for Terse profile 2 (RV-PROFILE.md section 2): code and
 * constants in ROM from 00010000h, data and the stack in RAM from
 * 00100000h. The image's RAM size is set by `terse pack --ram-pages`; the
 * build checks that .bss ends well below it (the stack grows down from the
 * end of RAM). */

OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY
{
    ROM (rx) : ORIGIN = 0x00010000, LENGTH = 128K
    RAM (rw) : ORIGIN = 0x00100000, LENGTH = 1M
}

SECTIONS
{
    .text : {
        KEEP(*(.text.start))
        *(.text .text.*)
    } > ROM
    .rodata : ALIGN(4) {
        *(.rodata .rodata.* .srodata .srodata.*)
    } > ROM
    .data : ALIGN(4) {
        __data_start = .;
        *(.data .data.* .sdata .sdata.*)
        . = ALIGN(4);
        __data_end = .;
    } > RAM AT > ROM
    __data_load = LOADADDR(.data);
    .bss (NOLOAD) : ALIGN(4) {
        *(.bss .bss.* .sbss .sbss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > RAM
    /DISCARD/ : { *(.comment) *(.note .note.*) *(.riscv.attributes) *(.eh_frame*) }
}
