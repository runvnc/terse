; hello.asm - Terse demo program (SPEC v0, machine profile 1).
;
; Standard Zilog syntax. Build with build.sh (`terse asm` + `terse pack`).
; Shows a title bar, some UTF-8 text, palette entries 0-15 written straight
; into cell memory, and a frame counter; echoes typed text; Esc exits.

; Device 1, system (SPEC 5)
SYS_EVENT   equ 10h     ; read: head event type; write: remove head
SYS_A       equ 11h
SYS_B       equ 12h
SYS_C       equ 13h
SYS_FRAME   equ 14h     ; 14h-17h, little-endian
SYS_EXIT    equ 1Fh

; Device 2, terminal (SPEC 6)
TERM_PUT    equ 20h
TERM_COL    equ 21h
TERM_ROW    equ 22h
TERM_FG     equ 23h
TERM_BG     equ 24h
TERM_CMD    equ 25h
TERM_PTRL   equ 26h
TERM_PTRH   equ 27h
TERM_DATA   equ 28h

CMD_CLEAR_EOL equ 04h

EV_KEY_DOWN equ 01h
EV_TEXT     equ 03h

KEY_ENTER   equ 28h     ; USB HID usage IDs
KEY_ESCAPE  equ 29h
KEY_BACKSPACE equ 2Ah

PALETTE_ROW equ 7
FRAME_ROW   equ 9
FRAME_COL   equ 8
FRAME_CELL  equ (FRAME_ROW*80+FRAME_COL)*4
PALETTE_CELL equ (PALETTE_ROW*80+2)*4

        org 0
        di
        ld sp,0                 ; first push lands at FFFEh (RAM page 83h)
        jp start

        org 38h
; IM 1 frame interrupt: nothing to do but wake the main loop from HALT.
irq:    ei
        reti

start:  im 1

        ; Title bar: white on blue across the whole of row 0.
        ld a,15
        out (TERM_FG),a
        ld a,4
        out (TERM_BG),a
        ld hl,title
        call print
        ld a,CMD_CLEAR_EOL
        out (TERM_CMD),a

        ; Text lines: each entry is row, column, fg, bg, zero-terminated UTF-8.
        ld hl,texts
text_loop:
        ld a,(hl)
        cp 0FFh
        jr z,texts_done
        out (TERM_ROW),a
        inc hl
        ld a,(hl)
        out (TERM_COL),a
        inc hl
        ld a,(hl)
        out (TERM_FG),a
        inc hl
        ld a,(hl)
        out (TERM_BG),a
        inc hl
        call print
        jr text_loop
texts_done:

        ; Palette row: three cells per entry, background = entry, written
        ; directly through the cell pointer.
        ld a,PALETTE_CELL & 0FFh
        out (TERM_PTRL),a
        ld a,PALETTE_CELL >> 8
        out (TERM_PTRH),a
        ld b,0
pal_loop:
        ld a,' '
        call pal_cell
        ld a,b
        call hex_digit
        call pal_cell
        ld a,' '
        call pal_cell
        inc b
        ld a,b
        cp 16
        jr nz,pal_loop

        ; Prompt; typed text appears after it.
        ld a,11
        out (TERM_ROW),a
        xor a
        out (TERM_COL),a
        out (TERM_BG),a
        ld a,10
        out (TERM_FG),a
        ld hl,prompt
        call print
        ld a,15
        out (TERM_FG),a

        ei
main:   halt                    ; one pass per frame
        call show_frame
event_loop:
        in a,(SYS_EVENT)
        or a
        jr z,main
        ld b,a
        in a,(SYS_A)
        ld e,a
        in a,(SYS_B)
        ld d,a
        in a,(SYS_C)
        ld c,a
        out (SYS_EVENT),a       ; remove the head event
        ld a,b
        cp EV_TEXT
        jr nz,not_text
        call put_code_point
        jr event_loop
not_text:
        cp EV_KEY_DOWN
        jr nz,event_loop
        ld a,e
        cp KEY_ESCAPE
        jr z,quit
        cp KEY_ENTER
        jr nz,not_enter
        ld hl,newline
        call print
        jr event_loop
not_enter:
        cp KEY_BACKSPACE
        jr nz,event_loop
        ld hl,rubout
        call print
        jr event_loop

quit:   xor a
        out (SYS_EXIT),a
        di
        halt

; Prints the zero-terminated string at HL; returns HL past the terminator.
print:  ld a,(hl)
        inc hl
        or a
        ret z
        out (TERM_PUT),a
        jr print

; Writes one palette cell at the cell pointer: character A, background B,
; foreground white on the dark entries 0-6 and black on the rest.
pal_cell:
        out (TERM_DATA),a
        xor a
        out (TERM_DATA),a
        ld a,b
        cp 7
        ld a,15
        jr c,pal_fg
        xor a
pal_fg: out (TERM_DATA),a
        ld a,b
        out (TERM_DATA),a
        ret

; Returns the ASCII hex digit for the low nibble of A.
hex_digit:
        and 0Fh
        add a,'0'
        cp '9'+1
        ret c
        add a,7
        ret

; Writes the 32-bit frame counter as eight hex digits into cell memory.
show_frame:
        ld a,FRAME_CELL & 0FFh
        out (TERM_PTRL),a
        ld a,FRAME_CELL >> 8
        out (TERM_PTRH),a
        ld c,SYS_FRAME+3        ; most significant byte first
frame_loop:
        in a,(c)
        push af
        rrca
        rrca
        rrca
        rrca
        call frame_digit
        pop af
        call frame_digit
        dec c
        ld a,c
        cp SYS_FRAME-1
        jr nz,frame_loop
        ret

frame_digit:
        call hex_digit
        out (TERM_DATA),a
        xor a
        out (TERM_DATA),a
        ld a,14
        out (TERM_DATA),a
        xor a
        out (TERM_DATA),a
        ret

; Writes the code point C:D:E (bits 16-20, 8-15, 0-7) to the terminal as
; UTF-8. Code points above FFFFh are written as U+FFFD.
put_code_point:
        ld a,c
        or a
        jr nz,put_replacement
        ld a,d
        or a
        jr nz,put_wide
        ld a,e
        cp 80h
        jr nc,put_two
        out (TERM_PUT),a
        ret
put_wide:
        cp 08h
        jr nc,put_three
put_two:                        ; 110yyyyy 10xxxxxx, y = DE >> 6
        ld a,d
        add a,a
        add a,a
        ld h,a
        ld a,e
        rlca
        rlca
        and 03h
        or h
        or 0C0h
        out (TERM_PUT),a
        jr put_last
put_three:                      ; 1110zzzz 10yyyyyy 10xxxxxx
        ld a,d
        rrca
        rrca
        rrca
        rrca
        and 0Fh
        or 0E0h
        out (TERM_PUT),a
        ld a,d
        and 0Fh
        add a,a
        add a,a
        ld h,a
        ld a,e
        rlca
        rlca
        and 03h
        or h
        or 80h
        out (TERM_PUT),a
put_last:
        ld a,e
        and 3Fh
        or 80h
        out (TERM_PUT),a
        ret
put_replacement:
        ld hl,replacement
        jp print

title:  db " Terse · hello", 0
texts:  db 2, 2, 11, 0, "héllo, wörld — 你好", 0
        db 3, 2, 7, 0, "Ελληνικά · кириллица · ┌─┬─┐ · ♥ ★ ✓", 0
        db 4, 2, 8, 0, "Type to echo text below. Enter: new line, Backspace, Esc: exit.", 0
        db 6, 2, 7, 0, "Palette entries 0-15, written through the cell pointer:", 0
        db FRAME_ROW, 2, 7, 0, "frame", 0
        db 0FFh
prompt: db "> ", 0
newline: db 0Ah, "> ", 0
rubout: db 08h, " ", 08h, 0
replacement: db 0EFh, 0BFh, 0BDh, 0
