#!/usr/bin/env python3
"""Compare exported ELF dynamic symbols with the reviewed C ABI v1 manifest."""
from __future__ import annotations

import pathlib
import struct
import sys


def cstring(table: bytes, offset: int) -> str:
    end = table.find(b"\0", offset)
    if end < 0:
        end = len(table)
    return table[offset:end].decode("ascii", "replace")


def elf_exports(path: pathlib.Path) -> set[str]:
    data = path.read_bytes()
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        raise ValueError("check_abi.py currently requires little-endian ELF64")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data, 0)
    shoff, shentsize, shnum = header[6], header[11], header[12]
    sections = [struct.unpack_from("<IIQQQQIIQQ", data, shoff + index * shentsize) for index in range(shnum)]
    exports: set[str] = set()
    for section in sections:
        section_type, offset, size, link, entsize = section[1], section[4], section[5], section[6], section[9]
        if section_type != 11:  # SHT_DYNSYM
            continue
        strings_section = sections[link]
        strings = data[strings_section[4]: strings_section[4] + strings_section[5]]
        entry_size = entsize or 24
        for position in range(offset, offset + size, entry_size):
            name_offset, info, other, shndx, _value, _size = struct.unpack_from("<IBBHQQ", data, position)
            binding = info >> 4
            visibility = other & 3
            if shndx == 0 or binding not in (1, 2) or visibility not in (0, 3):
                continue
            name = cstring(strings, name_offset)
            if name.startswith("ibaudio_"):
                exports.add(name)
    return exports


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: check_abi.py LIBIBAUDIO ABI_MANIFEST", file=sys.stderr)
        return 2
    actual = elf_exports(pathlib.Path(sys.argv[1]))
    expected = {line.strip() for line in pathlib.Path(sys.argv[2]).read_text().splitlines() if line.strip()}
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing or extra:
        print("ABI mismatch", file=sys.stderr)
        if missing:
            print("missing: " + ", ".join(missing), file=sys.stderr)
        if extra:
            print("extra: " + ", ".join(extra), file=sys.stderr)
        return 1
    print(f"ABI OK: {len(actual)} exported ibaudio_* symbols match v1 manifest")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
