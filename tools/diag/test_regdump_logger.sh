#!/bin/sh
# Dry-run regdump_logger.sh on a development machine. NEVER touches the host:
#  * OUT is redirected to a temp dir (nothing is written to /usr/bin or /dev),
#  * the mount/remount lines and the /dev/mem dd lines are stubbed,
#  * reboot is doubly disabled: REGDUMP_NO_REBOOT=1 AND the script's own
#    kernel guard (only 2.6.34* kernels may reboot). Run this instead of
#    hand-sed'ing the logger - hand-sed'ing once missed a reboot line.
set -e

SRC=$(cd "$(dirname "$0")" && pwd)/regdump_logger.sh
WORK=${1:-/tmp/opencode/simtest}
mkdir -p "$WORK"
SIM="$WORK/sim.sh"

sed \
    -e 's|^OUT=/usr/bin/regdump$|OUT='"$WORK"'/regdump|' \
    -e 's/^mount -o remount.*$/:/' \
    -e 's|^    dd if=/dev/mem of="\$OUT/iosc.*$|    : stub-dd-iosc|' \
    -e 's|^    cp "/opt/bosch/processes/\$f" "\$OUT/bins/\$f" 2>/dev/null|    : stub-cp-bin|' \
    -e 's|^    reboot$|    : stub-reboot|' \
    "$SRC" > "$SIM"

# belt and braces: the generated sim must contain no live reboot/mount/dd-to-host
if grep -nE '^[[:space:]]*(reboot|mount )' "$SIM"; then
    echo "FATAL: stubbing incomplete, refusing to run" >&2
    exit 1
fi
if grep -nE 'dd if=/dev/(mem|mmcblk|mtd)' "$SIM" | grep -v 'stub'; then
    echo "FATAL: live /dev dump left in sim, refusing to run" >&2
    exit 1
fi

sh -n "$SIM"
rm -rf "$WORK/regdump"
REGDUMP_NO_REBOOT=1
export REGDUMP_NO_REBOOT
sh "$SIM"
echo "=== markers ==="
cat "$WORK/regdump/.done"
grep -q "reboot SKIPPED" "$WORK/regdump/.done" || { echo "FATAL: reboot guard did not fire"; exit 1; }
echo "OK: dry-run complete, host untouched, reboot guard verified"
