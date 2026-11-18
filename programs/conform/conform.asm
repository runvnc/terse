; conform.asm - Terse conformance and responsiveness image (SPEC v0, profile 1).
;
; Run with conform.events (README, "Conformance image"). Each section checks
; what the program itself can observe and prints a PASS or FAIL line; the
; authoritative check is the state hash every 10 frames (expected-hashes.txt,
; STATE.md), which also covers what the program cannot see (palette, vector
; list, CPU timing details). After the checks a stress phase runs: full-budget
; frames that rewrite all 2,400 cells with wide and varied characters and
; submit a new 16,384-byte display list every frame. Then the results stay on
; the screen; Esc exits with the number of failed sections as the exit code.
;
; Frames used by the scripted input: events at 2-5 (read at 6), 70 text
; events at 8 (read at 9), controllers at 10-13 (read at 11, 12, 13).

RAM_PAGES   equ 16

; Device 1, system (SPEC 5)
SYS_EV      equ 10h
SYS_A       equ 11h
SYS_B       equ 12h
SYS_C       equ 13h
SYS_FRAME   equ 14h
SYS_TIME    equ 18h
SYS_RAND    equ 1Ch
SYS_PSEL    equ 1Dh
SYS_PAD     equ 1Eh
SYS_EXIT    equ 1Fh
; Device 2, terminal (SPEC 6)
T_PUT       equ 20h
T_COL       equ 21h
T_ROW       equ 22h
T_FG        equ 23h
T_BG        equ 24h
T_CMD       equ 25h
T_PTRL      equ 26h
T_PTRH      equ 27h
T_DATA      equ 28h
T_PALI      equ 29h
T_PALD      equ 2Ah
; Device 3, vector display (SPEC 8)
V_A0        equ 30h
V_A1        equ 31h
V_A2        equ 32h
V_L0        equ 33h
V_L1        equ 34h
V_CMD       equ 35h
; Device 4, content (SPEC 7)
C_B0        equ 40h
C_B1        equ 41h
C_SUB       equ 42h

; Variables (RAM page 80h, slot 4).
ticks       equ 8000h       ; frame interrupts taken
irq_ret     equ 8002h       ; return address of the last interrupt
halt_ret    equ 8004h
im2_seen    equ 8006h
fails       equ 8008h       ; failed checks in the current section
first_fail  equ 800Ah       ; cur_op of the first one
cur_op      equ 800Ch       ; the op (or checkpoint) being checked
extra       equ 800Eh       ; a value shown with the result
has_extra   equ 8010h
nresults    equ 8011h
rnd_state   equ 8012h
sframe      equ 8014h       ; stress frame number
cjk_next    equ 8016h
done_count  equ 8018h
want        equ 8019h
st_row      equ 801Ah
r_before    equ 801Bh
r_after     equ 801Ch
t0          equ 801Dh
f0          equ 801Eh
timeout     equ 8020h
tmp         equ 8022h
counts      equ 8030h       ; timing results, 2 bytes each
done_status equ 8050h       ; status of each content handle 1-8
results     equ 8100h       ; 10 bytes per section
PAGING_COPY equ 8400h       ; slot_test copied here to test slot 0
IM2_TABLE   equ 82FFh       ; I = 82h: the IM 2 vector is read from here
SMALL_LIST  equ 8800h       ; linear 000800h
BLOCK       equ 0A000h      ; content request blocks, linear 002000h
BLOCK2      equ 0A030h
PUT_SRC     equ 0A100h      ; linear 002100h
GET_DST     equ 0A200h      ; linear 002200h
GET_DST2    equ 0A300h      ; linear 002300h
BLOCKS8     equ 0A400h      ; nine blocks, 48 bytes apart
FEED_BUF    equ 0A800h      ; linear 002800h

STRESS_A    equ 60          ; frames with the many-primitives list
STRESS_B    equ 20          ; frames with the full-screen-discs list

; Test ops (run_ops): a compact way to write port checks.
T_OUT   macro p,v           ; out (p),v
        db 1,p,v
        endm
T_IN    macro p,v           ; in a,(p): expect v
        db 2,p,v
        endm
CH      macro col,row,code  ; cell (col,row) holds character code
        db 3
        dw ((row)*80+(col))*4
        db 2
        dw code
        endm
CELL    macro col,row,code,fg,bg ; the whole cell
        db 3
        dw ((row)*80+(col))*4
        db 4
        dw code
        db fg,bg
        endm
T_OUTN  macro p,v,n         ; out (p),v n times
        db 6,p,v,n
        endm
T_INN   macro p,n           ; in a,(p) n times, values ignored
        db 7,p,n
        endm
T_CALL  macro addr          ; call addr (it may call fail)
        db 8
        dw addr
        endm
AT      macro col,row       ; cursor to (col,row)
        db 1,T_COL,col,1,T_ROW,row
        endm
; db 5,bytes...,0 : the bytes to port 20h (put character)
; db 0 : end

        org 0
        di
        ld sp,0                 ; the stack is at the top of RAM page 83h
        jp start

        org 38h
; Frame interrupt (IM 0 and IM 1 both arrive here, SPEC 1.3): counts ticks
; and records where the interrupt returns to.
irq:    push af
        push hl
        ld hl,4
        add hl,sp
        ld a,(hl)
        inc hl
        ld h,(hl)
        ld l,a
        ld (irq_ret),hl
        ld hl,ticks
        inc (hl)
        pop hl
        pop af
        ei
        ret

; IM 2 handler, reached through IM2_TABLE.
irq2:   push af
        ld a,1
        ld (im2_seen),a
        pop af
        jr irq

start:  im 1
        ld hl,irq2
        ld (IM2_TABLE),hl
        ld hl,1
        ld (rnd_state),hl
        ld hl,title
        call header
        call t_paging
        call t_system
        ei
        ld hl,6
        call wait_until
        call t_events
        ld hl,9
        call wait_until
        call t_overflow
        call t_pads
        call t_timing
        call t_charges
        call t_fullbudget
        call reprint
        call t_terminal
        call reprint
        call t_vector
        call t_content
        call reprint
        call stress
        call final
idle:   halt
idle1:  in a,(SYS_EV)
        or a
        jr z,idle
        cp 1
        jr nz,idle2
        in a,(SYS_A)
        cp 29h                  ; Esc
        jr z,quit
idle2:  out (SYS_EV),a
        jr idle1
quit:   call failed_sections
        out (SYS_EXIT),a
        jr quit

; ---------------------------------------------------------------- output

print:  ld a,(hl)
        or a
        ret z
        out (T_PUT),a
        inc hl
        jr print

newline: ld a,0Ah
        out (T_PUT),a
        ret

hex16:  ld a,h
        call hex8
        ld a,l
hex8:   push af
        rrca
        rrca
        rrca
        rrca
        call nibble
        pop af
nibble: and 0Fh
        add a,'0'
        cp '9'+1
        jr c,nib1
        add a,7
nib1:   out (T_PUT),a
        ret

; Clears the screen and prints the title line (HL = text).
header: ld a,7
        out (T_FG),a
        xor a
        out (T_BG),a
        ld a,1
        out (T_CMD),a
        ld a,15
        out (T_FG),a
        ld a,4
        out (T_BG),a
        call print
        ld a,4
        out (T_CMD),a           ; the rest of row 0 in the title colours
        xor a
        out (T_BG),a
        ld a,7
        out (T_FG),a
        ld a,1
        out (T_ROW),a
        xor a
        out (T_COL),a
        ret

; Counts a failed check; the first one's cur_op is kept. Keeps all registers.
fail:   push af
        push hl
        ld hl,(fails)
        ld a,h
        or l
        jr nz,fail1
        ld hl,(cur_op)
        ld (first_fail),hl
        ld hl,0
fail1:  inc hl
        ld (fails),hl
        pop hl
        pop af
        ret

; Fails unless HL = DE.
expect_hl: push hl
        or a
        sbc hl,de
        pop hl
        call nz,fail
        ret

; Sets cur_op to A (a checkpoint number for sections written as code).
point:  push hl
        ld l,a
        ld h,0
        ld (cur_op),hl
        pop hl
        ret

; Records and prints the result of the section just run; DE = its name.
report: ld a,(nresults)
        call record
        ld (hl),e
        inc hl
        ld (hl),d
        inc hl
        ld de,(fails)
        ld (hl),e
        inc hl
        ld (hl),d
        inc hl
        ld de,(first_fail)
        ld (hl),e
        inc hl
        ld (hl),d
        inc hl
        ld de,(extra)
        ld (hl),e
        inc hl
        ld (hl),d
        inc hl
        ld a,(has_extra)
        ld (hl),a
        ld a,(nresults)
        inc a
        ld (nresults),a
        dec a
        call print_result
        ld hl,0
        ld (fails),hl
        ld (first_fail),hl
        ld (extra),hl
        xor a
        ld (has_extra),a
        ret

; HL = address of result record A.
record: ld l,a
        ld h,0
        add hl,hl
        ld b,h
        ld c,l
        add hl,hl
        add hl,hl
        add hl,bc
        ld bc,results
        add hl,bc
        ret

; Prints result record A as one line.
print_result:
        call record
        push hl
        pop ix
        ld a,(ix+2)
        or (ix+3)
        jr nz,pr_fail
        ld a,10
        out (T_FG),a
        ld hl,s_pass
        jr pr_name
pr_fail: ld a,9
        out (T_FG),a
        ld hl,s_fail
pr_name: call print
        ld a,7
        out (T_FG),a
        ld l,(ix+0)
        ld h,(ix+1)
        call print
        ld a,(ix+8)
        or a
        jr z,pr_nox
        ld hl,s_equals
        call print
        ld l,(ix+6)
        ld h,(ix+7)
        call hex16
pr_nox: ld a,(ix+2)
        or (ix+3)
        jr z,pr_end
        ld hl,s_fails
        call print
        ld l,(ix+2)
        ld h,(ix+3)
        call hex16
        ld hl,s_first
        call print
        ld l,(ix+4)
        ld h,(ix+5)
        call hex16
        ld a,')'
        out (T_PUT),a
pr_end: jp newline

; Clears the screen and prints every result so far.
reprint: ld hl,title
        call header
print_all:
        ld a,(nresults)
        or a
        ret z
        ld b,a
        xor a
pa_loop: push af
        push bc
        call print_result
        pop bc
        pop af
        inc a
        djnz pa_loop
        ret

; A = number of sections with a failed check.
failed_sections:
        ld a,(nresults)
        or a
        ret z
        ld b,a
        ld c,0
        ld hl,results+2
fs_loop: ld a,(hl)
        inc hl
        or (hl)
        jr z,fs_next
        inc c
fs_next: ld de,9
        add hl,de
        djnz fs_loop
        ld a,c
        ret

; ---------------------------------------------------------------- frames

; Waits (HALT) until the frame counter is at least HL.
wait_until:
        push hl
wu_loop: pop hl
        push hl
        in a,(SYS_FRAME)
        ld e,a
        in a,(SYS_FRAME+1)
        ld d,a
        ex de,hl
        or a
        sbc hl,de
        jr nc,wu_done
        halt
        jr wu_loop
wu_done: pop hl
        ret

; Spins (no HALT) until the next frame interrupt: the frame uses its whole budget.
wait_tick:
        ld a,(ticks)
        ld b,a
wt_loop: ld a,(ticks)
        cp b
        jr z,wt_loop
        ret

; ---------------------------------------------------------------- test ops

; Runs the ops at HL (see the macros above) until op 0.
run_ops:
ro_next: ld (cur_op),hl
        ld a,(hl)
        inc hl
        or a
        ret z
        dec a
        jr z,ro_out
        dec a
        jr z,ro_in
        dec a
        jr z,ro_bytes
        dec a
        jr z,ro_bad
        dec a
        jr z,ro_str
        dec a
        jr z,ro_outn
        dec a
        jr z,ro_inn
        dec a
        jr z,ro_call
ro_bad: call fail
        ret
ro_out: ld c,(hl)
        inc hl
        ld a,(hl)
        inc hl
        out (c),a
        jr ro_next
ro_in:  ld c,(hl)
        inc hl
        in a,(c)
        cp (hl)
        inc hl
        call nz,fail
        jr ro_next
ro_str: ld a,(hl)
        inc hl
        or a
        jr z,ro_next
        out (T_PUT),a
        jr ro_str
ro_outn: ld c,(hl)
        inc hl
        ld a,(hl)
        inc hl
        ld b,(hl)
        inc hl
ro_on1: out (c),a
        djnz ro_on1
        jr ro_next
ro_inn: ld c,(hl)
        inc hl
        ld b,(hl)
        inc hl
ro_in1: in a,(c)
        djnz ro_in1
        jr ro_next
ro_call: ld e,(hl)
        inc hl
        ld d,(hl)
        inc hl
        push hl
        ex de,hl
        call jp_hl
        pop hl
        jr ro_next
; Cell memory at a pointer: dw pointer, db n, n bytes expected.
ro_bytes: ld a,(hl)
        out (T_PTRL),a
        inc hl
        ld a,(hl)
        out (T_PTRH),a
        inc hl
        ld b,(hl)
        inc hl
ro_by1: in a,(T_DATA)
        cp (hl)
        inc hl
        call nz,fail
        djnz ro_by1
        jr ro_next
jp_hl:  jp (hl)

; ---------------------------------------------------------------- paging

; Every slot shows every RAM page, ROM pages 0-3 and unmapped pages
; correctly (SPEC 1.1, 4). Interrupts are off: slot 0 holds the vectors.
t_paging:
        di
        ld hl,slot_test
        ld de,PAGING_COPY
        ld bc,slot_test_end-slot_test
        ldir
        ld c,0
tp_loop: ld a,c
        rrca
        rrca
        rrca
        ld d,a                  ; slot base address, high byte
        push bc
        ld a,c
        cp 4
        jr z,tp_rom
        call PAGING_COPY        ; from RAM in slot 4
        jr tp_after
tp_rom: call slot_test          ; slot 4 itself: from ROM
tp_after: pop bc
        ld a,c
        call point
        ld a,e
        or a
        call nz,fail
        inc c
        ld a,c
        cp 8
        jr nz,tp_loop
        ld hl,ops_paging
        call run_ops
        ld de,n_paging
        jp report

; Tests slot C (base address D x 100h); E = errors. Position independent
; (relative jumps only) and uses no stack between changing and restoring the
; slot, so it can run from RAM and test the slots of the code and the stack.
slot_test:
        in a,(c)
        ex af,af'               ; the slot's page, restored at the end
        ld e,0
        ld l,0
        ld b,80h
st_w:   out (c),b
        in a,(c)                ; the register reads back
        cp b
        jr z,st_w1
        inc e
st_w1:  ld a,d
        or 0Eh
        ld h,a
        ld a,b
        xor c
        ld (hl),a
        ld a,d
        or 1Eh
        ld h,a
        ld a,b
        cpl
        xor c
        ld (hl),a
        inc b
        ld a,b
        cp 80h+RAM_PAGES
        jr nz,st_w
        ld b,80h
st_r:   out (c),b
        ld a,d
        or 0Eh
        ld h,a
        ld a,b
        xor c
        cp (hl)
        jr z,st_r1
        inc e
st_r1:  ld a,d
        or 1Eh
        ld h,a
        ld a,b
        cpl
        xor c
        cp (hl)
        jr z,st_r2
        inc e
st_r2:  inc b
        ld a,b
        cp 80h+RAM_PAGES
        jr nz,st_r
        ld h,d                  ; ROM pages 0-3 start F3h, A1h, A2h, A3h
        ld b,0
st_rom: out (c),b
        ld a,b
        or a
        ld a,0F3h
        jr z,st_rom1
        ld a,b
        or 0A0h
st_rom1: cp (hl)
        jr z,st_rom2
        inc e
st_rom2: inc b
        ld a,b
        cp 4
        jr nz,st_rom
        ld b,1                  ; writes to ROM are ignored
        out (c),b
        ld (hl),55h
        ld a,(hl)
        cp 0A1h
        jr z,st_rw
        inc e
st_rw:  ld b,04h                ; ROM beyond the image, 10h-7Fh: FFh, writes ignored
st_u1:  out (c),b
        ld (hl),0
        ld a,(hl)
        inc a
        jr z,st_u1b
        inc e
st_u1b: inc b
        ld a,b
        cp 80h
        jr nz,st_u1
        ld b,80h+RAM_PAGES      ; RAM pages beyond the declared size
st_u2:  out (c),b
        ld (hl),0
        ld a,(hl)
        inc a
        jr z,st_u2b
        inc e
st_u2b: inc b
        jr nz,st_u2
        ex af,af'
        out (c),a
        ret
slot_test_end:

ops_paging:
        T_IN 00h,00h
        T_IN 01h,01h
        T_IN 02h,02h
        T_IN 03h,03h
        T_IN 04h,80h
        T_IN 05h,81h
        T_IN 06h,82h
        T_IN 07h,83h
        T_IN 08h,0FFh
        T_IN 0Bh,0FFh
        T_IN 0Fh,0FFh
        db 0

; ---------------------------------------------------------------- system

t_system:
        ld hl,ops_system
        call run_ops
        ld de,n_system
        jp report

ops_system:
        T_IN SYS_EV,0           ; the queue is empty at frame 0
        T_IN SYS_A,0
        T_IN SYS_B,0
        T_IN SYS_C,0
        T_IN SYS_FRAME,0
        T_IN SYS_FRAME+1,0
        T_IN SYS_FRAME+2,0
        T_IN SYS_FRAME+3,0
        T_IN SYS_PSEL,0FFh      ; write-only
        T_IN SYS_EXIT,0FFh
        T_INN SYS_RAND,3        ; random bytes: consumed, values not checked
        T_CALL sys_latched
        T_IN 2Bh,80
        T_IN 2Ch,30
        T_IN T_PUT,0FFh
        T_IN T_CMD,0FFh
        T_IN T_PALD,0FFh
        T_IN 2Dh,0FFh
        T_IN 2Fh,0FFh
        T_IN V_A0,0FFh
        T_IN V_A1,0FFh
        T_IN V_A2,0FFh
        T_IN V_L0,0FFh
        T_IN V_L1,0FFh
        T_IN V_CMD,0
        T_IN 36h,0FFh
        T_IN 3Fh,0FFh
        T_IN C_B0,0FFh
        T_IN C_B1,0FFh
        T_IN C_SUB,0
        T_IN 43h,0FFh
        T_IN 4Fh,0FFh
        T_IN 50h,0FFh           ; reserved and free slots
        T_IN 5Fh,0FFh
        T_IN 60h,0FFh
        T_IN 70h,0FFh
        T_IN 8Fh,0FFh
        T_IN 98h,0FFh
        T_IN 9Bh,0FFh
        T_IN 0A0h,0FFh
        T_IN 0B0h,0FFh
        T_IN 0F0h,0FFh
        T_IN 0FFh,0FFh
        db 0

; The frame counter and the time stay the same within a frame; the upper
; address byte of a port is ignored (SPEC 1).
sys_latched:
        ld c,SYS_FRAME
        ld b,8
sl_loop: in a,(c)
        ld e,a
        in a,(c)
        cp e
        call nz,fail
        inc c
        djnz sl_loop
        ld bc,0AA2Bh
        in a,(c)
        cp 80
        call nz,fail
        ld bc,0FF2Ch
        in a,(c)
        cp 30
        call nz,fail
        ret

; ---------------------------------------------------------------- events

; The queue holds exactly the events of conform.events frames 2-5.
t_events:
        ld hl,ev_table
te_loop: ld (cur_op),hl
        ld a,(hl)
        cp 0FFh
        jr z,te_end
        ld c,SYS_EV
        ld b,4
te_byte: in a,(c)
        cp (hl)
        call nz,fail
        inc hl
        inc c
        djnz te_byte
        out (SYS_EV),a
        jr te_loop
te_end: ld hl,ops_empty
        call run_ops
        ld de,n_events
        jp report

ops_empty:
        T_IN SYS_EV,0
        T_IN SYS_A,0
        T_IN SYS_B,0
        T_IN SYS_C,0
        db 0

ev_table:
        db 01h,04h,03h,00h      ; down a shift ctrl
        db 02h,04h,00h,00h      ; up a
        db 01h,28h,0Ch,00h      ; press enter alt meta
        db 02h,28h,0Ch,00h
        db 03h,61h,00h,00h      ; text a
        db 03h,0E9h,00h,00h     ; é
        db 03h,60h,4Fh,00h      ; 你 U+4F60
        db 03h,00h,0F6h,01h     ; U+1F600
        db 04h,7Fh,0DFh,3Eh     ; pointer 639 479 buttons 7
        db 04h,00h,00h,00h      ; pointer 0 0
        db 04h,40h,0F0h,09h     ; click 320 240: pressed
        db 04h,40h,0F0h,01h     ; released
        db 06h,0FDh,02h,00h     ; wheel -3 2
        db 06h,7Fh,81h,00h      ; wheel 127 -127
        db 07h,00h,00h,00h      ; feeds updated (the second is dropped)
        db 01h,45h,08h,00h      ; down f12 meta
        db 02h,0E7h,00h,00h     ; up E7h
        db 0FFh

; 70 events in one frame: the first 64 are kept.
t_overflow:
        ld bc,0
to_loop: in a,(SYS_EV)
        or a
        jr z,to_end
        in a,(SYS_A)
        ld (tmp),a
        ld a,b
        or c
        jr nz,to_next
        ld a,1
        call point
        ld a,(tmp)
        cp '0'
        call nz,fail
to_next: out (SYS_EV),a
        inc bc
        jr to_loop
to_end: ld a,2
        call point
        ld h,b
        ld l,c
        ld (extra),hl
        ld a,1
        ld (has_extra),a
        ld de,64
        call expect_hl
        ld a,3
        call point
        ld a,(tmp)
        cp '3'
        call nz,fail
        ld de,n_overflow
        jp report

; ---------------------------------------------------------------- controllers

t_pads: ld hl,11
        call wait_until
        ld hl,ops_pad11
        call run_ops
        ld hl,12
        call wait_until
        ld hl,ops_pad12
        call run_ops
        ld hl,13
        call wait_until
        ld hl,ops_pad13
        call run_ops
        ld de,n_pads
        jp report

ops_pad11:
        T_OUT SYS_PSEL,0
        T_IN SYS_PAD,01h        ; connected
        T_IN SYS_PAD,13h        ; up, down, A
        T_IN SYS_PAD,20h        ; start
        T_IN SYS_PAD,81h        ; -127
        T_IN SYS_PAD,7Fh
        T_IN SYS_PAD,05h
        T_IN SYS_PAD,0FBh
        T_IN SYS_PAD,00h
        T_IN SYS_PAD,01h        ; back to byte 0
        T_IN SYS_PAD,13h
        T_OUT SYS_PSEL,1        ; not connected
        T_CALL pad_zeros
        T_OUT SYS_PSEL,2
        T_IN SYS_PAD,01h
        T_IN SYS_PAD,0FFh
        T_IN SYS_PAD,0FFh
        T_IN SYS_PAD,00h
        T_OUT SYS_PSEL,3
        T_CALL pad_zeros
        T_OUT SYS_PSEL,4        ; no controller
        T_CALL pad_zeros
        T_OUT SYS_PSEL,0FFh
        T_CALL pad_zeros
        T_OUT SYS_PSEL,0
        T_IN SYS_PAD,01h
        T_IN SYS_PAD,13h
        T_IN SYS_PAD,20h        ; the read position carries into frame 12
        db 0
ops_pad12:
        T_IN SYS_PAD,00h        ; byte 3 of the new state
        T_IN SYS_PAD,00h
        T_IN SYS_PAD,00h
        T_IN SYS_PAD,00h
        T_IN SYS_PAD,00h        ; byte 7
        T_IN SYS_PAD,01h
        T_IN SYS_PAD,01h        ; up
        db 0
ops_pad13:
        T_OUT SYS_PSEL,0        ; disconnected
        T_CALL pad_zeros
        db 0

; Nine reads of port 1Eh are all zero.
pad_zeros:
        ld b,9
pz_loop: in a,(SYS_PAD)
        or a
        call nz,fail
        djnz pz_loop
        ret

; ---------------------------------------------------------------- timing

; Waits for a frame interrupt (HALT), then counts the 35-T-state loop until
; the next one: HL. Also records R and the interrupt's return address.
count_frame:
        ld a,r
        ld (r_before),a
cf_halt: halt
        ld a,r
        ld (r_after),a
        ld hl,(irq_ret)
        ld (halt_ret),hl
; Counts loop iterations until the next frame interrupt: HL.
count_next:
        ld hl,0
        ld a,(ticks)
        ld b,a
cn_loop: inc hl                 ; 6
        ld a,(ticks)            ; 13
        cp b                    ; 4
        jr z,cn_loop            ; 12
        ret

; Counts iterations of a loop that scrolls the terminal (600 T-states of
; charge) until the next frame interrupt: HL.
count_heavy:
        ld hl,0
        ld a,(ticks)
        ld b,a
        ld c,T_CMD
        ld d,2
ch_loop: inc hl                 ; 6
        out (c),d               ; 12 + 600
        ld a,(ticks)            ; 13
        cp b                    ; 4
        jr z,ch_loop            ; 12
        ret

; Stores HL as timing result A (0-15) and as the shown value.
store_count:
        ld (extra),hl
        ex de,hl
        ld l,a
        ld h,0
        add hl,hl
        ld bc,counts
        add hl,bc
        ld (hl),e
        inc hl
        ld (hl),d
        ex de,hl
        ld a,1
        ld (has_extra),a
        ret

; Loop counts with the interrupt in modes 1, 2 and 0 (SPEC 1.3: 13, 19 and
; 13 T-states to accept), three consecutive frames without HALT (the
; negative remainder carries), and the HALT and R rules.
t_timing:
        call count_frame
        ld a,0
        call store_count
        ld de,EXPECT_IM1
        ld a,1
        call point
        call expect_hl
        ld de,n_im1
        call report

        xor a
        ld (im2_seen),a
        ld a,IM2_TABLE >> 8
        ld i,a
        im 2
        call count_frame
        im 1
        ld a,1
        call store_count
        ld de,EXPECT_IM2
        ld a,1
        call point
        call expect_hl
        ld a,2
        call point
        ld a,(im2_seen)
        cp 1
        call nz,fail
        ld de,n_im2
        call report

        im 0
        call count_frame
        im 1
        ld a,2
        call store_count
        ld de,EXPECT_IM0
        ld a,1
        call point
        call expect_hl
        ld de,n_im0
        call report

        ; Three frames in a row of a 647-T-state loop, most of it the charge
        ; of a scroll: up to 646 T-states run over into the next frame.
        call count_frame
        call count_heavy
        ld (counts+6),hl
        call count_heavy
        ld (counts+8),hl
        call count_heavy
        ld (counts+10),hl
        ld a,1
        call point
        ld hl,(counts+6)
        ld de,EXPECT_C1
        call expect_hl
        ld a,2
        call point
        ld hl,(counts+8)
        ld de,EXPECT_C2
        call expect_hl
        ld a,3
        call point
        ld hl,(counts+10)
        ld de,EXPECT_C3
        call expect_hl
        ld (extra),hl
        ld a,1
        ld (has_extra),a
        ld de,n_carry
        call report

        ; HALT: the interrupt returns to the instruction after it, and R
        ; advances by a fixed amount however much of the frame was skipped.
        call count_frame
        ld a,1
        call point
        ld hl,(halt_ret)
        ld de,cf_halt+1
        call expect_hl
        ld a,2
        call point
        ld a,(r_before)
        ld b,a
        ld a,(r_after)
        sub b
        and 7Fh
        ld l,a
        ld h,0
        ld (extra),hl
        ld a,1
        ld (has_extra),a
        ld de,EXPECT_R
        call expect_hl
        ld de,n_halt
        jp report

; Device charges (SPEC 6.5, 8.4, 7.5) count as T-states: 4 clears, a scroll
; each way and a clear by put character (7 x 600), a 1,000-byte submit and
; a 500-byte put, then the loop count to the next frame.
t_charges:
        ld ix,BLOCK
        call blk_clear
        ld (ix+0),2
        ld (ix+40),21h          ; addr 002100h
        ld (ix+42),0F4h         ; size 500
        ld (ix+43),01h
        halt
        ld b,4
tc_clr: ld a,1
        out (T_CMD),a
        djnz tc_clr
        ld a,2
        out (T_CMD),a
        ld a,3
        out (T_CMD),a
        ld a,0Ch
        out (T_PUT),a
        xor a
        out (V_A0),a
        out (V_A2),a
        ld a,08h
        out (V_A1),a
        ld a,0E8h
        out (V_L0),a
        ld a,03h
        out (V_L1),a
        ld a,1
        out (V_CMD),a
        xor a
        out (C_B0),a
        ld a,BLOCK >> 8
        out (C_B1),a
        out (C_SUB),a
        call count_next
        ld a,3
        call store_count
        ld de,EXPECT_CHARGES
        ld a,1
        call point
        call expect_hl
        call done_reset
        ld a,1
        call wait_done
        ld a,2
        call point
        ld a,(done_status+1)
        cp 1
        call nz,fail
        ld de,n_charges
        jp report

; A frame that never halts, with interrupts off for over two frames: the
; request is a latch, taken once after EI, after the instruction following it.
t_fullbudget:
        halt
        di
        ld a,(ticks)
        ld (t0),a
        in a,(SYS_FRAME)
        ld (f0),a
        ld bc,28846             ; 26 T-states each: 750,000
fb_loop: dec bc
        ld a,b
        or c
        jr nz,fb_loop
        ld a,1
        call point
        in a,(SYS_FRAME)
        ld hl,f0
        sub (hl)
        cp 2
        call nz,fail
        ld a,(t0)
        ld c,a
        ei
        ld a,(ticks)            ; EI's shadow: not yet taken
        ld b,a
        ld a,(ticks)            ; now taken, once
        sub c
        ld e,a
        ld a,b
        sub c
        ld d,a
        ld a,2
        call point
        ld a,d
        or a
        call nz,fail
        ld a,3
        call point
        ld a,e
        cp 1
        call nz,fail
        ld b,50                 ; and not again in this frame
fb_spin: djnz fb_spin
        ld a,4
        call point
        ld a,(ticks)
        sub c
        cp 1
        call nz,fail
        ld de,n_full
        jp report

; ---------------------------------------------------------------- terminal

t_terminal:
        ld hl,ops_print
        call run_ops
        ld de,n_print
        call report
        ld hl,ops_wrap
        call run_ops
        ld de,n_wrap
        call report
        ld hl,ops_wide
        call run_ops
        ld de,n_wide
        call report
        ld hl,ops_utf8
        call run_ops
        ld de,n_utf8
        call report
        ld hl,ops_cmds
        call run_ops
        ld de,n_cmds
        call report
        ld hl,ops_cellptr
        call run_ops
        ld de,n_cellptr
        call report
        ld hl,ops_palette
        call run_ops
        ld de,n_palette
        jp report

ops_print:
        T_OUT T_FG,7
        T_OUT T_BG,0
        T_OUT T_CMD,1
        db 5,"AB",0
        CELL 0,0,'A',7,0
        CELL 1,0,'B',7,0
        T_IN T_COL,2
        T_IN T_ROW,0
        T_OUT T_FG,12
        T_OUT T_BG,4
        T_IN T_FG,12
        T_IN T_BG,4
        db 5,"C",0
        CELL 2,0,'C',12,4
        T_OUT T_COL,200         ; clamped
        T_IN T_COL,79
        T_OUT T_ROW,99
        T_IN T_ROW,29
        T_OUT T_FG,7
        T_OUT T_BG,0
        AT 79,29                ; the last cell: pending wrap, no scroll yet
        db 5,"Z",0
        CH 79,29,'Z'
        T_IN T_COL,79
        T_IN T_ROW,29
        CH 0,0,'A'
        db 5,"Y",0              ; now it scrolls
        CH 79,28,'Z'
        CH 0,29,'Y'
        CH 0,0,' '
        T_IN T_COL,1
        T_IN T_ROW,29
        AT 5,3
        db 5,8,0                ; backspace
        T_IN T_COL,4
        T_OUT T_COL,0
        db 5,8,0
        T_IN T_COL,0
        db 5,9,0                ; tab
        T_IN T_COL,8
        db 5,9,0
        T_IN T_COL,16
        T_OUT T_COL,75
        db 5,9,0
        T_IN T_COL,79
        db 5,9,0
        T_IN T_COL,79
        db 5,0Dh,0
        T_IN T_COL,0
        AT 9,3
        db 5,0Ah,0              ; line feed: column 0 of the next row
        T_IN T_COL,0
        T_IN T_ROW,4
        T_OUT T_PUT,0           ; ignored controls
        db 5,1,7,1Bh,1Fh,7Fh,0
        T_IN T_COL,0
        T_IN T_ROW,4
        CH 0,4,' '
        db 5,"x",0
        T_OUT T_FG,5
        db 5,0Ch,0              ; form feed: clear, cursor home
        T_IN T_COL,0
        T_IN T_ROW,0
        CELL 0,4,' ',5,0
        CELL 79,29,' ',5,0
        T_OUT T_FG,7
        db 0

ops_wrap:
        T_OUT T_CMD,1
        AT 79,3                 ; LF after pending wrap moves one row
        db 5,"E",0Ah,"a",0
        CH 0,4,'a'
        T_IN T_ROW,4
        AT 79,3                 ; CR
        db 5,"E",0Dh,"b",0
        CH 0,3,'b'
        T_IN T_COL,1
        T_IN T_ROW,3
        AT 79,3                 ; BS
        db 5,"E",8,0
        T_IN T_COL,78
        T_IN T_ROW,3
        AT 79,3                 ; TAB
        db 5,"E",9,"c",0
        CH 79,3,'c'
        T_IN T_ROW,3
        T_IN T_COL,79
        AT 79,3                 ; port 21h
        db 5,"E",0
        T_OUT T_COL,79
        db 5,"d",0
        CH 79,3,'d'
        T_IN T_ROW,3
        AT 79,3                 ; port 22h
        db 5,"E",0
        T_OUT T_ROW,3
        db 5,"e",0
        CH 79,3,'e'
        T_IN T_ROW,3
        AT 79,3                 ; ignored controls and other commands keep it
        db 5,"E",7,1Bh,7Fh,0
        T_OUT T_CMD,4
        T_OUT T_FG,9
        db 5,"f",0
        CH 0,4,'f'
        T_IN T_ROW,4
        T_IN T_COL,1
        T_OUT T_FG,7
        AT 79,3                 ; command 01h clears it
        db 5,"E",0
        T_OUT T_CMD,1
        T_IN T_COL,0
        T_IN T_ROW,0
        db 5,"g",0
        CH 0,0,'g'
        db 0

ops_wide:
        T_OUT T_CMD,1
        T_OUT T_FG,3
        T_OUT T_BG,5
        db 5,"你x",0
        CELL 0,0,4F60h,3,5
        CELL 1,0,0FFFFh,3,5
        CH 2,0,'x'
        T_IN T_COL,3
        T_OUT T_COL,78          ; at column 78: fills the row, pending wrap
        db 5,"好",0
        CH 78,0,597Dh
        CH 79,0,0FFFFh
        T_IN T_COL,79
        T_IN T_ROW,0
        db 5,"n",0
        CH 0,1,'n'
        CH 78,0,597Dh
        CH 79,0,0FFFFh
        AT 79,5                 ; at column 79: a space there, then the next row
        T_OUT T_FG,6
        T_OUT T_BG,2
        db 5,"好",0
        CELL 79,5,' ',6,2
        CH 0,6,597Dh
        CH 1,6,0FFFFh
        T_IN T_COL,2
        T_IN T_ROW,6
        AT 79,29                ; on the last row it scrolls
        db 5,"好",0
        CH 0,29,597Dh
        CH 1,29,0FFFFh
        CH 79,28,' '
        CH 0,5,597Dh
        T_IN T_COL,2
        AT 79,7                 ; with pending wrap, column 79 keeps its character
        db 5,"z好",0
        CH 79,7,'z'
        CH 0,8,597Dh
        CH 1,8,0FFFFh
        ; Overwriting half of a wide pair (SPEC 6.3 step 3): pairs at 0-1
        ; and 2-3 in colours 1/2, then text in 3/4.
        T_CALL wide_setup
        T_OUT T_COL,0
        db 5,"x",0
        CELL 0,10,'x',3,4
        CELL 1,10,' ',1,2
        CELL 2,10,4F60h,1,2
        T_CALL wide_setup
        T_OUT T_COL,1
        db 5,"x",0
        CELL 0,10,' ',1,2
        CELL 1,10,'x',3,4
        CELL 2,10,4F60h,1,2
        CELL 3,10,0FFFFh,1,2
        T_CALL wide_setup
        T_OUT T_COL,1
        db 5,"好",0
        CELL 0,10,' ',1,2
        CELL 1,10,597Dh,3,4
        CELL 2,10,0FFFFh,3,4
        CELL 3,10,' ',1,2
        T_CALL wide_setup
        T_OUT T_COL,3
        db 5,"好",0
        CELL 2,10,' ',1,2
        CELL 3,10,597Dh,3,4
        CELL 4,10,0FFFFh,3,4
        T_OUT T_FG,7
        T_OUT T_BG,0
        AT 0,12                 ; U+FFFF and above U+FFFF: U+FFFD, one cell
        db 5,0EFh,0BFh,0BFh,0
        CH 0,12,0FFFDh
        T_IN T_COL,1
        db 5,0F0h,9Fh,98h,80h,0
        CH 1,12,0FFFDh
        T_IN T_COL,2
        db 0

wide_setup:
        ld hl,ops_wide_setup
        jp run_ops
ops_wide_setup:
        AT 0,10
        T_OUT T_FG,1
        T_OUT T_BG,2
        T_OUT T_CMD,4
        db 5,"你你",0
        T_OUT T_FG,3
        T_OUT T_BG,4
        db 0

ops_utf8:
        T_OUT T_CMD,1
        AT 0,0
        db 5,80h,0              ; lone continuation byte
        CH 0,0,0FFFDh
        T_IN T_COL,1
        AT 0,1
        db 5,0FFh,0
        CH 0,1,0FFFDh
        T_IN T_COL,1
        AT 0,2                  ; overlong lead byte
        db 5,0C0h,0AFh,0
        CH 0,2,0FFFDh
        CH 1,2,0FFFDh
        T_IN T_COL,2
        AT 0,3                  ; cut short: the byte is decoded afresh
        db 5,0C3h,"A",0
        CH 0,3,0FFFDh
        CH 1,3,'A'
        T_IN T_COL,2
        AT 0,4
        db 5,0E4h,0BDh,"A",0
        CH 0,4,0FFFDh
        CH 1,4,'A'
        T_IN T_COL,2
        AT 0,5                  ; overlong three-byte form
        db 5,0E0h,80h,80h,0
        CH 0,5,0FFFDh
        CH 1,5,0FFFDh
        CH 2,5,0FFFDh
        T_IN T_COL,3
        AT 0,6                  ; surrogate
        db 5,0EDh,0A0h,80h,0
        CH 0,6,0FFFDh
        CH 2,6,0FFFDh
        T_IN T_COL,3
        AT 0,7                  ; above U+10FFFF
        db 5,0F4h,90h,80h,80h,0
        CH 3,7,0FFFDh
        T_IN T_COL,4
        AT 0,8
        db 5,0C3h,0C3h,0A9h,0
        CH 0,8,0FFFDh
        CH 1,8,00E9h
        T_IN T_COL,2
        AT 0,9                  ; a malformed newline is still a newline
        db 5,0E4h,0Ah,0
        CH 0,9,0FFFDh
        T_IN T_COL,0
        T_IN T_ROW,10
        AT 5,10                 ; incomplete: nothing yet, then cut short
        db 5,0F0h,9Fh,98h,0
        CH 5,10,' '
        T_IN T_COL,5
        db 5,"Z",0
        CH 5,10,0FFFDh
        CH 6,10,'Z'
        AT 0,11
        db 5,"é€",0
        CH 0,11,00E9h
        CH 1,11,20ACh
        T_IN T_COL,2
        db 0

ops_cmds:
        T_OUT T_CMD,1
        db 5,"row0",0Dh,0Ah,"row1",0
        T_OUT T_BG,2
        T_OUT T_CMD,2           ; scroll up
        CH 3,0,'1'
        CELL 0,29,' ',7,2
        T_OUT T_CMD,3           ; scroll down
        CH 3,1,'1'
        CELL 0,0,' ',7,2
        T_IN T_COL,4
        T_IN T_ROW,1
        AT 2,1
        T_OUT T_CMD,4           ; clear to the end of the row
        CH 1,1,'o'
        CELL 2,1,' ',7,2
        CELL 79,1,' ',7,2
        T_IN T_COL,2
        T_OUT T_CMD,6           ; hide, show, unknown
        T_OUT T_CMD,5
        T_OUT T_CMD,77h
        CH 1,1,'o'
        T_OUT T_CMD,1
        T_IN T_COL,0
        T_IN T_ROW,0
        CELL 5,5,' ',7,2
        T_OUT T_BG,0
        T_OUT T_CMD,1
        db 0

ops_cellptr:
        T_OUT T_PTRL,4          ; write cell (1,0)
        T_OUT T_PTRH,0
        T_OUT T_DATA,'Q'
        T_OUT T_DATA,0
        T_OUT T_DATA,9
        T_OUT T_DATA,1
        T_IN T_PTRL,8
        T_IN T_PTRH,0
        CELL 1,0,'Q',9,1
        T_OUT T_PTRL,7Fh        ; 9,599 wraps to 0
        T_OUT T_PTRH,25h
        T_OUT T_DATA,55h
        T_IN T_PTRL,0
        T_IN T_PTRH,0
        CELL 79,29,' ',7,55h
        T_OUT T_PTRL,84h        ; 9,604: kept as written, used modulo 9,600
        T_OUT T_PTRH,25h
        T_IN T_PTRL,84h
        T_IN T_PTRH,25h
        T_IN T_DATA,'Q'
        T_IN T_PTRL,5
        T_IN T_PTRH,0
        T_IN T_DATA,0
        T_OUT T_PTRL,0FFh       ; 65,535 is byte 7,935
        T_OUT T_PTRH,0FFh
        T_OUT T_DATA,0AAh
        T_IN T_PTRL,00h
        T_IN T_PTRH,1Fh
        CELL 63,24,' ',7,0AAh
        T_OUT T_PTRL,(80*4) & 0FFh ; a stray continuation code at (0,1)
        T_OUT T_PTRH,(80*4) >> 8
        T_OUT T_DATA,0FFh
        T_OUT T_DATA,0FFh
        CH 0,1,0FFFFh
        db 0

ops_palette:
        T_IN T_PALI,0
        T_OUT T_PALI,255
        T_OUT T_PALD,1
        T_OUT T_PALD,2
        T_OUT T_PALD,3
        T_OUT T_PALD,4          ; wrapped to entry 0
        T_OUT T_PALD,5
        T_OUT T_PALD,6
        T_IN T_PALI,1
        T_OUT T_PALD,9          ; entry 1 red, then restart at entry 7's red
        T_OUT T_PALI,7
        T_IN T_PALI,7
        T_OUT T_PALD,10
        T_IN T_PALI,7
        ; Put back the xterm colours of 255, 0, 1 and 7.
        T_OUT T_PALI,255
        T_OUTN T_PALD,0EEh,3
        T_OUTN T_PALD,0,3
        T_OUT T_PALD,0CDh
        T_OUTN T_PALD,0,2
        T_IN T_PALI,2
        T_OUT T_PALI,7
        T_OUTN T_PALD,0E5h,3
        T_IN T_PALI,8
        T_OUT T_PALI,0
        db 0

; ---------------------------------------------------------------- vector

t_vector:
        ld hl,showcase
        ld de,SMALL_LIST
        ld bc,showcase_end-showcase
        ldir
        ld hl,cutoff
        ld de,SMALL_LIST+200h
        ld bc,cutoff_end-cutoff
        ldir
        ld hl,ops_vector
        call run_ops
        ld de,n_vector
        jp report

ops_vector:
        T_OUT V_A0,00h          ; showcase list at linear 000800h
        T_OUT V_A1,08h
        T_OUT V_A2,00h
        T_OUT V_L0,(showcase_end-showcase) & 0FFh
        T_OUT V_L1,(showcase_end-showcase) >> 8
        T_OUT V_CMD,1
        T_IN V_CMD,0
        T_IN V_A0,0FFh
        T_IN V_A1,0FFh
        T_IN V_A2,0FFh
        T_IN V_L0,0FFh
        T_IN V_L1,0FFh
        T_IN 36h,0FFh
        T_IN 3Fh,0FFh
        T_OUT V_L0,0            ; length 0
        T_OUT V_L1,0
        T_OUT V_CMD,1
        T_IN V_CMD,1
        T_OUT V_L0,01h          ; 16,385
        T_OUT V_L1,40h
        T_OUT V_CMD,1
        T_IN V_CMD,1
        T_OUT V_A0,0FFh         ; 01FFFFh + 2 runs past the end of RAM
        T_OUT V_A1,0FFh
        T_OUT V_A2,01h
        T_OUT V_L0,2
        T_OUT V_L1,0
        T_OUT V_CMD,1
        T_IN V_CMD,1
        T_OUT V_L0,1            ; 01FFFFh, one byte: accepted
        T_OUT V_CMD,1
        T_IN V_CMD,0
        T_OUT V_A0,0            ; 020000h: outside
        T_OUT V_A1,0
        T_OUT V_A2,02h
        T_OUT V_CMD,1
        T_IN V_CMD,1
        T_OUT V_A0,0FFh         ; FFFFFFh
        T_OUT V_A1,0FFh
        T_OUT V_A2,0FFh
        T_OUT V_CMD,1
        T_IN V_CMD,1
        T_OUT V_CMD,0           ; unknown commands
        T_IN V_CMD,1
        T_OUT V_CMD,2           ; clear
        T_IN V_CMD,0
        T_OUT V_CMD,3
        T_IN V_CMD,1
        T_OUT V_CMD,0FFh
        T_IN V_CMD,1
        T_OUT V_A0,0            ; 16,384 bytes ending at the end of RAM
        T_OUT V_A1,0C0h
        T_OUT V_A2,01h
        T_OUT V_L0,0
        T_OUT V_L1,40h
        T_OUT V_CMD,1
        T_IN V_CMD,0
        T_OUT V_A0,1            ; one byte further: rejected
        T_OUT V_CMD,1
        T_IN V_CMD,1
        T_OUT V_A0,00h          ; operands cut off at the end of the list
        T_OUT V_A1,0Ah
        T_OUT V_A2,00h
        T_OUT V_L0,cutoff_end-cutoff
        T_OUT V_L1,0
        T_OUT V_CMD,1
        T_IN V_CMD,0
        T_OUT V_L0,cutoff_unknown-cutoff ; ends at the unknown opcode
        T_OUT V_CMD,1
        T_IN V_CMD,0
        T_CALL show_list
        T_IN V_CMD,0
        db 0

; Submits the showcase list (every opcode; a circle of radius 200 units at
; the centre of the screen, round on a conforming host).
show_list:
        xor a
        out (V_A0),a
        out (V_A2),a
        ld a,08h
        out (V_A1),a
        ld a,(showcase_end-showcase) & 0FFh
        out (V_L0),a
        ld a,(showcase_end-showcase) >> 8
        out (V_L1),a
        ld a,1
        out (V_CMD),a
        ret

showcase:
        db 01h,40h,80h,0FFh,0FFh ; colour
        db 02h,0                ; width 0 counts as 1
        db 04h
        dw 16*16,400*16         ; move
        db 05h
        dw 120*16,400*16        ; line
        db 02h,48               ; three units
        db 05h
        dw 120*16,460*16
        db 05h
        dw 120*16,460*16        ; to the current point: a dot
        db 01h,0FFh,00h,00h,80h ; translucent red
        db 07h
        dw 24*16                ; disc
        db 03h
        dw 320*16,240*16        ; origin: the centre of the screen
        db 04h
        dw 0,0
        db 01h,00h,0FFh,80h,0FFh
        db 02h,32
        db 06h
        dw 200*16               ; circle, radius 200 units
        db 01h,0FFh,0C0h,00h,60h
        db 08h
        dw 220*16,100*16        ; rectangle from the centre
        db 09h
        dw -300*16,200*16,-200*16,230*16 ; triangle
        db 03h
        dw -16,-16              ; origin -1, -1 unit
        db 04h
        dw 16,16                ; (0, 0)
        db 01h,0FFh,0FFh,0FFh,0FFh
        db 07h
        dw 64                   ; a disc in the top left corner, partly off screen
        db 00h                  ; end
        db 05h
        dw 5000,5000            ; after the end: not drawn
showcase_end:

cutoff: db 04h
        dw 5120,3840
        db 07h
        dw 160
        db 0Ah                  ; unknown opcode
cutoff_unknown:
        db 07h
        dw 320
        db 05h,10h,00h          ; operands cut off
cutoff_end:

; ---------------------------------------------------------------- content

; IX = a request block: 45 zero bytes with status AAh.
blk_clear:
        push ix
        pop hl
        ld b,45
        xor a
bc_loop: ld (hl),a
        inc hl
        djnz bc_loop
        ld (ix+1),0AAh
        ret

; Submits the block at IX; A = the result (a handle, or 0).
submit: push ix
        pop hl
        ld a,l
        out (C_B0),a
        ld a,h
        out (C_B1),a
        out (C_SUB),a
        in a,(C_SUB)
        ret

done_reset:
        xor a
        ld (done_count),a
        ld hl,done_status
        ld b,9
dr_loop: ld (hl),0FFh
        inc hl
        djnz dr_loop
        ret

; Takes the content-done events from the queue (others are dropped).
collect: in a,(SYS_EV)
        or a
        ret z
        cp 5
        jr nz,co_drop
        in a,(SYS_A)
        and 0Fh
        ld e,a
        ld d,0
        in a,(SYS_B)
        ld hl,done_status
        add hl,de
        ld (hl),a
        ld hl,done_count
        inc (hl)
co_drop: out (SYS_EV),a
        jr collect

; Waits until A content-done events have arrived, or 600 frames.
wait_done:
        ld (want),a
        ld hl,600
        ld (timeout),hl
wd_loop: call collect
        ld a,(want)
        ld b,a
        ld a,(done_count)
        cp b
        ret nc
        ld hl,(timeout)
        dec hl
        ld (timeout),hl
        ld a,h
        or l
        jr z,wd_out
        halt
        jr wd_loop
wd_out: jp fail

; Fails unless the B bytes at HL and DE are equal.
compare: ld a,(de)
        cp (hl)
        call nz,fail
        inc hl
        inc de
        djnz compare
        ret

t_content:
        ; Put 100 bytes (i x 7 + 3), then get them back by the ID the put wrote.
        ld hl,PUT_SRC
        ld b,100
        ld a,3
tc_fill: ld (hl),a
        add a,7
        inc hl
        djnz tc_fill
        ld hl,GET_DST2
        ld b,20
tc_ee:  ld (hl),0EEh
        inc hl
        djnz tc_ee
        ld ix,BLOCK
        call blk_clear
        ld (ix+0),2
        ld (ix+40),21h          ; addr 002100h
        ld (ix+42),100          ; size
        call submit
        ld (tmp),a
        ld a,1
        call point
        ld a,(tmp)
        cp 1
        call nz,fail
        ld a,(ix+1)             ; pending
        or a
        call nz,fail
        call done_reset
        ld a,1
        call wait_done
        ld a,2
        call point
        ld a,(done_status+1)
        cp 1
        call nz,fail
        ld a,(ix+1)
        cp 1
        call nz,fail
        ld a,3
        call point
        push ix
        pop hl
        inc hl
        inc hl
        ld de,put_id
        ld b,32
        call compare

        ld ix,BLOCK2            ; get the whole blob
        call blk_clear
        ld (ix+0),1
        call copy_id
        ld (ix+37),100          ; length
        ld (ix+40),22h          ; addr 002200h
        call submit
        ld (tmp),a
        ld a,4
        call point
        ld a,(tmp)
        cp 1
        call nz,fail
        call done_reset
        ld a,1
        call wait_done
        ld a,5
        call point
        ld a,(ix+1)
        cp 1
        call nz,fail
        ld a,(ix+37)
        cp 100
        call nz,fail
        ld a,(ix+42)
        cp 100
        call nz,fail
        ld hl,PUT_SRC
        ld de,GET_DST
        ld b,100
        call compare

        ld ix,BLOCK2            ; offset 90, length 20: 10 bytes delivered
        call blk_clear
        ld (ix+0),1
        call copy_id
        ld (ix+34),90
        ld (ix+37),20
        ld (ix+40),23h          ; addr 002300h
        call submit
        call done_reset
        ld a,1
        call wait_done
        ld a,6
        call point
        ld a,(ix+1)
        cp 1
        call nz,fail
        ld a,(ix+37)
        cp 10
        call nz,fail
        ld a,(ix+42)
        cp 100
        call nz,fail
        ld hl,PUT_SRC+90
        ld de,GET_DST2
        ld b,10
        call compare
        ld a,(GET_DST2+10)
        cp 0EEh
        call nz,fail

        ld ix,BLOCK2            ; offset 100: done, length 0
        call blk_clear
        ld (ix+0),1
        call copy_id
        ld (ix+34),100
        ld (ix+37),5
        ld (ix+40),23h
        call submit
        call done_reset
        ld a,1
        call wait_done
        ld a,7
        call point
        ld a,(ix+1)
        cp 1
        call nz,fail
        ld a,(ix+37)
        or (ix+38)
        call nz,fail

        ld ix,BLOCK2            ; a blob nobody has: not found
        call blk_clear
        ld (ix+0),1
        call absent_id
        ld (ix+37),5
        ld (ix+40),23h
        call submit
        call done_reset
        ld a,1
        call wait_done
        ld a,8
        call point
        ld a,(done_status+1)
        cp 2
        call nz,fail
        ld a,(ix+1)
        cp 2
        call nz,fail
        ld de,n_putget
        call report

        ; Bad requests: rejected, status 04h, nothing else written.
        ld hl,bad_blocks
        ld c,0
cb_loop: ld a,(hl)
        cp 0FFh
        jr z,cb_done
        push bc
        push hl
        ld ix,BLOCK
        call blk_clear
        pop hl
        push hl
        ld a,(hl)
        ld (ix+0),a             ; op
        inc hl
        ld a,(hl)
        ld (ix+37),a            ; length
        inc hl
        ld a,(hl)
        ld (ix+38),a
        inc hl
        ld a,(hl)
        ld (ix+39),a            ; addr
        inc hl
        ld a,(hl)
        ld (ix+40),a
        inc hl
        ld a,(hl)
        ld (ix+41),a
        inc hl
        ld a,(hl)
        ld (ix+42),a            ; size
        inc hl
        ld a,(hl)
        ld (ix+43),a
        inc hl
        ld a,(hl)
        ld (ix+44),a
        call submit
        pop hl
        pop bc
        push af
        ld a,c
        call point
        pop af
        or a
        call nz,fail
        ld a,(ix+1)
        cp 4
        call nz,fail
        ld de,9
        add hl,de
        inc c
        jr cb_loop
cb_done:
        ld a,20h                ; a block in ROM: rejected
        call point
        ld ix,0100h
        call submit
        or a
        call nz,fail
        ld a,21h                ; a block running past the end of RAM: rejected, unchanged
        call point
        ld a,80h+RAM_PAGES-1
        out (5),a
        ld ix,0BFE0h
        call blk_clear
        ld (ix+0),1
        ld (ix+37),1
        call submit
        ld b,a
        ld a,(ix+1)
        ld c,a
        ld a,81h
        out (5),a
        ld a,b
        or a
        call nz,fail
        ld a,c
        cp 0AAh
        call nz,fail
        ld de,n_bad
        call report

        ; Eight pending requests; a ninth is rejected and its block left alone.
        ld ix,BLOCKS8
        ld c,1
cp_loop: call blk_clear
        ld (ix+0),1
        call absent_id
        ld (ix+37),1
        ld (ix+40),23h
        push bc
        call submit
        pop bc
        ld b,a
        ld a,c
        call point
        ld a,b
        cp c
        call nz,fail
        ld de,48
        add ix,de
        inc c
        ld a,c
        cp 9
        jr nz,cp_loop
        call blk_clear
        ld (ix+0),1
        ld (ix+37),1
        call submit
        ld b,a
        ld a,9
        call point
        ld a,b
        or a
        call nz,fail
        ld a,(ix+1)
        cp 0AAh
        call nz,fail
        call done_reset
        ld a,8
        call wait_done
        ld a,10
        call point
        ld hl,done_status+1
        ld b,8
cp_st:  ld a,(hl)
        cp 2
        call nz,fail
        inc hl
        djnz cp_st
        ld de,n_pending
        call report

        ; Subscriptions and timeline: done (the records depend on the host's feeds).
        ld ix,BLOCK
        call blk_clear
        ld (ix+0),3
        ld (ix+37),0A0h         ; 1,440 bytes: 40 records
        ld (ix+38),05h
        ld (ix+40),28h          ; addr 002800h
        call submit
        ld ix,BLOCK2
        call blk_clear
        ld (ix+0),4
        ld (ix+37),0
        ld (ix+38),10h          ; 4,096 bytes
        ld (ix+40),30h          ; addr 003000h
        call submit
        call done_reset
        ld a,2
        call wait_done
        ld a,1
        call point
        ld a,(BLOCK+1)
        cp 1
        call nz,fail
        ld a,2
        call point
        ld a,(BLOCK2+1)
        cp 1
        call nz,fail
        ld de,n_feeds
        jp report

; The put's ID into the block at IX.
copy_id: push ix
        pop hl
        inc hl
        inc hl
        ex de,hl
        ld hl,put_id
        ld bc,32
        ldir
        ret

; An ID no blob has (all 11h) into the block at IX.
absent_id:
        push ix
        pop hl
        inc hl
        inc hl
        ld b,32
ai_loop: ld (hl),11h
        inc hl
        djnz ai_loop
        ret

; Bad blocks: op, length (2), addr (3), size (3).
bad_blocks:
        db 00h, 1,0, 00h,21h,00h, 1,0,0  ; unknown op
        db 05h, 1,0, 00h,21h,00h, 1,0,0
        db 0FFh-1, 1,0, 00h,21h,00h, 1,0,0
        db 01h, 0,0, 00h,21h,00h, 0,0,0  ; get, length 0
        db 01h, 01h,40h, 00h,21h,00h, 0,0,0 ; get, 16,385
        db 01h, 2,0, 0FFh,0FFh,01h, 0,0,0 ; get past the end of RAM
        db 01h, 1,0, 00h,00h,02h, 0,0,0  ; get at 020000h
        db 02h, 0,0, 00h,21h,00h, 0,0,0  ; put, size 0
        db 02h, 0,0, 00h,21h,00h, 01h,00h,10h ; put, 1,048,577
        db 02h, 0,0, 0F6h,0FFh,01h, 11,0,0 ; put past the end of RAM
        db 03h, 0,0, 00h,28h,00h, 0,0,0  ; subscriptions, length 0
        db 04h, 01h,40h, 00h,28h,00h, 0,0,0 ; timeline, 16,385
        db 0FFh

; ---------------------------------------------------------------- stress

; Full-budget frames: every frame rewrites all 2,400 cells (rows of 40 new
; CJK characters, rows of 80 Latin Extended-A or Cyrillic letters, colours
; changing) and submits a new 16,384-byte display list: STRESS_A frames of a
; list of 4,257 lines, rectangles, triangles, circles and discs, then
; STRESS_B frames of 5,456 translucent discs that each cover the screen.
stress: call build_list_a
        call build_list_b
        ld hl,0
        ld (sframe),hl
        ld hl,4E00h
        ld (cjk_next),hl
sa_loop: call wait_tick
        call stress_text
        ld a,88h
        ld b,00h
        call stress_vec
        ld hl,(sframe)
        inc hl
        ld (sframe),hl
        ld a,l
        cp STRESS_A
        jr nz,sa_loop
sb_loop: call wait_tick
        call stress_text
        ld a,8Ah
        ld b,40h
        call stress_vec
        ld hl,(sframe)
        inc hl
        ld (sframe),hl
        ld a,l
        cp STRESS_A+STRESS_B
        jr nz,sb_loop
        ret

; Moves the list in page A (address 01xx00h, B = xx) by changing its origin
; command, then submits it.
stress_vec:
        out (5),a
        ld a,(sframe)
        and 0Fh
        add a,a
        add a,a
        add a,a
        add a,a
        ld (0A001h),a
        ld (0A003h),a
        ld a,81h
        out (5),a
        xor a
        out (V_A0),a
        out (V_L0),a
        ld a,b
        out (V_A1),a
        ld a,1
        out (V_A2),a
        ld a,40h
        out (V_L1),a
        ld a,1
        out (V_CMD),a
        ret

stress_text:
        xor a
        ld (st_row),a
stt_row: ld a,(st_row)
        out (T_ROW),a
        ld b,a
        xor a
        out (T_COL),a
        ld a,(sframe)
        add a,b
        and 7
        add a,9
        out (T_FG),a
        ld a,b
        and 15
        add a,232
        out (T_BG),a
        ld a,b
        rrca
        jr c,stt_narrow
        ld de,(cjk_next)        ; 40 wide characters, UTF-8 by hand
        ld b,40
stt_w:  ld a,d
        rrca
        rrca
        rrca
        rrca
        and 0Fh
        or 0E0h
        out (T_PUT),a
        ld a,d
        and 0Fh
        add a,a
        add a,a
        ld c,a
        ld a,e
        rlca
        rlca
        and 3
        or c
        or 80h
        out (T_PUT),a
        ld a,e
        and 3Fh
        or 80h
        out (T_PUT),a
        inc de
        djnz stt_w
        ld a,d
        cp 9Fh
        jr c,stt_wok
        ld de,4E00h
stt_wok: ld (cjk_next),de
        jr stt_next
stt_narrow:
        and 1                   ; (row >> 1) & 1 chooses the block
        ld d,01h                ; U+0100-017F
        jr z,stt_n1
        ld d,04h                ; U+0400-047F
stt_n1: ld a,d
        add a,a
        add a,a
        or 0C0h
        ld d,a                  ; first byte, high bits
        ld a,(sframe)
        ld e,a
        ld b,80
stt_n:  ld a,e
        and 7Fh
        ld c,a
        rlca
        rlca
        and 1
        or d
        out (T_PUT),a
        ld a,c
        and 3Fh
        or 80h
        out (T_PUT),a
        inc e
        djnz stt_n
stt_next:
        ld a,(st_row)
        inc a
        ld (st_row),a
        cp 30
        jp nz,stt_row
        ret

; xorshift16: HL = the next random number.
rnd16:  ld hl,(rnd_state)
        ld a,h
        rra
        ld a,l
        rra
        xor h
        ld h,a
        ld a,l
        rra
        ld a,h
        rra
        xor l
        ld l,a
        xor h
        ld h,a
        ld (rnd_state),hl
        ret

; Appends A, or HL, at IX.
put_a:  ld (ix+0),a
        inc ix
        ret
put_hl: ld (ix+0),l
        ld (ix+1),h
        inc ix
        inc ix
        ret

; HL = a random x (0-8,191 + 1,024) or y (0-4,095 + 1,792) in sixteenths.
rnd_x:  call rnd16
        ld a,h
        and 1Fh
        add a,04h
        ld h,a
        ret
rnd_y:  call rnd16
        ld a,h
        and 0Fh
        add a,07h
        ld h,a
        ret

; Maps pages A and A + 1 at A000h-DFFFh and points IX at A000h.
map_list:
        out (5),a
        inc a
        out (6),a
        ld ix,0A000h
        ld a,03h                ; origin (0, 0), changed every frame
        call put_a
        ld hl,0
        call put_hl
        jp put_hl

unmap_list:
        ld a,81h
        out (5),a
        ld a,82h
        out (6),a
        ret

; List A in pages 88h-89h (linear 010000h): groups of colour, width, move,
; line, rectangle, triangle, move and 30 concentric circles and discs.
build_list_a:
        ld a,88h
        call map_list
bla_group:
        ld a,1
        call put_a
        call rnd16
        ld a,l
        call put_a
        ld a,h
        call put_a
        call rnd16
        ld a,l
        call put_a
        ld a,h
        or 0C0h
        call put_a
        ld a,2
        call put_a
        call rnd16
        ld a,l
        and 3Fh
        add a,8
        call put_a
        ld a,4
        call put_a
        call rnd_x
        call put_hl
        call rnd_y
        call put_hl
        ld a,5
        call put_a
        call rnd_x
        call put_hl
        call rnd_y
        call put_hl
        ld a,8
        call put_a
        call rnd_x
        call put_hl
        call rnd_y
        call put_hl
        ld a,9
        call put_a
        ld b,4
bla_tri: push bc
        call rnd_x
        call put_hl
        pop bc
        djnz bla_tri
        ld a,4
        call put_a
        call rnd_x
        call put_hl
        call rnd_y
        call put_hl
        ld b,30
        ld hl,16
bla_ring: ld a,b
        and 1
        add a,6                 ; circle or disc
        call put_a
        call put_hl
        ld de,24
        add hl,de
        djnz bla_ring
        push ix
        pop hl
        ld de,0E000h-126
        or a
        sbc hl,de
        jp c,bla_group
bla_fill: xor a                 ; the rest: End
        call put_a
        push ix
        pop hl
        ld a,h
        cp 0E0h
        jr nz,bla_fill
        jp unmap_list

; List B in pages 8Ah-8Bh (linear 014000h): 5,456 discs of radius 65,535
; sixteenths at the centre, each 3% opaque.
build_list_b:
        ld a,8Ah
        call map_list
        ld a,1
        call put_a
        ld a,0FFh
        call put_a
        ld a,60h
        call put_a
        ld a,20h
        call put_a
        ld a,8
        call put_a
        ld a,4
        call put_a
        ld hl,5120
        call put_hl
        ld hl,3840
        call put_hl
        ld bc,5456
blb_loop: ld a,7
        call put_a
        ld hl,0FFFFh
        call put_hl
        dec bc
        ld a,b
        or c
        jr nz,blb_loop
        xor a
        call put_a
        jp unmap_list

; ---------------------------------------------------------------- end

final:  ld hl,STRESS_A+STRESS_B
        ld (extra),hl
        ld a,1
        ld (has_extra),a
        ld de,n_stress
        call report
        ld a,7
        out (T_FG),a
        xor a
        out (T_BG),a
        call reprint
        call newline
        call failed_sections
        or a
        jr nz,fi_fail
        ld a,10
        out (T_FG),a
        ld hl,s_allpass
        call print
        jr fi_list
fi_fail: push af
        ld a,9
        out (T_FG),a
        ld hl,s_failed
        call print
        pop af
        call hex8
fi_list: ld a,7
        out (T_FG),a
        ld hl,s_esc
        call print
        jp show_list

; ---------------------------------------------------------------- text

title:      db "Terse conformance image ",0E2h,80h,0A2h," conform.events",0
s_pass:     db "PASS ",0
s_fail:     db "FAIL ",0
s_equals:   db " = ",0
s_fails:    db " (failed checks ",0
s_first:    db ", first at ",0
s_allpass:  db "All sections pass.",0
s_failed:   db "Sections failed: ",0
s_esc:      db "  Esc: exit (code = sections failed).",0
n_paging:   db "paging: 8 slots x ROM, RAM, unmapped pages",0
n_system:   db "system ports, absent devices",0
n_events:   db "events: key, text, pointer, wheel, feeds",0
n_overflow: db "event queue overflow (70 -> 64)",0
n_pads:     db "controllers",0
n_im1:      db "frame loop count, IM 1",0
n_im2:      db "frame loop count, IM 2",0
n_im0:      db "frame loop count, IM 0",0
n_carry:    db "negative remainder carry",0
n_halt:     db "HALT return address, R",0
n_charges:  db "device charges",0
n_full:     db "full-budget frames, interrupt latch, EI",0
n_print:    db "terminal: print, cursor, controls",0
n_wrap:     db "terminal: pending wrap",0
n_wide:     db "terminal: wide characters",0
n_utf8:     db "terminal: malformed UTF-8",0
n_cmds:     db "terminal: commands",0
n_cellptr:  db "terminal: cell pointer",0
n_palette:  db "terminal: palette",0
n_vector:   db "vector: submit, reject, clear",0
n_putget:   db "content: put, get, not found",0
n_bad:      db "content: bad requests",0
n_pending:  db "content: eight pending",0
n_feeds:    db "content: subscriptions, timeline",0
n_stress:   db "stress frames run",0

; BLAKE3-256 of the 100 bytes i x 7 + 3 (put by t_content).
put_id:     db 052h,003h,0B1h,052h,025h,06Ch,0E5h,0A2h,0AEh,0A1h,02Fh,012h,04Eh,03Bh,0D9h,0DEh
            db 0F9h,0EBh,0E2h,012h,0B4h,09Ch,027h,051h,023h,077h,039h,0F6h,030h,0EAh,051h,02Ah

; Timing results of this implementation, which is the reference (SPEC 9).
; Worked out by hand in README ("Conformance image"); the carry counts are
; what this implementation gives.
EXPECT_IM1      equ 8566
EXPECT_IM2      equ 8564
EXPECT_IM0      equ 8566
EXPECT_C1       equ 464
EXPECT_C2       equ 463
EXPECT_C3       equ 463
EXPECT_R        equ 20
EXPECT_CHARGES  equ 8395

; ROM pages 1-3 start with A1h, A2h, A3h (checked by slot_test).
        org 2000h
        db 0A1h
        org 4000h
        db 0A2h
        org 6000h
        db 0A3h
