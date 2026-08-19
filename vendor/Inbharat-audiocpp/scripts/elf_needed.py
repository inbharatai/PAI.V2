#!/usr/bin/env python3
"""Print DT_NEEDED entries from a little-endian ELF64 file without host binutils."""
from __future__ import annotations
import pathlib
import struct
import sys


def cstring(data: bytes, offset: int) -> str:
    end = data.find(b"\0", offset)
    return data[offset:end].decode("ascii", "replace")


def main() -> int:
    data = pathlib.Path(sys.argv[1]).read_bytes()
    if data[:6] != b"\x7fELF\x02\x01":
        raise SystemExit("expected little-endian ELF64")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
    shoff, shentsize, shnum = header[6], header[11], header[12]
    sections = [struct.unpack_from("<IIQQQQIIQQ", data, shoff + i * shentsize) for i in range(shnum)]
    needed: list[str] = []
    for section in sections:
        if section[1] != 6:  # SHT_DYNAMIC
            continue
        strings_section = sections[section[6]]
        strings = data[strings_section[4]:strings_section[4] + strings_section[5]]
        for offset in range(section[4], section[4] + section[5], section[9] or 16):
            tag, value = struct.unpack_from("<QQ", data, offset)
            if tag == 1:
                needed.append(cstring(strings, value))
    for entry in needed:
        print(entry)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
