#!/bin/sh
# regdump - one-shot registry/KDS extractor for the Nissan Bosch head unit.
#
# Runs via the proven USB `logger` exploit: the stick's ext2 label is
# "../../usr/bin/" so the stick mounts over /usr/bin and the system's `logger`
# call executes this script as root (same mechanism as nissan_connect3_map_conv/
# diag/navdiag_logger.sh). Output lands in /usr/bin/regdump = ./regdump on the
# stick. Reboots the HU at ALL_DONE = clean "pull the stick" signal.
#
# USAGE: insert the stick after the navigation map is on screen. The script
# never waits for anything and every section ends with sync, so an early pull
# keeps everything collected so far (progress markers in regdump/.done).
#
# v3 - rewritten after the F16A run (dump_01). Lessons baked in:
#   * THIS UNIT FAMILY HAS NO busybox `timeout`, `ipcs`, `md5sum` and its
#     busybox `cp` does not do `-R dir/.` -> all removed.
#   * NO mtd, NO /dev/ffs2: the unit is LCN2EVO (2.6.34.13, eMMC). The
#     registry/KDS live on the TENGINE side; Linux sees them as /dev/iosc
#     shared-memory windows mapped at physical 0xb4000000+ (RAM-backed).
#     THE MAIN HARVEST = those physical windows dumped via /dev/mem, plus
#     /dev/shm/OSAL-arena, plus rfs2 (`/dev/media/rfs`) and region_static
#     .reg/.ERG file harvest.
#   * Only RAM-backed /dev/mem reads (iosc p-window inside mem=768M + attached
#     SysV shm with nattch>=1), so no oops on this panic_on_oops kernel.
#
# ORDER = decreasing importance / increasing size, `sync` after every section.

OUT=/usr/bin/regdump

mount -o remount,rw /           2>/dev/null
mount -o remount,rw /usr/bin/   2>/dev/null

# re-entrancy guard: `logger` may be invoked repeatedly
[ -e "$OUT/.done" ] && exit 0
mkdir -p "$OUT/shm" "$OUT/iosc" "$OUT/regs" "$OUT/misc" 2>/dev/null
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
# does a command exist at all (no `command -v` reliance on ancient ash)
have() { [ -n "$(which "$1" 2>/dev/null)" ]; }

# ===========================================================================
# SECTION 1 (tiny): manifest + process list. No fancy tools, no timeouts.
# ===========================================================================
{
    echo "=== uname ==="; uname -a
    echo "=== cmdline ==="; cat /proc/cmdline
    echo "=== versions ==="
    cat /lcn2kai_version.txt 2>/dev/null; echo
    cat /rfs_version.txt     2>/dev/null; echo
    cat /cc_label.txt        2>/dev/null; echo
    echo "=== busybox applets ==="
    [ -x /bin/busybox ] && /bin/busybox --list 2>/dev/null | tr '\n' ' '; echo
    echo "=== date ==="; date
} > "$OUT/00_manifest.txt" 2>&1

echo "=== procs (comm) ===" >  "$OUT/00b_procs.txt"
for p in /proc/[0-9]*; do
    c=$(cat "$p/comm" 2>/dev/null) || continue
    echo "pid=${p#/proc/} comm=$c" >> "$OUT/00b_procs.txt"
done
echo "SECTION1_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 2 (the prize #1): /dev/iosc Tengine-shared physical windows.
# procbaselx/procmwlx map them rw-s at file-offset == physical addr; those
# pages hold the IOSC registry tree + KDS table maintained by Tengine.
# Union of every rw-s /dev/iosc VMA across ALL processes, rounded to MiB,
# dumped from /dev/mem. 2.6.34 always maps pfn->VIRT_IO, reads are safe.
# ===========================================================================
LO=999999999; HI=0
for m in /proc/[0-9]*/maps; do
    grep " /dev/iosc$" "$m" 2>/dev/null
done | while read s_rng s_perm s_off s_dev s_ino s_path; do
    [ "$s_perm" = "rw-s" ] || continue
    echo "$((0x$s_off))"
done | sort -n | while read o; do
    [ "$o" = "$prev" ] && continue
    prev=$o
    echo "$o"
done > "$OUT/iosc/_offsets.txt" 2>/dev/null
# also dump one file per distinct window (named by physical offset) so the
# host can triage them individually
NW=0
while read off; do
    case "$off" in ''|*[!0-9]*) continue ;; esac
    [ "$off" -lt 1048576 ] && continue          # below 1MiB paddr: nonsense
    [ "$off" -gt 805306368 ] && continue        # above mem=768M: not RAM
    hexoff=$(printf '%08x' "$off")
    sz=$(for m in /proc/[0-9]*/maps; do
            grep " /dev/iosc$" "$m" 2>/dev/null
         done | while read w_rng w_perm w_off rest; do
            [ "$w_perm" = "rw-s" ] || continue
            [ "$((0x$w_off))" -eq "$off" ] || continue
            a=${w_rng%-*}; b=${w_rng#*-}
            echo $(( 0x$b - 0x$a ))
         done | sort -rn | head -1)
    case "$sz" in ''|*[!0-9]*) sz=65536 ;; esac
    [ "$sz" -gt 8388608 ] && sz=8388608
    echo "win p_$hexoff sz=$sz" >> "$OUT/iosc/_windows.txt"
    dd if=/dev/mem of="$OUT/iosc/p_${hexoff}_${sz}.bin" bs=4096 \
       skip=$((off/4096)) count=$((sz/4096+1)) 2>/dev/null
    NW=$((NW+1)); sync
done < "$OUT/iosc/_offsets.txt" 2>/dev/null
{ echo "iosc windows dumped: $NW"
  cat "$OUT/iosc/_windows.txt" 2>/dev/null
  ls -l "$OUT/iosc"
} > "$OUT/01_iosc_list.txt" 2>&1
echo "SECTION2_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 3 (prize #2): POSIX shm - OSAL-arena holds the local IOSC/registry
# cache. busybox-safe per-file copy (cp -R dir/. silently copies nothing).
# Also attached SysV segments (rare on this family, kept anyway).
# ===========================================================================
mkdir -p "$OUT/shm/devshm" 2>/dev/null
find /dev/shm -type f 2>/dev/null > "$OUT/shm/_files.txt"
while read f; do
    nf=${f##*/}
    cp "$f" "$OUT/shm/devshm/$nf" 2>/dev/null
    sync
done < "$OUT/shm/_files.txt"
ls -l /dev/shm > "$OUT/shm/NOTE_devshm.txt" 2>&1

cat /proc/sysvipc/shm > "$OUT/shm/NOTE_sysv.txt" 2>/dev/null
# NOTE: on 2.6.34 the shm listing has NO physical-address column (key shmid
# perms size cpid lpid nattch uid gid cuid cgid atime dtime ctime), so SysV
# segments cannot be mapped to /dev/mem from here. On this family it does not
# matter (single tiny segments; registry lives in iosc windows / OSAL-arena).

# on-unit sanity: which files smell like the decrypted registry
{
    echo "=== files containing LOCAL_MACHINE ==="
    for f in "$OUT"/shm/devshm/* "$OUT"/shm/*.bin "$OUT"/iosc/*.bin; do
        [ -f "$f" ] || continue
        if grep -aq LOCAL_MACHINE "$f" 2>/dev/null; then
            n=$(grep -ac LOCAL_MACHINE "$f" 2>/dev/null)
            echo "HIT $f LOCAL_MACHINE=$n"
        fi
    done
} > "$OUT/02_registry_hits.txt" 2>&1
echo "SECTION3_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 4 (prize #3): on-disk registry sources. On this family: rfs2
# (/dev/media/rfs = "registry filesystem"), region_static, dynamic overlays.
# Directory TREES are listed first (tiny, tell us everything), then actual
# *.reg / *.ERG / *.erg files are copied preserving mangled names.
# ===========================================================================
{ echo "=== find .reg/.ERG (whole fs, bounded by prune) ==="
  find /var /opt /etc /usr/share /dev/media /dev/ffs* -xdev \
       \( -name "*.reg" -o -name "*.ERG" -o -name "*.erg" \) 2>/dev/null
} > "$OUT/03_regfiles_found.txt" 2>&1

ls -lR /dev/media/rfs > "$OUT/03b_rfs_tree.txt" 2>/dev/null
ls -l /var/opt/bosch  > "$OUT/03c_varopt_top.txt" 2>&1

while read f; do
    case "$f" in /*.reg|/*.ERG|/*.erg) ;; *) continue ;; esac
    nf=$(echo "${f#/}" | tr '/' '%')
    cp "$f" "$OUT/regs/$nf" 2>/dev/null
    sync
done < "$OUT/03_regfiles_found.txt"
ls -l "$OUT/regs" > "$OUT/03d_regs_copied.txt" 2>&1
echo "SECTION4_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 5 (medium): full process maps of every bosch/nav process (who maps
# what: registry arenas, iosc windows, kds devices), fds, /dev inventory.
# ===========================================================================
for p in /proc/[0-9]*; do
    pid=${p#/proc/}
    c=$(cat "$p/comm" 2>/dev/null) || continue
    case "$c" in
        *baselx*|*mwlx*|*hmi*|*mapengine*|*DAPI*|*dapi*|*PROCNAV*|*nav*|\
        *voice*|*sds*|*sysprog*|*tengine*|*iosc*|*kds*)
            { echo "=== $c pid=$pid maps ==="; cat "$p/maps" 2>/dev/null
              echo "=== $c pid=$pid fds ==="; ls -l "$p/fd" 2>/dev/null
            } > "$OUT/misc/maps_${pid}_${c}.txt" 2>&1
            ;;
    esac
done
ls -la /dev > "$OUT/misc/04_devnodes.txt" 2>&1
ls -lR /dev  > "$OUT/misc/04b_dev_tree.txt" 2>/dev/null
echo "SECTION5_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 6 (medium): installed file manifests (no md5sum on this unit; the
# unit's OWN static.md5/dynamic.md5 already are the inventory) + region_static
# top tree (it is 2GB - we take listings + only registry-ish files, done in
# section 4, NOT the whole partition).
# ===========================================================================
cp /var/opt/bosch/static/system/static.md5   "$OUT/misc/" 2>/dev/null
cp /var/opt/bosch/dynamic/system/dynamic.md5 "$OUT/misc/" 2>/dev/null
ls -lR /var/opt/bosch/region_static > "$OUT/misc/05_region_static_tree.txt" 2>/dev/null
ls -lR /var/opt/bosch/dynamic/processes > "$OUT/misc/05b_dynamic_procs.txt" 2>/dev/null
ls -lR /opt/bosch/processes > "$OUT/misc/05c_opt_procs.txt" 2>&1
echo "SECTION6_DONE $(date)" >> "$OUT/.done"; sync

# ===========================================================================
# SECTION 7 (largest, LEAST important - safe to never reach): binary copies.
# DAPIAPP.OUT deliberately NOT copied (we read the SD ourselves).
# ===========================================================================
have md5sum && {
    cd /opt/bosch/processes 2>/dev/null && md5sum ./* 2>/dev/null
    cd /var/opt/bosch/dynamic/processes 2>/dev/null && md5sum ./* 2>/dev/null
} > "$OUT/misc/06_md5.txt"
[ -s "$OUT/misc/06_md5.txt" ] || ls -l /opt/bosch/processes > "$OUT/misc/06_md5.txt" 2>&1
mkdir -p "$OUT/bins" 2>/dev/null
for f in libosal_linux_so.so libiosclib_so.so PROCNAV.OUT procmapengine.out \
         procbaselx_out.out procmwlx_out.out prochmi_out.out; do
    [ -f "/opt/bosch/processes/$f" ] || continue
    cp "/opt/bosch/processes/$f" "$OUT/bins/$f" 2>/dev/null
    sync
done
[ -f /var/opt/bosch/dynamic/processes/PROCNAV.OUT ] && \
    cp /var/opt/bosch/dynamic/processes/PROCNAV.OUT "$OUT/bins/dyn_PROCNAV.OUT" 2>/dev/null
ls -l "$OUT/bins" > "$OUT/misc/06b_bins.txt" 2>&1

dmesg > "$OUT/misc/07_dmesg_end.txt" 2>&1
df /usr/bin    > "$OUT/98_df_end.txt" 2>&1
ls -laR "$OUT" > "$OUT/99_collected.txt" 2>&1
echo "ALL_DONE $(date)" >> "$OUT/.done"
sync; sync

# ---------------------------------------------------------------------------
# REBOOT GUARD (safety-by-construction, learned the hard way during local
# testing): only reboot machines that are provably the head unit. Refuse when
# REGDUMP_NO_REBOOT is set, when the kernel is not the HU's 2.6.34, or when
# the root is the ext4 eMMC/rootfs combo the HU does NOT have as `rootfs`...
# simplest robust test: kernel release must start with 2.6.34 .
# ---------------------------------------------------------------------------
case "$(uname -r)" in
    2.6.34*) DO_REBOOT=1 ;;
    *)       DO_REBOOT=0 ;;
esac
[ -n "$REGDUMP_NO_REBOOT" ] && DO_REBOOT=0
if [ "$DO_REBOOT" = 1 ]; then
    reboot
else
    echo "reboot SKIPPED (not an HU kernel $(uname -r) or REGDUMP_NO_REBOOT set)" >> "$OUT/.done"
fi
