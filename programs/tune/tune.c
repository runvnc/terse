/* tune: a short piece for the sound device (device 6, SPEC 18), showing
 * each kind of voice in turn and then several together: a plain wavetable
 * melody over a square bass; pairing (one voice's output moving another's
 * phase: a bell); feedback (a voice moving its own phase: a brassy lead);
 * ring modulation (two voices multiplied: a metallic clang); noise (hi-hat
 * and snare, with a kick drum made by sweeping a sine's frequency every
 * frame); and a looped sample (a pluck the program computes into RAM).
 * The terminal names what is playing. It exits with 0 after about 18
 * seconds (1,106 frames).
 *
 * Everything is integer: the note increments are constants (round(hz *
 * 2^32 / 48,000) for MIDI notes 36 to 96), so the same image gives the
 * same samples on every host. Built with clang (see build.sh). */

#include <stdint.h>
#include "terse.h"

/* round(127 sin(2 pi i / 64)). */
static const int8_t sine[64] = {
    0, 12, 25, 37, 49, 60, 71, 81, 90, 98, 106, 112, 117, 122, 125, 126,
    127, 126, 125, 122, 117, 112, 106, 98, 90, 81, 71, 60, 49, 37, 25, 12,
    0, -12, -25, -37, -49, -60, -71, -81, -90, -98, -106, -112, -117, -122, -125, -126,
    -127, -126, -125, -122, -117, -112, -106, -98, -90, -81, -71, -60, -49, -37, -25, -12,
};

/* Phase increments of MIDI notes 36 (C2) to 96 (C7). */
static const uint32_t notes[61] = {
    5852465u, 6200470u, 6569170u, 6959793u, 7373644u, 7812103u,
    8276635u, 8768789u, 9290209u, 9842633u, 10427907u, 11047982u,
    11704930u, 12400941u, 13138339u, 13919586u, 14747287u, 15624207u,
    16553270u, 17537579u, 18580418u, 19685267u, 20855814u, 22095965u,
    23409859u, 24801882u, 26276679u, 27839171u, 29494575u, 31248413u,
    33106541u, 35075158u, 37160835u, 39370534u, 41711627u, 44191930u,
    46819719u, 49603764u, 52553357u, 55678342u, 58989149u, 62496826u,
    66213081u, 70150316u, 74321671u, 78741067u, 83423255u, 88383859u,
    93639437u, 99207528u, 105106715u, 111356685u, 117978298u, 124993653u,
    132426162u, 140300631u, 148643341u, 157482134u, 166846509u, 176767719u,
    187278874u,
};
#define N(midi) notes[(midi) - 36]

#define W_SINE 0
#define W_SQUARE 1
#define W_SAW 2
#define W_TRI 3

static int8_t wave[64];
#define PLUCK_LEN 2048
static int8_t pluck[PLUCK_LEN];
static uint8_t list[512];

/* The melody, one note a beat: a line in C major. The bass, one a bar. */
static const uint8_t melody[16] = { 72, 76, 79, 76, 77, 74, 71, 74, 72, 76, 79, 84, 83, 79, 74, 72 };
static const uint8_t bass[4] = { 48, 41, 43, 48 };
#define BEAT 8 /* frames */

static void say(const char *s)
{
    term_puts(s);
    term_puts("\r\n");
}

static void load_waves(void)
{
    snd_load_wave(W_SINE, sine);
    for (int i = 0; i < 64; i++)
        wave[i] = i < 32 ? 90 : -90;
    snd_load_wave(W_SQUARE, wave);
    for (int i = 0; i < 64; i++)
        wave[i] = (int8_t)(i * 4 - 128);
    snd_load_wave(W_SAW, wave);
    for (int i = 0; i < 64; i++)
        wave[i] = (int8_t)(i < 32 ? i * 8 - 128 : 383 - i * 8);
    snd_load_wave(W_TRI, wave);
    /* A pluck: a saw of period 32, halving every 512 samples, looped over
     * its last period (it is meant to be played at 8,000 to 13,000
     * samples a second). */
    for (int i = 0; i < PLUCK_LEN; i++)
        pluck[i] = (int8_t)((((i & 31) * 8) - 128) >> (i >> 9));
    snd_load_sample(0, pluck, PLUCK_LEN, PLUCK_LEN - 32);
}

/* A wavetable voice's wave, volume and pan. */
static void voice(uint8_t v, uint8_t w, uint8_t vol, uint8_t l, uint8_t r)
{
    snd_voice(v);
    snd_wave(w);
    snd_volume(vol);
    snd_pan(l, r);
}

static void note_on(uint8_t v, uint32_t inc)
{
    snd_voice(v);
    snd_frequency(inc);
    snd_key_on();
}

static void note_off(uint8_t v)
{
    snd_voice(v);
    snd_key_off();
}

/* Sections of 16 beats, each followed by half a second of silence. */
enum { PLAIN, PAIR, FEED, RING, NOISE, SAMPLE, ALL, END };
static const char *const titles[END] = {
    "1. plain wavetable: a triangle melody over a square bass",
    "2. pairing: voice 3's output moves voice 2's phase (a bell)",
    "3. feedback: voice 4's own output moves its phase (brass)",
    "4. ring modulation: voice 5 times voice 6 (a clang)",
    "5. noise: hi-hat and snare; a kick swept by its frequency",
    "6. sample: a pluck computed into RAM, looped",
    "7. together: melody, bass, drums and pluck",
};

static void setup(void)
{
    snd_begin(list, sizeof list);
    /* 0: melody, triangle. 1: bass, square. */
    voice(0, W_TRI, 200, 255, 160);
    snd_envelope(0x4000, 400, 160, 900);
    voice(1, W_SQUARE, 150, 160, 255);
    snd_envelope(0xFFFF, 300, 120, 2000);
    /* 2: the bell's carrier, its phase moved by 3 (muted: only heard
     * through 2), at 3.5 times its frequency. */
    voice(2, W_SINE, 230, 230, 230);
    snd_envelope(0xFFFF, 60, 0, 60);
    snd_modulator(3, 90);
    voice(3, W_SINE, 255, 0, 0);
    snd_mix(0);
    snd_envelope(0xFFFF, 120, 0, 120);
    /* 4: brass, a sine with feedback. */
    voice(4, W_SINE, 200, 220, 220);
    snd_envelope(0x0800, 200, 180, 1500);
    snd_feedback(150);
    /* 5 times 6 (muted): ring modulation. */
    voice(5, W_SINE, 255, 220, 220);
    snd_envelope(0xFFFF, 150, 0, 150);
    snd_ring(6);
    voice(6, W_SINE, 255, 0, 0);
    snd_mix(0);
    snd_envelope(0, 0, 255, 0);
    /* 7: hi-hat noise; 8: snare noise; 9: kick, a sine swept down. */
    snd_voice(7);
    snd_noise();
    snd_volume(110);
    snd_pan(200, 255);
    snd_envelope(0xFFFF, 4000, 0, 4000);
    snd_voice(8);
    snd_noise();
    snd_volume(180);
    snd_pan(255, 220);
    snd_envelope(0xFFFF, 900, 0, 900);
    voice(9, W_SINE, 255, 255, 255);
    snd_envelope(0xFFFF, 500, 0, 500);
    /* 10: the pluck. */
    snd_voice(10);
    snd_sample(0);
    snd_volume(220);
    snd_pan(230, 230);
    snd_envelope(0xFFFF, 120, 0, 400);
    snd_submit_list(list);
}

/* The kick's increment t frames after its hit: 150 Hz falling to 45. */
static uint32_t kick_inc(int t)
{
    int hz = 150 - t * 12;
    return (uint32_t)(hz < 45 ? 45 : hz) * 89478u;
}

static void drums(int beat, int t)
{
    if (t == 0) {
        note_on(7, 0xF0000000u); /* hi-hat: noise stepping 45,000 times a second */
        if (beat % 4 == 2)
            note_on(8, 0x30000000u); /* snare: 9,000 a second */
        if (beat % 4 == 0)
            note_on(9, kick_inc(0));
    } else if (t < 10) {
        snd_voice(9);
        snd_frequency(kick_inc(t));
    }
}

static void play(int section, int beat, int t)
{
    uint8_t m = melody[beat];
    if (section == PLAIN || section == ALL) {
        if (t == 0)
            note_on(0, N(m));
        if (t == BEAT - 2)
            note_off(0);
        if (t == 0 && beat % 4 == 0)
            note_on(1, N(bass[beat / 4]));
        if (t == 0 && beat % 4 == 3)
            note_off(1);
    }
    if (section == PAIR && t == 0 && beat % 2 == 0) {
        note_on(2, N(m));
        note_on(3, N(m) * 3 + N(m) / 2);
    }
    if (section == FEED) {
        if (t == 0)
            note_on(4, N(m - 12));
        if (t == BEAT - 1)
            note_off(4);
    }
    if (section == RING && t == 0 && beat % 2 == 0) {
        note_on(5, N(m - 12));
        note_on(6, 27827808u); /* 311 Hz */
    }
    if (section == NOISE || section == ALL)
        drums(beat, t);
    if ((section == SAMPLE || section == ALL) && t == 0 && beat % 2 == 0) {
        /* The pluck's period is 32 samples, so a note of f Hz plays it
         * at 32 f samples a second: C4, E4, G4. The increment is that
         * rate in 16.16 positions an output sample: rate * 65,536 /
         * 48,000. */
        static const uint32_t rate[3] = { 8372, 10548, 12544 };
        note_on(10, rate[(beat / 2) % 3] * 4096u / 3000u);
    }
}

int main(void)
{
    load_waves();
    setup();
    term_cmd(TERM_CLEAR);
    say("tune: the sound device (SPEC 18)");
    say("");
    for (int section = 0; section < END; section++) {
        say(titles[section]);
        for (int beat = 0; beat < 16; beat++) {
            for (int t = 0; t < BEAT; t++) {
                snd_begin(list, sizeof list);
                play(section, beat, t);
                if (snd_length(list))
                    snd_submit_list(list);
                terse_wait_frame();
            }
        }
        snd_begin(list, sizeof list);
        for (int v = 0; v < 11; v++)
            note_off(v);
        snd_submit_list(list);
        for (int t = 0; t < 30; t++)
            terse_wait_frame();
    }
    say("");
    say("done");
    return 0;
}
