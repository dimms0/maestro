/*
 * render_file.c: rendering a MIDI file to an audio file.
 *
 * Renders on a second thread while the main one prints the progress, the way
 * an application keeps its UI responsive. The render uses the SoundFonts and
 * synth settings of this application's profile, like realtime playback does.
 *
 *     render_file song.mid song.flac
 */
#include "sleep.h"

#include <stdio.h>
#include <string.h>

#if defined(_WIN32)
#include <process.h>
#else
#include <pthread.h>
#endif

#include "maestro.h"

static const MaestroCoreV1 *core;
static const MaestroFileRenderV1 *fr;

struct render {
    MaestroFileRender *job;
    MaestroResult result;
    char error[512];
    volatile int done;
};

#if defined(_WIN32)
static unsigned __stdcall render_thread(void *arg) {
#else
static void *render_thread(void *arg) {
#endif
    struct render *r = arg;
    r->result = fr->run(r->job);
    /* last_error is per thread, so read it on the thread that failed. */
    if (r->result != MAESTRO_OK)
        snprintf(r->error, sizeof r->error, "%s", core->last_error());
    r->done = 1;
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <input.mid> <output.flac>\n", argv[0]);
        return 2;
    }

    core = maestro_get_interface(MAESTRO_CORE_V1);
    fr = maestro_get_interface(MAESTRO_FILE_RENDER_V1);
    if (!core || !fr) {
        fprintf(stderr, "The installed Maestro is too old for this program\n");
        return 1;
    }

    MaestroAppInfo app = { sizeof app };
    app.app_id = "org.example.maestro.render-file";
    app.app_name = "Maestro API example: render a file";

    MaestroFileRenderParams params = { sizeof params };
    params.midi_path = argv[1];
    params.output_path = argv[2];
    params.format = MAESTRO_FORMAT_FLAC;

    struct render r = { 0 };
    if (fr->open(&app, &params, &r.job) != MAESTRO_OK) {
        fprintf(stderr, "open: %s\n", core->last_error());
        return 1;
    }

#if defined(_WIN32)
    HANDLE thread = (HANDLE)_beginthreadex(NULL, 0, render_thread, &r, 0, NULL);
#else
    pthread_t thread;
    pthread_create(&thread, NULL, render_thread, &r);
#endif

    while (!r.done) {
        MaestroFileRenderProgress p = { sizeof p };
        if (fr->progress(r.job, &p) == MAESTRO_OK)
            fprintf(stderr, "\r%6.1f / %.1f s  %5.1fx  %llu voices   ", p.position_s,
                    p.length_s, p.speed, (unsigned long long)p.voice_count);
        sleep_ms(200);
    }
    fprintf(stderr, "\n");

#if defined(_WIN32)
    WaitForSingleObject(thread, INFINITE);
    CloseHandle(thread);
#else
    pthread_join(thread, NULL);
#endif
    fr->close(r.job);

    if (r.result != MAESTRO_OK) {
        fprintf(stderr, "render: %s\n", r.error);
        return 1;
    }
    printf("Wrote %s\n", argv[2]);
    return 0;
}
