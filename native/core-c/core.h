/* tcore: a small independent Terse core (SPEC.md machine profile 1) that
   keeps every device but System in driver processes (DEVICES.md). */
#ifndef TCORE_H
#define TCORE_H
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#define ROM_BASE 0x00010000u
#define RAM_BASE 0x00100000u
#define BUDGET 125000
#define QMAX 64

enum { RUNNING, EXITED, FAULTED };
enum { F_ILLEGAL = 1, F_BREAK, F_FETCH, F_LOAD, F_STORE, F_MISALIGNED };

static inline uint32_t rd16(const uint8_t *p) { return p[0] | p[1] << 8; }
static inline uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }
static inline void wr16(uint8_t *p, uint32_t v) { p[0] = v; p[1] = v >> 8; }
static inline void wr32(uint8_t *p, uint32_t v) { p[0] = v; p[1] = v >> 8; p[2] = v >> 16; p[3] = v >> 24; }

typedef struct {
    /* image */
    uint8_t *file; size_t file_len; uint8_t id[32];
    const uint8_t *rom; uint32_t rom_len;
    uint8_t *ram; uint32_t ram_len;
    int ndev; uint8_t dslot[16], dver[16], dopt[16], granted[16];
    int has[256]; uint8_t ver[256];     /* the devices the machine has */
    /* CPU and frame loop */
    uint32_t x[32], pc;
    uint32_t frame;
    uint8_t stopped, exit_code, fault_kind;
    uint32_t fault_enc, fault_pc, fault_addr;
    int64_t budget;
    int yield;                          /* wait_frame ends the CPU loop */
    /* System device */
    uint32_t time;
    uint8_t q[QMAX][4]; int qhead, qn;
    uint8_t pads[4][8];                 /* latched for the frame */
    int feeds_this_frame;
} M;

/* blake3.c */
void blake3(const void *data, size_t len, uint8_t out[32]);
/* cpu.c: run while budget > 0 */
void cpu_run(M *m);
void fault(M *m, int kind, uint32_t addr);
/* machine.c */
int in_rom(M *m, uint32_t a, uint32_t n);
int in_ram(M *m, uint32_t a, uint32_t n);
int image_load(M *m, uint8_t *file, size_t len, char *err, size_t errlen);
void ecall(M *m);
void queue_event(M *m, const uint8_t ev[4]);
size_t serialize(M *m, uint8_t **out);
/* bus.c */
uint32_t bus_call(M *m, int slot, int fn, int64_t *charge);
int bus_setup(M *m, char *err, size_t errlen);
void bus_begin(M *m, const uint8_t (*ev)[4], int nev, int64_t *charge);
void bus_end(M *m);
int bus_state(M *m, int slot, uint8_t **out, uint32_t *len);
void bus_shutdown(void);
void bus_stats(void);
extern int bus_nobatch;
extern const char *bus_driver_dir;
void bus_set_driver(const char *slots_eq_cmd);
extern int bus_withheld[256];
_Noreturn void die(int status, const char *fmt, ...);
#endif
