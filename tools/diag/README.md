# regdump stick — registry/KDS extraction from a live head unit

Collects everything needed to replace the undecryptable region `.ERG` files
(see `docs/reverse_engineering/erg_registry_crypto.md` for *why*).

## Post-mortem of run dump_01 (F16A unit) — why v1 came back nearly empty

The unit is **LCN2EVO** (`AI_PRJ_NISSAN_LCN2EVO_15.6V01`, kernel 2.6.34.13,
eMMC rootfs), not lcn2kai as assumed. Concrete failures, all fixed in v3:

* busybox on this build has **no `timeout`, `ipcs`, `md5sum`** — the wrapper
  killed the proc scan, and `timeout dd ...` silently skipped every dump;
* busybox `cp -R dir/.` **copies nothing** (exit != 0, stderr discarded) —
  `/dev/shm` (incl. `OSAL-arena`, the 1.1 MB IOSC/registry cache!) and
  `region_static` were skipped;
* **no `/dev/ffs2`, no `/proc/mtd`** — KAI-era paths that don't exist here;
* the only SysV shm segment had `nattch=0` → the attached-only filter dumped
  nothing. **The registry is NOT in SysV shm on this family.**

What the dump *did* prove (from `04_procbaselx_maps.txt`): the registry + KDS
live on the **Tengine side**; Linux sees them as **`/dev/iosc` rw-s windows
whose file-offset equals the physical address** (`b42xxxxx..b55xxxxx`, inside
`mem=768M` ⇒ RAM-backed ⇒ `/dev/mem`-safe), plus `/dev/shm/OSAL-arena`, and
the installed files live under **rfs2 (`/dev/media/rfs`, "registry filesystem")**
and `region_static` (`/dev/mmcblk0p2`).

## What v3 collects (priority order, `sync` after each section)

| section | dump path | why |
|---|---|---|
| 1 manifest + proc list | `00_manifest.txt` (incl. busybox applet list), `00b_procs.txt` | environment ground truth |
| 2 **iosc physical windows** | `iosc/p_<paddr>_<size>.bin` | THE registry tree + KDS table (Tengine RAM) |
| 3 POSIX shm (`OSAL-arena`!) + attached SysV + on-unit `LOCAL_MACHINE` hit report | `shm/devshm/`, `shm/*.bin`, `02_registry_hits.txt` | local IOSC/registry cache |
| 4 `find` of every `*.reg`/`*.ERG` on all fs + copies + `rfs` tree listing | `03_regfiles_found.txt`, `regs/`, `03b_rfs_tree.txt` | authoritative installed files |
| 5 maps+fds of all Bosch/nav processes, `/dev` tree | `misc/maps_*.txt` | where anything else is mapped |
| 6 static/dynamic.md5, region_static + dynamic trees | `misc/05*.txt` | inventories (unit's own md5 lists) |
| 7 binary copies (no DAPIAPP — we have the SD) | `bins/` | nice-to-have, safe to lose |

## Car side

```sh
./build_regdump_stick.sh            # -> regdump.ext2 (1 GiB)
sudo dd if=regdump.ext2 of=/dev/sdX bs=4M   # double-check sdX!
```

Insert the stick **after the map is visible on screen**. The script never
waits, collects small+important first, and **reboots the HU at ALL_DONE =
pull the stick**. Early pull keeps everything synced so far (progress markers
`SECTION*_DONE`/`ALL_DONE` in `regdump/.done`).

## Host side triage (after mounting the stick)

```sh
cd regdump
cat 02_registry_hits.txt          # which dumps contain LOCAL_MACHINE

# 1. carve the registry key tree out of the winning iosc window(s):
strings -a iosc/p_*.bin | grep -a "LOCAL_MACHINE" | sort -u > registry_keys.txt
# (also try shm/devshm/OSAL-arena — the local cache may already hold it all)

# 2. KDS key hunt: candidate 16-byte KDS entry -> XOR mask 03050607080a0b0c0e87618762876f00
#    -> AES-128-ECB test against ni2k_eur/NAVAPP.ERG payload at file offset 12.
#    Winner makes the first plaintext block printable ("\r\n\r\n;-*-*" registry text).
#    KDS candidates live in the iosc windows (KDS_SHME) and/or regs/*.ERG copies.

# 3. compare builds:
ls -l bins/                       # vs D605 files; only matters if we re-target.
```

If `02_registry_hits.txt` is empty, start from `misc/maps_*.txt` (who maps
`/dev/iosc` at which offsets) and `03b_rfs_tree.txt` — the data is there, the
dump scope just needs another notch.
