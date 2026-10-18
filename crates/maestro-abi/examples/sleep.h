/* sleep.h: a portable millisecond sleep for the examples. Include it first. */
#ifndef MAESTRO_EXAMPLES_SLEEP_H
#define MAESTRO_EXAMPLES_SLEEP_H

#if defined(_WIN32)
#  include <windows.h>
static void sleep_ms(unsigned ms) { Sleep(ms); }
#else
#  ifndef _POSIX_C_SOURCE
#    define _POSIX_C_SOURCE 200809L
#  endif
#  include <time.h>
static void sleep_ms(unsigned ms) {
    struct timespec ts = { ms / 1000, (long)(ms % 1000) * 1000000L };
    while (nanosleep(&ts, &ts) != 0) {}
}
#endif

#endif
