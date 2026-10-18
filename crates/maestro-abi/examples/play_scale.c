/*
 * play_scale.c: the smallest useful Maestro API program.
 *
 * Links against the library, opens an instance, and schedules a C major scale
 * ahead of time with timestamps, so the notes play evenly no matter when this
 * thread gets to run.
 */
#include "sleep.h"

#include <stdio.h>

#include "maestro.h"

int main(void) {
    uint32_t version = maestro_get_version();
    if (version >> 16 != MAESTRO_VERSION_MAJOR || (version & 0xFFFF) < MAESTRO_VERSION_MINOR) {
        fprintf(stderr, "Maestro API %u.%u is installed, but this program needs %u.%u\n",
                version >> 16, version & 0xFFFF, MAESTRO_VERSION_MAJOR, MAESTRO_VERSION_MINOR);
        return 1;
    }

    MaestroOpenParams params = { sizeof params };
    params.app_id = "org.example.maestro.play-scale";
    params.app_name = "Maestro API example: play a scale";

    MaestroInstance *inst;
    if (maestro_open(MAESTRO_API_VERSION, &params, &inst) != MAESTRO_OK) {
        fprintf(stderr, "maestro_open: %s\n", maestro_last_error());
        return 1;
    }

    const MaestroExtMidi1 *midi = maestro_get_extension(inst, MAESTRO_EXT_MIDI1);
    if (!midi) {
        fprintf(stderr, "This library does not provide %s\n", MAESTRO_EXT_MIDI1);
        maestro_close(inst);
        return 1;
    }

    /* Program change to a piano on channel 1, right away. */
    midi->send_short(inst, 0, 0x0000C0, MAESTRO_TIME_NOW);

    /* Queue the whole scale up front, a quarter second per note, starting a
     * little in the future so the first note isn't late. */
    static const uint8_t scale[] = { 60, 62, 64, 65, 67, 69, 71, 72 };
    const MaestroTime step = maestro_ns_to_ticks(250000000ull);
    MaestroTime at = maestro_now() + maestro_ns_to_ticks(50000000ull);

    for (size_t i = 0; i < sizeof scale; i++) {
        uint8_t note = scale[i];
        /* status | data1 << 8 | data2 << 16 */
        MaestroResult r = midi->send_short(inst, 0, 0x90 | note << 8 | 100 << 16, at);
        if (r == MAESTRO_OK)
            r = midi->send_short(inst, 0, 0x80 | note << 8, at + step - step / 8);
        if (r != MAESTRO_OK)
            fprintf(stderr, "send_short: %s\n", maestro_last_error());
        at += step;
    }

    /* The events are queued; keep the instance open until they've played. */
    sleep_ms(8 * 250 + 1000);

    maestro_close(inst);
    return 0;
}
