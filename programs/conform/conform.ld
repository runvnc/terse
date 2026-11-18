/* Linker script for the conformance image: ROM from 00010000h; data, bss
 * and the stack in the first 112 KiB of its 128 KiB of RAM (16 pages); the
 * last 16 KiB (TOP in conform.c) is left for the checks. */

OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY
{
    ROM (rx) : ORIGIN = 0x00010000, LENGTH = 128K
    RAM (rw) : ORIGIN = 0x00100000, LENGTH = 112K
}

__stack_top = 0x0011C000;

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
    __rom_end = __data_load + SIZEOF(.data);
    .bss (NOLOAD) : ALIGN(4) {
        *(.bss .bss.* .sbss .sbss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > RAM
    /DISCARD/ : { *(.comment) *(.note .note.*) *(.riscv.attributes) *(.eh_frame*) }
}
