/*
 * ump_stats.c: MIDI 2.0 through Universal MIDI Packets, with live feedback.
 *
 * Asks for MIDI 2.0 (all 16 UMP groups as ports), reads back the settings the
 * user's profile produced, and streams MIDI 2.0 notes with 16-bit velocities on
 * two groups while polling the statistics. The notify callback reports when the
 * user edits this application's settings in Maestro.
 */
#include "sleep.h"

#include <stdio.h>

#include "maestro.h"

static const MaestroCoreV1 *core;
static const MaestroRealtimeV1 *rt;

/* A MIDI 2.0 channel voice message (message type 4) is two words. */
static void midi2_note(uint32_t out[2], uint8_t group, uint8_t on, uint8_t channel,
                       uint8_t note, uint16_t velocity) {
    out[0] = 0x4u << 28 | (uint32_t)group << 24 | (on ? 0x9u : 0x8u) << 20
           | (uint32_t)channel << 16 | (uint32_t)note << 8;
    out[1] = (uint32_t)velocity << 16;
}

/* Sleeps until the host time `t`. */
static void wait_until(MaestroTime t) {
    const MaestroTime ms = rt->ns_to_ticks(1000000ull);
    for (MaestroTime now; (now = rt->now()) < t;)
        sleep_ms((unsigned)((t - now) / ms) + 1);
}

static volatile int settings_changed;

static void MAESTRO_CALL on_notify(void *user, uint32_t what) {
    (void)user;
    if (what == MAESTRO_NOTIFY_SETTINGS_CHANGED)
        settings_changed = 1; /* call get_info again on the main thread */
}

static void print_info(MaestroRealtime *inst) {
    MaestroRealtimeInfo i = { sizeof i };
    if (rt->get_info(inst, &i) == MAESTRO_OK)
        printf("%u Hz, %u ports, MIDI 2.0: %s, blocks of %u us\n",
               i.sample_rate, i.num_ports, i.midi2 ? "yes" : "no",
               i.render_block_us);
}

int main(void) {
    core = maestro_get_interface(MAESTRO_CORE_V1);
    rt = maestro_get_interface(MAESTRO_REALTIME_V1);
    if (!core || !rt) {
        fprintf(stderr, "The installed Maestro is too old for this program\n");
        return 1;
    }

    MaestroAppInfo app = { sizeof app };
    app.app_id = "org.example.maestro.ump-stats";
    app.app_name = "Maestro API example: UMP and statistics";

    MaestroRealtimeParams params = { sizeof params };
    params.midi2 = 1;
    params.notify = on_notify;

    MaestroRealtime *inst;
    if (rt->open(&app, &params, &inst) != MAESTRO_OK) {
        fprintf(stderr, "open: %s\n", core->last_error());
        return 1;
    }
    print_info(inst);

    /* A rising arpeggio on group 0 and its echo an octave up on group 1, with
     * velocities swelling across the full 16-bit range. Rather than queueing
     * it all up front, each step is sent a short lookahead before it is due,
     * as a sequencer would: the timestamp absorbs this thread's jitter. */
    const MaestroTime step = rt->ns_to_ticks(30000000ull);
    const MaestroTime lookahead = rt->ns_to_ticks(20000000ull);
    const MaestroTime start = rt->now() + lookahead;
    for (int i = 0; i < 96; i++) {
        MaestroTime at = start + step * (MaestroTime)i;
        wait_until(at - lookahead);

        uint8_t note = (uint8_t)(48 + (i * 7) % 36);
        uint16_t velocity = (uint16_t)(0x2000 + i * 0x0250);
        uint32_t packets[8];
        midi2_note(&packets[0], 0, 1, 0, note, velocity);
        midi2_note(&packets[2], 1, 1, 0, (uint8_t)(note + 12), velocity / 2);
        midi2_note(&packets[4], 0, 0, 0, note, 0);
        midi2_note(&packets[6], 1, 0, 0, (uint8_t)(note + 12), 0);

        /* One call may carry several packets that share a timestamp. */
        if (rt->send_ump(inst, &packets[0], 4, at) != MAESTRO_OK
            || rt->send_ump(inst, &packets[4], 4, at + step * 3) != MAESTRO_OK)
            fprintf(stderr, "send_ump: %s\n", core->last_error());

        if (i % 8 == 7) {
            MaestroRealtimeStats st = { sizeof st };
            if (rt->get_stats(inst, &st) == MAESTRO_OK)
                printf("%6llu notes/s  %6llu events/s  (limit %llu)  latency %u us\n",
                       (unsigned long long)st.nps, (unsigned long long)st.eps,
                       (unsigned long long)st.nps_limit, st.latency_us);
        }
        if (settings_changed) {
            settings_changed = 0;
            printf("Settings changed: ");
            print_info(inst);
        }
    }
    wait_until(rt->now() + rt->ns_to_ticks(500000000ull));

    rt->close(inst);
    return 0;
}
