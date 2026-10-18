"""play.py: the Maestro API from Python, through ctypes.

Any language with a C FFI binds maestro.h the same way: declare the structs and
function signatures, load the library by name, and go.
"""

import ctypes
import sys
import time

LIBRARY = {"win32": "maestro.dll", "darwin": "libmaestro.dylib"}.get(sys.platform, "libmaestro.so")
CALLBACK = ctypes.CFUNCTYPE  # MAESTRO_CALL is __cdecl on Windows too

MAESTRO_OK = 0
MAESTRO_TIME_NOW = 0

LogFn = CALLBACK(None, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_char_p)


class AppInfo(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("reserved", ctypes.c_uint32),
        ("app_id", ctypes.c_char_p),
        ("app_name", ctypes.c_char_p),
        ("log", LogFn),
        ("user", ctypes.c_void_p),
    ]


class RealtimeInfo(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("sample_rate", ctypes.c_uint32),
        ("num_ports", ctypes.c_uint32),
        ("midi2", ctypes.c_uint32),
        ("render_block_us", ctypes.c_uint32),
        ("max_schedule_us", ctypes.c_uint32),
    ]


Handle = ctypes.c_void_p
Result = ctypes.c_int32
Time = ctypes.c_uint64


class CoreV1(ctypes.Structure):
    _fields_ = [
        ("get_version", CALLBACK(ctypes.c_uint32)),
        ("last_error", CALLBACK(ctypes.c_char_p)),
    ]


# Every field up to the last one used must be declared, in the header's order.
class RealtimeV1(ctypes.Structure):
    _fields_ = [
        ("open", CALLBACK(Result, ctypes.POINTER(AppInfo), ctypes.c_void_p, ctypes.POINTER(Handle))),
        ("close", CALLBACK(None, Handle)),
        ("now", CALLBACK(Time)),
        ("ns_to_ticks", CALLBACK(Time, ctypes.c_uint64)),
        ("send_short", CALLBACK(Result, Handle, ctypes.c_uint8, ctypes.c_uint32, Time)),
        ("send_long", CALLBACK(Result, Handle, ctypes.c_uint8,
                               ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t, Time)),
        ("send_ump", CALLBACK(Result, Handle, ctypes.POINTER(ctypes.c_uint32), ctypes.c_size_t, Time)),
        ("get_info", CALLBACK(Result, Handle, ctypes.POINTER(RealtimeInfo))),
    ]


lib = ctypes.CDLL(LIBRARY)
lib.maestro_get_interface.restype = ctypes.c_void_p
lib.maestro_get_interface.argtypes = [ctypes.c_char_p]


def interface(interface_id, table):
    ptr = lib.maestro_get_interface(interface_id)
    if not ptr:
        sys.exit(f"The installed Maestro lacks {interface_id.decode()}; update it")
    return ctypes.cast(ptr, ctypes.POINTER(table)).contents


def on_log(_user, level, message):
    print(f"[maestro {level}] {message.decode()}", file=sys.stderr)


def main():
    core = interface(b"maestro.core/1", CoreV1)
    rt = interface(b"maestro.realtime/1", RealtimeV1)

    # The callback object must outlive the engine, or ctypes frees it.
    log = LogFn(on_log)
    app = AppInfo(
        struct_size=ctypes.sizeof(AppInfo),
        app_id=b"org.example.maestro.python",
        app_name=b"Maestro API example: Python",
        log=log,
    )
    inst = Handle()
    if rt.open(ctypes.byref(app), None, ctypes.byref(inst)) != MAESTRO_OK:
        sys.exit(f"open: {core.last_error().decode()}")

    try:
        info = RealtimeInfo(struct_size=ctypes.sizeof(RealtimeInfo))
        if rt.get_info(inst, ctypes.byref(info)) == MAESTRO_OK:
            print(f"{info.sample_rate} Hz, blocks of {info.render_block_us} us")

        ms = lambda n: rt.ns_to_ticks(n * 1_000_000)

        # "Twinkle twinkle", scheduled ahead in one go.
        tune = [60, 60, 67, 67, 69, 69, 67]
        at = rt.now() + ms(50)
        for note in tune:
            rt.send_short(inst, 0, 0x90 | note << 8 | 100 << 16, at)
            rt.send_short(inst, 0, 0x80 | note << 8, at + ms(350))
            at += ms(400)

        time.sleep(len(tune) * 0.4 + 1)
    finally:
        rt.close(inst)


if __name__ == "__main__":
    main()
