#!/usr/bin/env python3
"""Carve the embedded sysprog*.uout overlay ELFs out of triton_mid.bin.

The Triton monitor image is monolithic: after the kernel code it stores the
loadable system-program overlays (the `sysprog*.uout` modules) back to back as
normal little-endian 32-bit ARM ET_EXEC ELF files. They are relocated and
relocated+run at monitor boot; their object-service registrations (e.g. kernel
object 0x64, the OSAL utility/message service) are therefore invisible to a
flat disassembly of the image. This script locates every embedded ARM ELF and
splits them on the back-to-back boundaries, naming each by the `SysProg*.cpp`
source string it embeds. Read-only: it never writes to the firmware tree.

Usage: extract_sysprog_overlays.py <triton_mid.bin> <out_dir>
"""
import re
import struct
import sys
import os


def is_arm_elf(data: bytes, i: int) -> bool:
    if i + 20 > len(data):
        return False
    return (
        data[i : i + 4] == b"\x7fELF"
        and data[i + 4] == 1  # ELFCLASS32
        and data[i + 5] == 1  # ELFDATA2LSB
        and struct.unpack_from("<H", data, i + 18)[0] == 40  # EM_ARM
    )


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    image, out_dir = sys.argv[1], sys.argv[2]
    data = open(image, "rb").read()
    os.makedirs(out_dir, exist_ok=True)

    offsets = []
    pos = 0
    while True:
        i = data.find(b"\x7fELF", pos)
        if i < 0:
            break
        pos = i + 1
        if is_arm_elf(data, i):
            offsets.append(i)
    offsets = sorted(set(offsets))

    for k, start in enumerate(offsets):
        end = offsets[k + 1] if k + 1 < len(offsets) else len(data)
        blob = data[start:end]
        found = re.findall(rb"SysProg([A-Za-z]+)\.cpp", blob[:0x8000])
        name = "sysprog" + found[0].decode().lower() if found else f"overlay{k}"
        path = os.path.join(out_dir, f"{name}.elf")
        open(path, "wb").write(blob)
        print(f"{name:20s} fileoff=0x{start:06x} size=0x{len(blob):x} -> {path}")
    print(f"carved {len(offsets)} overlays")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())