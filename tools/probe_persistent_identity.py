#!/usr/bin/env python3
"""Report Darwin volume identity capabilities without source or mount changes."""

import argparse
import ctypes
import json
import os
import platform
import struct
import uuid


class AttributeList(ctypes.Structure):
    _fields_ = [
        ("bitmapcount", ctypes.c_uint16),
        ("reserved", ctypes.c_uint16),
        ("commonattr", ctypes.c_uint32),
        ("volattr", ctypes.c_uint32),
        ("dirattr", ctypes.c_uint32),
        ("fileattr", ctypes.c_uint32),
        ("forkattr", ctypes.c_uint32),
    ]


def probe(path):
    """Read explicit capabilities and UUID from an already mounted volume root."""
    library = ctypes.CDLL(None, use_errno=True)
    library.fgetattrlist.argtypes = [
        ctypes.c_int, ctypes.POINTER(AttributeList), ctypes.c_void_p,
        ctypes.c_size_t, ctypes.c_uint32,
    ]
    library.fgetattrlist.restype = ctypes.c_int
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        request = AttributeList(5, 0, 0x80000000, 0x80060000, 0, 0, 0)
        buffer = ctypes.create_string_buffer(72)
        if library.fgetattrlist(descriptor, ctypes.byref(request), buffer, len(buffer), 0):
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error), path)
        size = struct.unpack_from("=I", buffer.raw)[0]
        returned = struct.unpack_from("=5I", buffer.raw, 4)
        if size != 72 or not returned[0] & 0x80000000 or returned[1] & 0x60000 != 0x60000:
            raise ValueError("missing or incomplete native attributes")
        capabilities = struct.unpack_from("=8I", buffer.raw, 24)
        supported = capabilities[0] & capabilities[4]
        volume_uuid = uuid.UUID(bytes=buffer.raw[56:72])
        return {
            "host": platform.node(),
            "os": platform.platform(),
            "path": path,
            "format_capabilities": hex(capabilities[0]),
            "format_valid": hex(capabilities[4]),
            "persistent_ids": bool(supported & 1),
            "persistent_nonrecycled_ids": bool(supported & 0x4000),
            "64bit_ids": bool(supported & 0x20000),
            "volume_uuid": str(volume_uuid),
            "capability_pass": supported & 0x24001 == 0x24001 and volume_uuid.int != 0,
        }
    finally:
        os.close(descriptor)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("volume_roots", nargs="+")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("this probe requires Darwin")
    records = [probe(path) for path in args.volume_roots]
    print(json.dumps(records, indent=2))
    return 0 if all(record["capability_pass"] for record in records) else 1


if __name__ == "__main__":
    raise SystemExit(main())
