# regdump stick — registry/KDS extraction from a live head unit

Collects everything needed to replace the undecryptable region `.ERG` files
(see `docs/reverse_engineering/erg_registry_crypto.md` for *why*):

| what | where in the dump | why |
|---|---|---|
| decrypted registry tree | `shm/sysv_*.bin`, `shm/devshm/*` | the real FC_STAT/FC_TEA/… `APP_CONFIG` + `SERVICES` values |
| KDS key table | the shm segment(s) containing KDS entries | 16 B entry `0x1216`/`0x1217` XOR `03050607080a0b0c0e87618762876f00` = AES-128 key of the ERGs |
| installed registry files | `ffs2/registry/`, `ffs2/region_static/` | authoritative `.reg` (+ `.ERG` copies) for this build |
| KDS raw store | `misc/mtd_*.bin` (NOR partitions named kds/ffd/…) | offline key search |
| binary/md5 inventory | `06_md5_optbosch.txt`, `bins/` | diff the car's build vs our D605 target |

## Car side

```sh
./build_regdump_stick.sh            # -> regdump.ext2
sudo dd if=regdump.ext2 of=/dev/sdX bs=4M   # double-check sdX!
```

Insert the stick **cold**, start the car. The logger waits until procbaselx +
PROCNAV are up (+30 s settle), collects, then **reboots the HU = done, pull the
stick**. Map SD can stay in the unit.

## Host side triage (after mounting the stick)

```sh
cd regdump

# 1. find the decrypted registry segment(s):
for f in shm/*.bin shm/devshm/*; do c=$(grep -ac LOCAL_MACHINE "$f" 2>/dev/null); \
  [ "${c:-0}" != 0 ] && echo "$f: $c hits"; done
# the registry segment is usually a few MB and full of BLAUPUNKT key paths;
# carve the printable tree with:
strings -a <regseg>.bin | grep -a "LOCAL_MACHINE" | sort -u > registry_keys.txt

# 2. KDS key hunt: candidate 16-byte KDS entry -> XOR mask -> AES-ECB test
#    against ni2k_eur/NAVAPP.ERG payload at file offset 12. Any key that makes
#    the first plaintext block printable ("\r\n\r\n;-*-*..." registry text) wins.

# 3. compare builds:
md5sum bins/*   # vs D605 files; only matters if we ever re-target the emulator.
```

If `shm/` contains no `LOCAL_MACHINE` hits, the registry is not in SysV shm on
this build (IOSC-over-Tengine variant): the `07_*_maps.txt` + `02_sysvipc_shm`
+ `01_procs_fds.txt` files still tell us *where* it lives, and the mtd dumps
may contain a persisted copy.