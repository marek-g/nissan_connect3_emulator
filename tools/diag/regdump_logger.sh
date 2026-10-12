#!/bin/sh
# regdump - one-shot registry/KDS extractor for the LCN2KAI Bosch head unit.
#
# Runs via the proven USB `logger` exploit: the stick's ext2 label is
# "../../usr/bin/" so the stick mounts over /usr/bin and the system's `logger`
# call executes this script as root (same mechanism as nissan_connect3_map_conv/
# diag/navdiag_logger.sh). Output lands in /usr/bin/regdump = ./regdump on the
# stick; the script reboots the unit when done (= pull the stick).
#
# What it collects (priority order):
#   1. ALL SysV shm segments + /dev/shm files  -> the DECRYPTED registry tree
#      lives in the IOSC registry shm (KDS entries live in KDS_SHME). Post-process
#      on the host: strings/grep for "LOCAL_MACHINE" to pick the right segment.
#   2. /dev/ffs2/registry (installed .reg/.ERG), region_static, custver files
#   3. KDS surface: /proc/*/fd kds holders, /dev/mtd* (flashblock_kds = raw key store)
#   4. binary inventory (md5) + copies of libosal + nav apps for D605-vs-car diff
#
# busybox-safe: no bash-isms, no head/tail/sed, `timeout` and `dd` exist (proven).

OUT=/usr/bin/regdump

mount -o remount,rw /        2>/dev/null
mount -o remount,rw /usr/bin/ 2>/dev/null

[ -e "$OUT/.done" ] && exit 0
mkdir -p "$OUT/shm" "$OUT/misc" 2>/dev/null
echo "collected $(date) pid=$$ ppid=$PPID" > "$OUT/.done" 2>/dev/null

findpid() {
    for p in /proc/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null)
        [ -n "$c" ] || continue
        case "$c" in *"$1"*) echo "${p#/proc/}"; return 0 ;; esac
    done
    return 1
}

# ---------------------------------------------------------------------------
# 0. wait until the registry is populated: procbaselx (registry/IOSC host) and
#    PROCNAV must be up, then give the boot 30 more seconds for the last ERG
#    merges. Cap total wait at 180s so a broken unit still produces a dump.
# ---------------------------------------------------------------------------
i=0
while [ $i -lt 180 ]; do
    p1=$(findpid baselx); p2=$(findpid PROCNAV)
    [ -n "$p1" ] && [ -n "$p2" ] && break
    sleep 5; i=$((i+5))
done
sleep 30
echo "wait_done=$i baselx=$p1 procnav=$p2" >> "$OUT/.done"

# ---------------------------------------------------------------------------
# 1. manifest / environment
# ---------------------------------------------------------------------------
{
    echo "=== uname ==="; uname -a
    echo "=== cmdline ==="; cat /proc/cmdline
    echo "=== versions ==="
    cat /lcn2kai_version.txt 2>/dev/null; echo
    cat /rfs_version.txt 2>/dev/null; echo
    cat /cc_label.txt 2>/dev/null; echo
    echo "=== df ==="; df 2>/dev/null
    echo "=== mounts ==="; cat /proc/mounts 2>/dev/null
    echo "=== date ==="; date
} > "$OUT/00_manifest.txt" 2>&1

# registry/KDS-relevant process + fd inventory
{
    echo "=== procs (comm) ==="
    for p in /proc/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null) || continue
        echo "pid=${p#/proc/} comm=$c"
    done
    echo "=== fds mentioning registry|kds|iosc|ffs ==="
    for p in /proc/[0-9]*; do
        ls -l "$p/fd" 2>/dev/null | grep -aiE 'registry|kds|iosc|ffs' | while read l; do
            echo "pid=${p#/proc/} $l"
        done
    done
} > "$OUT/01_procs_fds.txt" 2>&1

# ---------------------------------------------------------------------------
# 2. SysV shared memory: the decrypted registry + KDS key table live here.
#    Dump every segment <= 32 MiB (skip obvious audio/media buffers).
# ---------------------------------------------------------------------------
cat /proc/sysvipc/shm > "$OUT/02_sysvipc_shm.txt" 2>/dev/null
ipcs -m > "$OUT/02b_ipcs_m.txt" 2>&1
TOTAL=0
n=0
# columns: key pid cuid shm_cpid nattch shmseg address cbytes crn
while read s_key s_pid s_cuid s_cpid s_nattch shmseg s_addr size s_crn; do
    case "$size" in ''|*[!0-9]*) continue ;; esac
    [ "$n" -ge 40 ] && break
    case "$shmseg" in ''|0x*|*[!0-9]*) continue ;; esac
    if [ "$size" -le 33554432 ]; then
        timeout 20 dd if=/dev/mem of="$OUT/shm/sysv_${shmseg}_${size}.bin" \
            bs=4096 skip=$((shmseg/4096)) count=$((size/4096+1)) 2>/dev/null
        sz=$(ls -l "$OUT/shm/sysv_${shmseg}_${size}.bin" 2>/dev/null)
        case "$sz" in *0*) : ;; esac
        TOTAL=$((TOTAL+size)); n=$((n+1))
    fi
done < /proc/sysvipc/shm
echo "sysv segments dumped: $n (bytes=$TOTAL)" > "$OUT/shm/NOTE_sysv.txt"

# POSIX shm
cp -R /dev/shm/. "$OUT/shm/devshm/" 2>/dev/null
ls -lR /dev/shm > "$OUT/shm/NOTE_devshm.txt" 2>&1

# sanity: how many dumps contain registry keys
{
    echo "=== segments containing LOCAL_MACHINE / PROCESS/ strings ==="
    for f in "$OUT"/shm/*.bin "$OUT"/shm/devshm/*; do
        [ -f "$f" ] || continue
        c=$(grep -ac LOCAL_MACHINE "$f" 2>/dev/null)
        r=$(grep -ac 'BLAUPUNKT' "$f" 2>/dev/null)
        [ "$c" != "0" ] && echo "HIT $f LOCAL_MACHINE=$c BLAUPUNKT=$r"
    done
} > "$OUT/03_registry_hits.txt" 2>&1

# ---------------------------------------------------------------------------
# 3. on-disk registry sources
# ---------------------------------------------------------------------------
mkdir -p "$OUT/ffs2" 2>/dev/null
for d in /dev/ffs2/registry /dev/ffs/registry /var/opt/bosch/region_static; do
    [ -d "$d" ] || continue
    b=${d##*/}
    cp -R "$d" "$OUT/ffs2/${b}/" 2>/dev/null
done
ls -lR "$OUT/ffs2" > "$OUT/04_ffs2_list.txt" 2>&1
cp /var/opt/bosch/static/system/static.md5 "$OUT/ffs2/" 2>/dev/null
cp /var/opt/bosch/dynamic/system/dynamic.md5 "$OUT/ffs2/" 2>/dev/null
cp /custver.reg /var/opt/bosch/dynamic/custver.reg "$OUT/ffs2/" 2>/dev/null

# ---------------------------------------------------------------------------
# 4. KDS raw store: mtd partitions (flashblock_kds lives in NOR) + device nodes
# ---------------------------------------------------------------------------
cat /proc/mtd > "$OUT/05_mtd.txt" 2>/dev/null
ls -la /dev > "$OUT/05b_devnodes.txt" 2>&1
# dump any mtd whose name suggests kds/ffd/registry/ffs/config (skip huge/rootfs)
grep -i 'kds\|ffd\|ffs\|registry\|config\|pers' /proc/mtd 2>/dev/null | while read line; do
    dev=$(echo "$line" | cut -d: -f1)
    [ -c "/dev/$dev" ] || continue
    timeout 60 dd if="/dev/$dev" of="$OUT/misc/mtd_${dev}.bin" bs=4096 2>/dev/null
done
ls -l "$OUT/misc" > "$OUT/05c_mtd_dumped.txt" 2>&1

# ---------------------------------------------------------------------------
# 5. binary inventory (md5) + the binaries we actually want to diff vs D605
# ---------------------------------------------------------------------------
{
    cd /opt/bosch/processes 2>/dev/null && md5sum ./* 2>/dev/null
    cd /var/opt/bosch/dynamic/processes 2>/dev/null && md5sum ./* 2>/dev/null
    cd /opt/bosch 2>/dev/null && md5sum ./start_createVIN.sh 2>/dev/null
} > "$OUT/06_md5_optbosch.txt" 2>&1
mkdir -p "$OUT/bins" 2>/dev/null
for f in libosal_linux_so.so libiosclib_so.so PROCNAV.OUT DAPIAPP.OUT procmapengine.out procbaselx_out.out procmwlx_out.out; do
    cp "/opt/bosch/processes/$f" "$OUT/bins/" 2>/dev/null
done
# dynamic overlay versions if present (media-provided DAPIAPP/PROCNAV overrides)
for f in DAPIAPP.OUT PROCNAV.OUT; do
    [ -f "/var/opt/bosch/dynamic/processes/$f" ] && cp "/var/opt/bosch/dynamic/processes/$f" "$OUT/bins/dyn_$f" 2>/dev/null
done
ls -l "$OUT/bins" > "$OUT/06b_bins.txt" 2>&1

# ---------------------------------------------------------------------------
# 6. per-process maps for the registry clients (segment addresses + lib set)
# ---------------------------------------------------------------------------
for name in procbaselx PROCNAV PROCMWLX DAPIAPP; do
    pid=$(findpid "$name") || continue
    { echo "=== $name pid=$pid maps ==="; cat "/proc/$pid/maps" 2>/dev/null; } \
        > "$OUT/07_${name}_maps.txt" 2>&1
done

ls -laR "$OUT" > "$OUT/99_collected.txt" 2>&1
sync; sync
reboot