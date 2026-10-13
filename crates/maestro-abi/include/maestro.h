/*
 * maestro.h: the Maestro API 1.0.
 *
 * Conventions:
 *  - Strings are UTF-8 and NUL-terminated. The library copies any string it
 *    needs to keep past the end of a call.
 *  - Booleans are uint32_t (0 or 1). Enums are passed as uint32_t, and both
 *    sides must tolerate values they don't recognize.
 *  - Structs that may grow start with a uint32_t struct_size field. New fields
 *    are only ever added at the end.
 *  - Timestamps use the platform's native monotonic clock:
 *    QueryPerformanceCounter ticks on Windows, CLOCK_MONOTONIC nanoseconds on
 *    Linux, mach_absolute_time ticks on macOS. A timestamp of 0 (or one in the
 *    past) plays as soon as possible. Timestamps within an instance should not
 *    decrease.
 *  - Threading is noted on each function:
 *      [rt]   realtime-safe: no allocation or I/O, and no locks except while
 *      the engine is being rebuilt after a settings change. Any thread.
 *      [any]  may block. Any thread.
 *      [ctrl] may block. Must not run concurrently with other calls on the
 *      same instance.
 *  - Callbacks may run on any thread. Keep them short and don't call back into
 *    Maestro from inside them.
 *  - A failing function returns a MaestroResult below 0 and
 *    maestro_last_error() describes it.
 *
 * The library is installed as maestro.dll, libmaestro.so.1 or
 * libmaestro.1.dylib. Link against it, or load it at runtime and resolve the
 * seven core functions; either way, check maestro_get_version() first:
 *
 *     MaestroOpenParams params = { sizeof params };
 *     params.app_id = "org.example.app";
 *     MaestroInstance *inst;
 *     if (maestro_open(MAESTRO_API_VERSION, &params, &inst) != MAESTRO_OK) {
 * ... } const MaestroExtMidi1 *midi = maestro_get_extension(inst,
 * MAESTRO_EXT_MIDI1); midi->send_short(inst, 0, 0x7F3C90, MAESTRO_TIME_NOW);
 */
#ifndef MAESTRO_H
#define MAESTRO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
#define MAESTRO_CALL __cdecl
#define MAESTRO_API __declspec(dllimport)
#else
#define MAESTRO_CALL
#define MAESTRO_API
#endif

#define MAESTRO_VERSION_MAJOR 1
#define MAESTRO_VERSION_MINOR 0
#define MAESTRO_MAKE_VERSION(major, minor)                                     \
  (((uint32_t)(major) << 16) | (uint32_t)(minor))
#define MAESTRO_API_VERSION                                                    \
  MAESTRO_MAKE_VERSION(MAESTRO_VERSION_MAJOR, MAESTRO_VERSION_MINOR)

typedef uint64_t MaestroTime;
#define MAESTRO_TIME_NOW ((MaestroTime)0)

typedef struct MaestroInstance MaestroInstance; /* opaque */

/* Returned by every fallible call. Anything below 0 is an error, and
 * maestro_last_error() describes it. */
typedef int32_t MaestroResult;
enum {
  MAESTRO_OK = 0,
  MAESTRO_ERR_INVALID_ARG = -1,
  MAESTRO_ERR_UNSUPPORTED =
      -2, /* the requested API version is newer than this library */
  MAESTRO_ERR_DISABLED =
      -3, /* the user disabled the API, or this application's profile */
  MAESTRO_ERR_UNAVAILABLE = -4, /* the audio device could not be opened */
  MAESTRO_ERR_NOT_RUNNING = -5, /* the engine is stopped or restarting */
  MAESTRO_ERR_FULL = -6,        /* event queue full; the event was dropped */
  MAESTRO_ERR_INTERNAL = -7 /* includes a panic caught at the FFI boundary */
};

/* Severity passed to the log callback. */
enum {
  MAESTRO_LOG_ERROR = 0,
  MAESTRO_LOG_WARN = 1,
  MAESTRO_LOG_INFO = 2,
  MAESTRO_LOG_DEBUG = 3
};

/* What the notify callback reports. */
enum {
  MAESTRO_NOTIFY_SETTINGS_CHANGED =
      0, /* the user edited this app's profile; re-query maestro.info */
  MAESTRO_NOTIFY_ENGINE_STOPPED =
      1, /* the profile was disabled, or the engine failed to restart */
  MAESTRO_NOTIFY_ENGINE_STARTED = 2
};

typedef void(MAESTRO_CALL *MaestroLogFn)(void *user, uint32_t level,
                                         const char *message);
typedef void(MAESTRO_CALL *MaestroNotifyFn)(void *user, uint32_t what);

/* What the app would prefer. 0 means "no preference". Both shape the engine
 * on every open. */
typedef struct MaestroHints {
  uint32_t struct_size;
  uint32_t num_ports; /* MIDI 1 ports the app will address, 1-16 (default 1) */
  uint32_t midi2;     /* 1 gives the app all 16 UMP groups as ports */
} MaestroHints;

typedef struct MaestroOpenParams {
  uint32_t struct_size;
  const char *app_id; /* required. Stable reverse-DNS ID, e.g. "org.example.app"
                       * ([A-Za-z0-9._-]). Selects the user's profile */
  const char *app_name;      /* shown in Maestro's settings; may be NULL */
  const MaestroHints *hints; /* may be NULL */
  MaestroLogFn log;          /* may be NULL */
  MaestroNotifyFn notify;    /* may be NULL */
  void *user;                /* passed to both callbacks */
} MaestroOpenParams;

typedef struct MaestroInfo {
  uint32_t struct_size; /* set by the caller */
  uint32_t sample_rate;
  uint32_t num_ports;
  uint32_t midi2;
  uint32_t render_block_us;
} MaestroInfo;

typedef struct MaestroStats {
  uint32_t struct_size;
  uint64_t nps;       /* notes per second processed by the synth */
  uint64_t eps;       /* events per second processed by the synth */
  uint64_t nps_limit; /* the current limit, after the load limiter */
} MaestroStats;

/* ---- Core ---- */

/* [rt] Version of the installed library, as MAESTRO_MAKE_VERSION. Use the
 * library only if its major matches and its minor is at least yours. */
MAESTRO_API uint32_t MAESTRO_CALL maestro_get_version(void);

/* [rt] Description of the last error on the calling thread. Valid until this
 * thread's next Maestro call. Never NULL. */
MAESTRO_API const char *MAESTRO_CALL maestro_last_error(void);

/* [rt] The current host time. */
MAESTRO_API MaestroTime MAESTRO_CALL maestro_now(void);

/* [rt] Converts a duration for scheduling ahead ("now + 5 ms"). */
MAESTRO_API MaestroTime MAESTRO_CALL maestro_ns_to_ticks(uint64_t ns);

/* [any] Opens an instance, with its own engine and audio output. A process can
 * have several open at once. api_version should be MAESTRO_API_VERSION. */
MAESTRO_API MaestroResult MAESTRO_CALL
maestro_open(uint32_t api_version, const MaestroOpenParams *params,
             MaestroInstance **out);

/* [ctrl] Closes the instance. After this returns, no callback for the instance
 * will run again. */
MAESTRO_API void MAESTRO_CALL maestro_close(MaestroInstance *inst);

/* [any] Returns the function table for the extension `id`, or NULL if this
 * library doesn't provide it. The pointer stays valid for the life of the
 * instance. */
MAESTRO_API const void *MAESTRO_CALL
maestro_get_extension(MaestroInstance *inst, const char *id);

/* ---- Extension: MIDI 1.0 byte stream ---- */
#define MAESTRO_EXT_MIDI1 "maestro.midi1/1"
typedef struct MaestroExtMidi1 {
  /* [rt] A packed short message: status | data1 << 8 | data2 << 16. */
  MaestroResult(MAESTRO_CALL *send_short)(MaestroInstance *inst, uint8_t port,
                                          uint32_t msg, MaestroTime at);
  /* [rt] SysEx or any long message. The data is copied before the call
   * returns. */
  MaestroResult(MAESTRO_CALL *send_long)(MaestroInstance *inst, uint8_t port,
                                         const uint8_t *data, size_t len,
                                         MaestroTime at);
} MaestroExtMidi1;

/* ---- Extension: Universal MIDI Packets ---- */
#define MAESTRO_EXT_UMP "maestro.ump/1"
typedef struct MaestroExtUmp {
  /* [rt] One or more complete UMPs, in host byte order. */
  MaestroResult(MAESTRO_CALL *send)(MaestroInstance *inst,
                                    const uint32_t *words, size_t count,
                                    MaestroTime at);
} MaestroExtUmp;

/* ---- Extension: control ---- */
#define MAESTRO_EXT_CONTROL "maestro.control/1"
typedef struct MaestroExtControl {
  /* [any] All notes off, controllers reset, and every queued event dropped. */
  MaestroResult(MAESTRO_CALL *reset)(MaestroInstance *inst);
  /* [any] Opens Maestro's settings for this app's profile so the user can
   * tune it. */
  MaestroResult(MAESTRO_CALL *show_settings)(MaestroInstance *inst);
} MaestroExtControl;

/* ---- Extension: effective settings (read-only) ---- */
#define MAESTRO_EXT_INFO "maestro.info/1"
typedef struct MaestroExtInfo {
  /* [any] The settings that resulted from the hints combined with the user's
   * profile. */
  MaestroResult(MAESTRO_CALL *get)(MaestroInstance *inst, MaestroInfo *out);
} MaestroExtInfo;

/* ---- Extension: statistics ---- */
#define MAESTRO_EXT_STATS "maestro.stats/1"
typedef struct MaestroExtStats {
  /* [rt] */
  MaestroResult(MAESTRO_CALL *get)(MaestroInstance *inst, MaestroStats *out);
} MaestroExtStats;

#ifdef __cplusplus
}
#endif
#endif /* MAESTRO_H */
