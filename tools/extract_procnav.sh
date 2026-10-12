#!/bin/sh
# Regenerate the repo-local navbin (data/navbin/PROCNAV.OUT) from the map SD
# card's ULI container using the LX monitor's own LZ decompressor.
#
# The emulator serves /opt/bosch/navbin from <repo>/nissan_connect3_emulator/
# data/navbin (override: EMU_NAVBIN_DIR). The unpacked ELF is a derived
# Bosch binary and is NOT committed; if the file is missing the emulator logs
# "has no PROCNAV.OUT" at startup and PROCNAV silently never starts (run r347).
#
# Requires python3 + unicorn (uv venv /tmp/opencode/pyul; uv pip install unicorn).
set -e
cd "$(dirname "$0")/.."

CARD="/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/NISSAN Connect LCN3 V7 2022_2023"
UNPACK=/home/marek/Ext/Src/navi/nissan_connect3_map_conv/src/elf_decompressor/uli_unpack.py
OUT=nissan_connect3_emulator/data/navbin/PROCNAV.OUT
PY=${PY:-/tmp/opencode/pyul/bin/python}

[ -f "$OUT" ] && { echo "$OUT already present ($(stat -c%s "$OUT") bytes); rm it to re-extract"; exit 0; }
mkdir -p "$(dirname "$OUT")"
"$PY" "$UNPACK" "$CARD/CRYPTNAV/DNL/BIN/NAV/COMMON/PROCNAV.OUT" "$OUT"
readelf -h "$OUT" | grep -i "entry point"
