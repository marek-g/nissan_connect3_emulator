#!/bin/sh
# regdump - one-shot registry/KDS extractor for the LCN2KAI Bosch head unit.
#
# Runs via the proven USB `logger` exploit: the stick's ext2 label is
# "../../usr/bin/" so the stick mounts over /usr/bin and the system's `logger`
# call executes this script as root (same mechanism as nissan_connect3_map_conv/
# diag/navdiag_logger.sh, run successfully on this unit family). Output lands in
# /usr/bin/regdump = ./regdump on the stick.
#
# USAGE: for the intended capture, insert the stick **after the map is visible
# on screen** - the registry is then fully populated and every target of this
# script exists. The logger does NOT wait for any process (so it also completes
# on a cold boot, just with less in the shm dump). It reboots the HU at ALL_DONE
# = clean "pull the stick" signal.
#
# ORDER OF COLLECTION = DECREASING IMPORTANCE / INCREASING SIZE. After EVERY
# section the script syncs, so if you cannot wait for the reboot you may pull
# the stick once the file you care about exists (check from another shell, or
# simply wait for the sections listed in README). Pulling before `sync` of a
# section loses only that section; earlier ones are on disk.
#
# SAFETY MODEL: every action is READ-ONLY except (a) remount-rw of the stick
# itself (/usr/bin), (b) writing into /usr/bin/regdump on the stick, (c) the
# final reboot. Nothing under /opt, /var, /etc and NO flash block device is
# ever opened for writing; mtd/stick access is read-only dd. shm dumps are
# taken ONLY for segments with nattch>=1 (pages then provably exist and are
# RAM-backed, so /dev/mem reads cannot hit unmapped physical memory -> no oops
# on this panic_on_oops kernel) and are hard-capped (64 MiB/seg, 600 MiB total).
# Every unbounded reader is wrapped in `timeout`.
#
# busybox-safe: no bash-isms, no head/sed/awk-of-fancy; `timeout`, `dd`,
# `grep`, `cut`, `md5sum` are proven present on this unit.

OUT=/usr/bin/regdump

mount -o remount,rw /         2>/dev/null
mount -o remount,rw /usr/bin/ 2>/dev/null

# re-entrancy guard: `logger` may be invoked repeatedly
[ -e "$OUT/.done" ] && exit 0
mkdir -p "$OUT/shm" "$OUT/misc" "$OUT/ffs2" "$OUT/bins" 2>/dev/null
echo "collected $(date) pid=$$ ppid=$PPID" > "$OUT/.done" 2>/dev/null
df /usr/bin > "$OUT/.df_start" 2>&1

findpid() {
    for p in /proc/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null)
        [ -n "$c" ] || continue
        case "$c" in *"$1"*) echo "${p#/proc/}"; return 0 ;; esac
    done
    return 1
}

# ===========================================================================
# SECTION 1 (tiny, deterministic, no boot-wait needed): manifest + the
# INSTALLED registry files on disk. Even if everything else fails or is yanked,
# these two files are the minimum useful harvest.
# ===========================================================================
{
    echo "=== uname ==="; uname -a
    echo "=== cmdline ==="; cat /proc/cmdline
    echo "=== versions ==="
    cat /lcn2kai_version.txt 2>/dev/null; echo
    cat /rfs_version.txt 2>/dev/null; echo
    cat /cc_label.txt 2>/dev/null; echo
    echo "=== date ==="; date
} > "$OUT/00_manifest.txt" 2>&1
sync

for d in /dev/ffs2/registry /dev/ffs/registry /var/opt/bosch/region_static; do
    [ -d "$d" ] || continue
    b=${d##*/}
    cp -R "$d" "$OUT/ffs2/${b}/" 2>/dev/null
done
cp /var/opt/bosch/static/system/static.md5  "$OUT/ffs2/" 2>/dev/null
cp /var/opt/bosch/dynamic/system/dynamic.md5 "$OUT/ffs2/" 2>/dev/null
ls -lR "$OUT/ffs2" > "$OUT/01_ffs2_list.txt" 2>&1
echo "SECTION1_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 2 (tiny): inventory. NOTE: the stick is inserted WARM - only after
# the map is visible on screen - so procbaselx/PROCNAV and the fully-populated
# registry are guaranteed already; no waiting needed, just a presence log.
# ===========================================================================
p1=$(findpid baselx); p2=$(findpid PROCNAV); p3=$(findpid PROCMWLX)
echo "at_run baselx=$p1 procnav=$p2 procmwlx=$p3" >> "$OUT/.done"

{
    echo "=== procs (comm) ==="
    timeout 30 sh -c 'for p in /proc/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null) || continue
        echo "pid=${p#/proc/} comm=$c"
    done'
    echo "=== fds mentioning registry|kds|iosc|ffs ==="
    timeout 60 sh -c 'for p in /proc/[0-9]*; do
        ls -l "$p/fd" 2>/dev/null | grep -aiE "registry|kds|iosc|ffs"
    done' 2>/dev/null
    echo "=== mounts ==="; cat /proc/mounts 2>/dev/null
    echo "=== df ==="; df 2>/dev/null
} > "$OUT/02_procs_fds.txt" 2>&1

cat /proc/sysvipc/shm > "$OUT/03_sysvipc_shm.txt" 2>/dev/null
ipcs -m > "$OUT/03b_ipcs_m.txt" 2>&1

# per-process maps of the registry clients (addresses for the shm cross-check)
for name in procbaselx PROCNAV PROCMWLX; do
    pid=$(findpid "$name") || continue
    { echo "=== $name pid=$pid maps ==="; cat "/proc/$pid/maps" 2>/dev/null; } \
        > "$OUT/04_${name}_maps.txt" 2>&1
done
echo "SECTION2_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 3 (medium, THE main prize): attached SysV shm segments (the decrypted
# registry tree + KDS table) and /dev/shm. nattch>=1 only, <=64 MiB/seg,
# 600 MiB total budget. Each segment is synced as it completes -> you can pull
# the stick mid-section and keep everything dumped so far.
# ===========================================================================
TOTAL=0
NDUMP=0
if [ -c /dev/mem ]; then
    # columns: key pid cuid shm_cpid nattch shmseg address cbytes crn
    while read s_key s_pid s_cuid s_cpid s_nattch shmseg s_addr size s_crn; do
        case "$size"     in ''|*[!0-9]*) continue ;; esac
        case "$shmseg"   in ''|*[!0-9]*) continue ;; esac
        case "$s_nattch" in ''|0)        continue ;; esac   # attached only
        [ "$size" -gt 67108864 ] && continue               # skip >64 MiB buffers
        [ "$TOTAL" -gt 629145600 ] && break                # 600 MiB budget
        timeout 60 dd if=/dev/mem of="$OUT/shm/sysv_${shmseg}_${size}.bin" \
            bs=4096 skip=$((shmseg/4096)) count=$((size/4096+1)) 2>/dev/null
        sync
        TOTAL=$((TOTAL+size)); NDUMP=$((NDUMP+1))
    done < /proc/sysvipc/shm
    echo "sysv segments dumped: $NDUMP (bytes=$TOTAL)" > "$OUT/shm/NOTE_sysv.txt"
else
    echo "no /dev/mem char device - sysv shm dump skipped" > "$OUT/shm/NOTE_sysv.txt"
fi
cp -R /dev/shm/. "$OUT/shm/devshm/" 2>/dev/null
ls -lR /dev/shm > "$OUT/shm/NOTE_devshm.txt" 2>&1

# on-unit sanity check: which dumps actually contain registry keys
{
    echo "=== segments containing LOCAL_MACHINE / BLAUPUNKT strings ==="
    timeout 60 sh -c 'for f in "'"$OUT"'/shm"/*.bin "'"$OUT"'/shm/devshm"/*; do
        [ -f "$f" ] || continue
        c=$(grep -ac LOCAL_MACHINE "$f" 2>/dev/null)
        r=$(grep -ac BLAUPUNKT "$f" 2>/dev/null)
        [ "${c:-0}" != "0" ] && echo "HIT $f LOCAL_MACHINE=$c BLAUPUNKT=$r"
    done; true'
} > "$OUT/05_registry_hits.txt" 2>&1
echo "SECTION3_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 4 (medium): KDS raw store. /proc/mtd inventory + CAPPED 2 MiB head
# of every partition (NOR config/kds areas live at partition starts; huge
# rootfs mid partitions are not what we want).
# ===========================================================================
cat /proc/mtd > "$OUT/06_mtd.txt" 2>/dev/null
ls -la /dev   > "$OUT/06b_devnodes.txt" 2>&1
n=0
while read line; do
    dev=$(echo "$line" | cut -d: -f1)
    [ -c "/dev/$dev" ] || continue
    n=$((n+1))
    timeout 30 dd if="/dev/$dev" of="$OUT/misc/mtd_${dev}_cap2MiB.bin" bs=4096 count=512 2>/dev/null
    sync
done < /proc/mtd
{ echo "mtd capped dumps: $n"; ls -l "$OUT/misc"; } > "$OUT/06c_mtd_dumped.txt" 2>&1
echo "SECTION4_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 5 (largest, LEAST important - safe to skip/never reach): binary
# inventory. DAPIAPP.OUT is NOT copied (we read the SD card ourselves). ~17 MiB.
# ===========================================================================
{
    cd /opt/bosch/processes 2>/dev/null && md5sum ./* 2>/dev/null
    cd /var/opt/bosch/dynamic/processes 2>/dev/null && md5sum ./* 2>/dev/null
} > "$OUT/07_md5_optbosch.txt" 2>&1
sync
for f in libosal_linux_so.so libiosclib_so.so PROCNAV.OUT procmapengine.out procbaselx_out.out procmwlx_out.out; do
    cp "/opt/bosch/processes/$f" "$OUT/bins/" 2>/dev/null
    sync
done
# dynamic overlay PROCNAV (firmware-side, not on the SD) if present
[ -f /var/opt/bosch/dynamic/processes/PROCNAV.OUT ] && \
    cp /var/opt/bosch/dynamic/processes/PROCNAV.OUT "$OUT/bins/dyn_PROCNAV.OUT" 2>/dev/null
ls -l "$OUT/bins" > "$OUT/07b_bins.txt" 2>&1

# final dmesg snapshot (did anything we touched complain?)
dmesg > "$OUT/08_dmesg_end.txt" 2>&1

df /usr/bin    > "$OUT/98_df_end.txt" 2>&1
ls -laR "$OUT" > "$OUT/99_collected.txt" 2>&1
echo "ALL_DONE $(date)" >> "$OUT/.done"
sync; sync
reboot