/*
 * runtime_load.c: loading the Maestro API at runtime.
 *
 * An application that should still start when Maestro isn't installed loads
 * the library itself instead of linking against it, then resolves its one
 * export, maestro_get_interface, by name. It also shows the log callback,
 * telling the user why the API is unavailable, and reset.
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
#define LIBRARY_NAME "libmaestro.dylib"
#else
#define LIBRARY_NAME "libmaestro.so"
#endif
#define open_library() dlopen(LIBRARY_NAME, RTLD_NOW | RTLD_LOCAL)
#define find_symbol(lib, name) dlsym((lib), (name))
typedef void *Library;
#endif

#define MAESTRO_NO_PROTOTYPES
#include "maestro.h"

static const MaestroCoreV1 *core;
static const MaestroRealtimeV1 *rt;

static int load_maestro(void) {
  Library lib = open_library();
  if (!lib)
    return 0;

  MaestroGetInterfaceFn get_interface;
  *(void **)&get_interface = find_symbol(lib, "maestro_get_interface");
  if (!get_interface)
    return 0;

  core = get_interface(MAESTRO_CORE_V1);
  rt = get_interface(MAESTRO_REALTIME_V1);
  return core && rt;
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

  MaestroAppInfo app = {sizeof app};
  app.app_id = "org.example.maestro.runtime-load";
  app.app_name = "Maestro API example: runtime loading";
  app.log = on_log;

  MaestroRealtime *inst;
  MaestroResult r = rt->open(&app, NULL, &inst);
  switch (r) {
  case MAESTRO_OK:
    break;
  case MAESTRO_ERR_DISABLED:
    /* The user's choice, not a fault: fall back quietly. */
    printf("Maestro is turned off for this application: %s\n",
           core->last_error());
    return 0;
  default:
    fprintf(stderr, "open failed (%d): %s\n", r, core->last_error());
    return 1;
  }

  /* A chord, played now and released a second later. */
  static const uint8_t chord[] = {48, 55, 64, 67};
  MaestroTime release = rt->now() + rt->ns_to_ticks(1000000000ull);
  for (size_t i = 0; i < sizeof chord; i++) {
    rt->send_short(inst, 0, 0x90 | chord[i] << 8 | 90 << 16, MAESTRO_TIME_NOW);
    rt->send_short(inst, 0, 0x80 | chord[i] << 8, release);
  }

  /* A GM System On SysEx goes through send_long, which copies the data. */
  static const uint8_t gm_on[] = {0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7};
  rt->send_long(inst, 0, gm_on, sizeof gm_on, release);

  sleep_ms(1500);

  /* Silence everything and drop anything still queued, e.g. when the user
   * presses Stop. */
  rt->reset(inst);

  rt->close(inst);
  return 0;
}
