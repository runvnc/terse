; fetch.asm - Terse content device demo (SPEC v0 section 7, machine profile 1).
;
; Gets the blob whose ID build.sh writes to message_id.inc (the BLAKE3-256 of
; message.txt), shows its status, size and text, then puts a 256-byte buffer
; built in RAM and shows the start of the ID the host returns. Esc exits.

; Device 1, system (SPEC 5)
SYS_EVENT   equ 10h
SYS_A       equ 11h
SYS_B       equ 12h
SYS_EXIT    equ 1Fh

EV_KEY_DOWN equ 01h
EV_CONTENT_DONE equ 05h
KEY_ESCAPE  equ 29h

; Device 2, terminal (SPEC 6)
TERM_PUT    equ 20h
TERM_FG     equ 23h
TERM_BG     equ 24h
TERM_CMD    equ 25h
CMD_CLEAR_EOL equ 04h

; Device 4, content (SPEC 7)
CONTENT_LO  equ 40h     ; request block address
CONTENT_HI  equ 41h
CONTENT_SUBMIT equ 42h  ; write: submit; read: handle, 0 if rejected

OP_GET      equ 01h
OP_PUT      equ 02h
ST_DONE     equ 01h
BLOCK_LEN   equ 45
BLK_STATUS  equ 1
BLK_ID      equ 2
BLK_LENGTH  equ 37
BLK_SIZE    equ 42

; RAM (slots 4-7 show RAM pages 80h-83h, linear 0000h-7FFFh)
GET_BLOCK   equ 8000h
PUT_BLOCK   equ 8040h
get_handle  equ 8080h
put_handle  equ 8081h
GET_DATA    equ 9000h   ; linear 001000h
GET_MAX     equ 4096
PUT_DATA    equ 0A000h  ; linear 002000h
PUT_SIZE    equ 256

        org 0
        di
        ld sp,0
        jp start

        org 38h
; IM 1 frame interrupt: only wakes the main loop from HALT.
irq:    ei
        reti

start:  im 1
        ld a,15
        out (TERM_FG),a
        ld a,4
        out (TERM_BG),a
        ld hl,title
        call print
        ld a,CMD_CLEAR_EOL
        out (TERM_CMD),a
        ld a,7
        out (TERM_FG),a
        xor a
        out (TERM_BG),a

        ld hl,s_getting
        call print
        ld hl,message_id
        ld b,8
        call print_hex_bytes
        ld hl,s_getting_end
        call print

        ld hl,get_template
        ld de,GET_BLOCK
        ld bc,BLOCK_LEN
        ldir
        ld hl,message_id
        ld de,GET_BLOCK+BLK_ID
        ld bc,32
        ldir
        ld hl,GET_BLOCK
        call submit
        ld (get_handle),a
        ld hl,s_fetching
        or a
        jr nz,submitted
        ld hl,s_rejected
submitted:
        call print

        ei
main:   halt                    ; one pass per frame
events: in a,(SYS_EVENT)
        or a
        jr z,main
        ld b,a
        in a,(SYS_A)
        ld c,a                  ; key code, or request handle
        out (SYS_EVENT),a       ; remove the head event
        ld a,b
        cp EV_KEY_DOWN
        jr nz,not_key
        ld a,c
        cp KEY_ESCAPE
        jr nz,events
        xor a
        out (SYS_EXIT),a
        di
        halt
not_key:
        cp EV_CONTENT_DONE
        jr nz,events
        ; Handles are reused once freed, so each is cleared when its request ends.
        ld a,(get_handle)
        cp c
        jr nz,not_get
        xor a
        ld (get_handle),a
        call get_done
        jr events
not_get:
        ld a,(put_handle)
        cp c
        jr nz,events
        xor a
        ld (put_handle),a
        call put_done
        jr events

; The get finished: status, size and text, then start the put.
get_done:
        ld a,(GET_BLOCK+BLK_STATUS)
        call print_status
        ld a,(GET_BLOCK+BLK_STATUS)
        cp ST_DONE
        jr nz,start_put
        ld hl,s_size
        call print
        ld hl,(GET_BLOCK+BLK_SIZE)
        ld a,(GET_BLOCK+BLK_SIZE+2)
        ld e,a
        call print_dec
        ld hl,s_size_end
        call print
        ld bc,(GET_BLOCK+BLK_LENGTH) ; bytes delivered
        ld hl,GET_DATA
text_loop:
        ld a,b
        or c
        jr z,start_put
        ld a,(hl)
        out (TERM_PUT),a
        inc hl
        dec bc
        jr text_loop

start_put:
        ld hl,PUT_DATA          ; bytes 00h, 01h, ... FFh
        xor a
fill:   ld (hl),a
        inc hl
        inc a
        jr nz,fill
        ld hl,put_template
        ld de,PUT_BLOCK
        ld bc,BLOCK_LEN
        ldir
        ld hl,s_putting
        call print
        ld hl,PUT_BLOCK
        call submit
        ld (put_handle),a
        or a
        ret nz
        ld hl,s_rejected
        jp print

; The put finished: status and the start of the ID.
put_done:
        ld a,(PUT_BLOCK+BLK_STATUS)
        call print_status
        ld a,(PUT_BLOCK+BLK_STATUS)
        cp ST_DONE
        jr nz,put_end
        ld hl,s_id
        call print
        ld hl,PUT_BLOCK+BLK_ID
        ld b,8
        call print_hex_bytes
        ld hl,s_ellipsis
        call print
put_end:
        ld hl,s_exit
        jp print

; Submits the request block at HL; returns the handle (0: rejected) in A.
submit: ld a,l
        out (CONTENT_LO),a
        ld a,h
        out (CONTENT_HI),a
        out (CONTENT_SUBMIT),a
        in a,(CONTENT_SUBMIT)
        ret

; Prints "status NN name" and a new line: green for done, red otherwise.
print_status:
        push af
        ld hl,s_status
        call print
        pop af
        push af
        call print_hex_a
        ld a,' '
        out (TERM_PUT),a
        pop af
        cp 5
        jr c,known
        ld a,5
known:  ld b,10
        cp ST_DONE
        jr z,colour
        ld b,9
colour: ld c,a
        ld a,b
        out (TERM_FG),a
        ld a,c
        add a,a
        ld e,a
        ld d,0
        ld hl,status_names
        add hl,de
        ld a,(hl)
        inc hl
        ld h,(hl)
        ld l,a
        call print
        ld a,7
        out (TERM_FG),a
        ld a,0Ah
        out (TERM_PUT),a
        ret

; Prints the zero-terminated string at HL.
print:  ld a,(hl)
        inc hl
        or a
        ret z
        out (TERM_PUT),a
        jr print

; Prints B bytes from HL as lowercase hex.
print_hex_bytes:
        ld a,(hl)
        call print_hex_a
        inc hl
        djnz print_hex_bytes
        ret

print_hex_a:
        push af
        rrca
        rrca
        rrca
        rrca
        call hex_digit
        pop af
hex_digit:
        and 0Fh
        add a,'0'
        cp '9'+1
        jr c,hex_out
        add a,'a'-'9'-1
hex_out:
        out (TERM_PUT),a
        ret

; Prints the 24-bit number E:H:L in decimal.
print_dec:
        ld c,0
dec_loop:
        call div10
        push af
        inc c
        ld a,e
        or h
        or l
        jr nz,dec_loop
dec_out:
        pop af
        add a,'0'
        out (TERM_PUT),a
        dec c
        jr nz,dec_out
        ret

; E:H:L = E:H:L / 10, A = remainder.
div10:  ld b,24
        xor a
div_loop:
        add hl,hl
        rl e
        rla
        cp 10
        jr c,div_next
        sub 10
        inc l
div_next:
        djnz div_loop
        ret

get_template:
        db OP_GET, 0
        ds 32                   ; id, copied from message_id
        db 0, 0, 0              ; offset
        dw GET_MAX              ; length
        db 00h, 10h, 00h        ; addr: linear 001000h = GET_DATA
        db 0, 0, 0              ; size, written on completion

put_template:
        db OP_PUT, 0
        ds 32                   ; id, written on completion
        db 0, 0, 0
        dw 0
        db 00h, 20h, 00h        ; addr: linear 002000h = PUT_DATA
        db PUT_SIZE & 0FFh, PUT_SIZE >> 8, 0

status_names:
        dw s_pending, s_done, s_not_found, s_failed, s_bad, s_unknown

title:  db " Terse · fetch", 0
s_getting: db 0Ah, 0Ah, "Getting blob ", 0
s_getting_end: db "… (message.txt)", 0Ah, 0
s_fetching: db "fetching… ", 0
s_rejected: db "submit rejected", 0Ah, 0
s_status: db "status ", 0
s_size: db "size: ", 0
s_size_end: db " bytes. Text:", 0Ah, 0Ah, 0
s_putting: db 0Ah, "Putting 256 bytes built in RAM… ", 0
s_id:   db "blob ID: ", 0
s_ellipsis: db "…", 0Ah, 0
s_exit: db 0Ah, "Esc: exit.", 0
s_pending: db "pending", 0
s_done: db "done", 0
s_not_found: db "not found (not in the store or on the relay)", 0
s_failed: db "failed (network error or timeout)", 0
s_bad:  db "bad request", 0
s_unknown: db "unknown", 0

        include "message_id.inc"
