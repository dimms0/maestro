/*
 * play_scale.c: the smallest useful Maestro API program.
 *
 * Links against the library, opens a realtime engine, and schedules a C major scale
 * ahead of time with timestamps, so the notes play evenly no matter when this
 * thread gets to run.
 */
#include "sleep.h"

#include <stdio.h>

#include "maestro.h"

int main(void) {
    /* Ask for exactly the interfaces this program uses; an older library
     * returns NULL for any it doesn't have. */
    const MaestroCoreV1 *core = maestro_get_interface(MAESTRO_CORE_V1);
    const MaestroRealtimeV1 *rt = maestro_get_interface(MAESTRO_REALTIME_V1);
    if (!core || !rt) {
        fprintf(stderr, "The installed Maestro is too old for this program\n");
        return 1;
    }

    MaestroAppInfo app = { sizeof app };
    app.app_id = "org.example.maestro.play-scale";
    app.app_name = "Maestro API example: play a scale";

    MaestroRealtime *inst;
    if (rt->open(&app, NULL, &inst) != MAESTRO_OK) {
        fprintf(stderr, "open: %s\n", core->last_error());
        return 1;
    }

    /* Program change to a piano on channel 1, right away. */
    rt->send_short(inst, 0, 0x0000C0, MAESTRO_TIME_NOW);

    /* Queue the whole scale up front, a quarter second per note, starting a
     * little in the future so the first note isn't late. */
    static const uint8_t scale[] = { 60, 62, 64, 65, 67, 69, 71, 72 };
    const MaestroTime step = rt->ns_to_ticks(250000000ull);
    MaestroTime at = rt->now() + rt->ns_to_ticks(50000000ull);

    for (size_t i = 0; i < sizeof scale; i++) {
        uint8_t note = scale[i];
        /* status | data1 << 8 | data2 << 16 */
        MaestroResult r = rt->send_short(inst, 0, 0x90 | note << 8 | 100 << 16, at);
        if (r == MAESTRO_OK)
            r = rt->send_short(inst, 0, 0x80 | note << 8, at + step - step / 8);
        if (r != MAESTRO_OK)
            fprintf(stderr, "send_short: %s\n", core->last_error());
        at += step;
    }

    /* The events are queued; keep the engine open until they've played. */
    sleep_ms(8 * 250 + 1000);

    rt->close(inst);
    return 0;
}
