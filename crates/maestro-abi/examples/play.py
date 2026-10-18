"""play.py: the Maestro API from Python, through ctypes.

Any language with a C FFI binds maestro.h the same way: declare the structs and
function signatures, load the library by its versioned name, and go.
"""

import ctypes
import sys
import time

LIBRARY = {"win32": "maestro.dll", "darwin": "libmaestro.1.dylib"}.get(sys.platform, "libmaestro.so.1")
CALLBACK = ctypes.CFUNCTYPE  # MAESTRO_CALL is __cdecl on Windows too

MAESTRO_API_VERSION = 1 << 16 | 0
MAESTRO_OK = 0
MAESTRO_TIME_NOW = 0

LogFn = CALLBACK(None, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_char_p)
NotifyFn = CALLBACK(None, ctypes.c_void_p, ctypes.c_uint32)


class OpenParams(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("app_id", ctypes.c_char_p),
        ("app_name", ctypes.c_char_p),
        ("hints", ctypes.c_void_p),
        ("log", LogFn),
        ("notify", NotifyFn),
        ("user", ctypes.c_void_p),
    ]


class Info(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("sample_rate", ctypes.c_uint32),
        ("num_ports", ctypes.c_uint32),
        ("midi2", ctypes.c_uint32),
        ("render_block_us", ctypes.c_uint32),
    ]


Instance = ctypes.c_void_p
Result = ctypes.c_int32
Time = ctypes.c_uint64


class ExtMidi1(ctypes.Structure):
    _fields_ = [
        ("send_short", CALLBACK(Result, Instance, ctypes.c_uint8, ctypes.c_uint32, Time)),
        ("send_long", CALLBACK(Result, Instance, ctypes.c_uint8,
                               ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t, Time)),
    ]


class ExtInfo(ctypes.Structure):
    _fields_ = [("get", CALLBACK(Result, Instance, ctypes.POINTER(Info)))]


lib = ctypes.CDLL(LIBRARY)
lib.maestro_get_version.restype = ctypes.c_uint32
lib.maestro_last_error.restype = ctypes.c_char_p
lib.maestro_now.restype = Time
lib.maestro_ns_to_ticks.restype = Time
lib.maestro_ns_to_ticks.argtypes = [ctypes.c_uint64]
lib.maestro_open.restype = Result
lib.maestro_open.argtypes = [ctypes.c_uint32, ctypes.POINTER(OpenParams), ctypes.POINTER(Instance)]
lib.maestro_close.argtypes = [Instance]
lib.maestro_get_extension.restype = ctypes.c_void_p
lib.maestro_get_extension.argtypes = [Instance, ctypes.c_char_p]


def extension(inst, ext_id, table):
    ptr = lib.maestro_get_extension(inst, ext_id)
    if not ptr:
        raise RuntimeError(f"Maestro lacks the {ext_id.decode()} extension")
    return ctypes.cast(ptr, ctypes.POINTER(table)).contents


def on_log(_user, level, message):
    print(f"[maestro {level}] {message.decode()}", file=sys.stderr)


def main():
    version = lib.maestro_get_version()
    if version >> 16 != MAESTRO_API_VERSION >> 16 or version & 0xFFFF < MAESTRO_API_VERSION & 0xFFFF:
        sys.exit(f"Unsupported Maestro API version {version >> 16}.{version & 0xFFFF}")

    # The callback object must outlive the instance, or ctypes frees it.
    log = LogFn(on_log)
    params = OpenParams(
        struct_size=ctypes.sizeof(OpenParams),
        app_id=b"org.example.maestro.python",
        app_name=b"Maestro API example: Python",
        log=log,
    )
    inst = Instance()
    if lib.maestro_open(MAESTRO_API_VERSION, ctypes.byref(params), ctypes.byref(inst)) != MAESTRO_OK:
        sys.exit(f"maestro_open: {lib.maestro_last_error().decode()}")

    try:
        info = Info(struct_size=ctypes.sizeof(Info))
        if extension(inst, b"maestro.info/1", ExtInfo).get(inst, ctypes.byref(info)) == MAESTRO_OK:
            print(f"{info.sample_rate} Hz, blocks of {info.render_block_us} us")

        midi = extension(inst, b"maestro.midi1/1", ExtMidi1)
        ms = lambda n: lib.maestro_ns_to_ticks(n * 1_000_000)

        # "Twinkle twinkle", scheduled ahead in one go.
        tune = [60, 60, 67, 67, 69, 69, 67]
        at = lib.maestro_now() + ms(50)
        for note in tune:
            midi.send_short(inst, 0, 0x90 | note << 8 | 100 << 16, at)
            midi.send_short(inst, 0, 0x80 | note << 8, at + ms(350))
            at += ms(400)

        time.sleep(len(tune) * 0.4 + 1)
    finally:
        lib.maestro_close(inst)


if __name__ == "__main__":
    main()
