;; crt0 for Terse programs built with SDCC (sdasz80 syntax, sdcccall(1)).
;;
;; Memory layout (SPEC 1.1, 1.2: at reset slots 0-3 show ROM pages 00h-03h
;; and slots 4-7 RAM pages 80h-83h; RAM is zero-filled):
;;
;;   0000h  _HEADER  reset entry; 0038h RST 38h frame interrupt handler, then
;;                   the start-up code (ROM, below 0100h)
;;   0100h  _HOME, _CODE, _INITIALIZER, _GSINIT, _GSFINAL  (ROM, --code-loc 0x0100)
;;   8000h  _DATA, _INITIALIZED, _BSEG, _BSS, _HEAP       (RAM, --data-loc 0x8000)
;;    ...   free RAM
;;   FFFFh  stack top (SP starts at 0000h, so the first push writes FFFEh-FFFFh)
;;
;; ROM can reach 7FFFh before it runs into slot 4; a program that remaps a
;; ROM slot must keep its code below that slot.

	.module crt0
	.optsdcc -mz80 sdcccall(1)
	.globl	_main
	.globl	_terse_ticks

	.area	_HEADER (ABS)
	.org	0x0000
	di
	ld	sp, #0x0000
	jp	init

	;; Interrupt mode 1 (and mode 0, since the bus reads FFh) runs RST 38h
	;; once per frame (SPEC 1.3). Count frames for terse_wait_frame.
	.org	0x0038
	push	af
	ld	a, (_terse_ticks)
	inc	a
	ld	(_terse_ticks), a
	pop	af
	ei
	reti

	;; Still in _HEADER: _CODE starts at 0100h (--code-loc), so this must
	;; stay short.
init:
	call	gsinit
	im	1
	ei
	call	_main
	;; main's int result is in DE (sdcccall(1)); the low byte is the exit
	;; code (SPEC 5: port 1Fh stops the machine).
	ld	a, e
	out	(0x1F), a
1$:	halt
	jr	1$

	;; Ordering of areas for the linker.
	.area	_HOME
	.area	_CODE
	.area	_INITIALIZER
	.area	_GSINIT
	.area	_GSFINAL

	.area	_DATA
	.area	_INITIALIZED
	.area	_BSEG
	.area	_BSS
	.area	_HEAP

	.area	_DATA
_terse_ticks::
	.ds	1

	;; RAM is zero at reset (SPEC 1.2), so _DATA and _BSS need no clearing;
	;; only explicitly initialised globals are copied from ROM.
	.area	_GSINIT
gsinit::
	ld	bc, #l__INITIALIZER
	ld	a, b
	or	a, c
	jr	Z, gsinit_next
	ld	de, #s__INITIALIZED
	ld	hl, #s__INITIALIZER
	ldir
gsinit_next:

	.area	_GSFINAL
	ret
