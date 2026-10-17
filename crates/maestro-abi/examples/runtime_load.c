/*
 * runtime_load.c: loading the Maestro API at runtime.
 *
 * An application that should still start when Maestro isn't installed loads
 * the library itself instead of linking against it, then resolves the core
 * functions by name. It also shows the log callback, telling the user why the
 * API is unavailable, and the control extension.
 */
#include "sleep.h"

#include <stdio.h>

#if defined(_WIN32)
#include <windows.h>
#define LIBRARY_NAME "maestro.dll"
#define open_library() LoadLibraryA(LIBRARY_NAME)
#define find_symbol(lib, name) ((void *)GetProcAddress((lib), (name)))
typedef HMODULE Library;
#else
#include <dlfcn.h>
#if defined(__APPLE__)
#define LIBRARY_NAME "libmaestro.1.dylib"
#else
#define LIBRARY_NAME "libmaestro.so.1"
#endif
#define open_library() dlopen(LIBRARY_NAME, RTLD_NOW | RTLD_LOCAL)
#define find_symbol(lib, name) dlsym((lib), (name))
typedef void *Library;
#endif

#define MAESTRO_NO_PROTOTYPES
#include "maestro.h"

/* The core functions, as pointers. */
static struct {
  MaestroGetVersionFn get_version;
  MaestroLastErrorFn last_error;
  MaestroNowFn now;
  MaestroNsToTicksFn ns_to_ticks;
  MaestroOpenFn open;
  MaestroCloseFn close;
  MaestroGetExtensionFn get_extension;
} maestro;

static int load_maestro(void) {
  Library lib = open_library();
  if (!lib)
    return 0;

#define RESOLVE(field, name)                                                   \
  if (!(*(void **)&maestro.field = find_symbol(lib, name)))                    \
    return 0;

  RESOLVE(get_version, "maestro_get_version")
  RESOLVE(last_error, "maestro_last_error")
  RESOLVE(now, "maestro_now")
  RESOLVE(ns_to_ticks, "maestro_ns_to_ticks")
  RESOLVE(open, "maestro_open")
  RESOLVE(close, "maestro_close")
  RESOLVE(get_extension, "maestro_get_extension")
#undef RESOLVE

  uint32_t version = maestro.get_version();
  return version >> 16 == MAESTRO_VERSION_MAJOR &&
         (version & 0xFFFF) >= MAESTRO_VERSION_MINOR;
}

/* May run on any thread; keep it short and don't call into Maestro. */
static void MAESTRO_CALL on_log(void *user, uint32_t level,
                                const char *message) {
  static const char *const names[] = {"error", "warn"};
  (void)user;
  fprintf(stderr, "[maestro %s] %s\n", level < 2 ? names[level] : "?", message);
}

int main(void) {
  if (!load_maestro()) {
    printf("Maestro (" LIBRARY_NAME
           ") is not installed or too old; carrying on without it.\n");
    return 0;
  }

  MaestroOpenParams params = {sizeof params};
  params.app_id = "org.example.maestro.runtime-load";
  params.app_name = "Maestro API example: runtime loading";
  params.log = on_log;

  MaestroInstance *inst;
  MaestroResult r = maestro.open(MAESTRO_API_VERSION, &params, &inst);
  switch (r) {
  case MAESTRO_OK:
    break;
  case MAESTRO_ERR_DISABLED:
    /* The user's choice, not a fault: fall back quietly. */
    printf("Maestro is turned off for this application: %s\n",
           maestro.last_error());
    return 0;
  default:
    fprintf(stderr, "maestro_open failed (%d): %s\n", r, maestro.last_error());
    return 1;
  }

  const MaestroExtMidi1 *midi = maestro.get_extension(inst, MAESTRO_EXT_MIDI1);
  const MaestroExtControl *control =
      maestro.get_extension(inst, MAESTRO_EXT_CONTROL);

  /* A chord, played now and released a second later. */
  static const uint8_t chord[] = {48, 55, 64, 67};
  MaestroTime release = maestro.now() + maestro.ns_to_ticks(1000000000ull);
  for (size_t i = 0; i < sizeof chord; i++) {
    midi->send_short(inst, 0, 0x90 | chord[i] << 8 | 90 << 16,
                     MAESTRO_TIME_NOW);
    midi->send_short(inst, 0, 0x80 | chord[i] << 8, release);
  }

  /* A GM System On SysEx goes through send_long, which copies the data. */
  static const uint8_t gm_on[] = {0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7};
  midi->send_long(inst, 0, gm_on, sizeof gm_on, release);

  sleep_ms(1500);

  /* Silence everything and drop anything still queued, e.g. when the user
   * presses Stop. */
  if (control)
    control->reset(inst);

  maestro.close(inst);
  return 0;
}
