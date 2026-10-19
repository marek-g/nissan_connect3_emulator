You are working on emulator of the Bosch lcn2kai car headunit. The goal is to have an usable emulator to run and test navigation system with GUI.

The emulator has access to the firmware: `/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/lx001.tar.gz` and SD card content: `/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/NISSAN Connect LCN3 V7 2022_2023`. Do not ever modify the content located there.

Here you have Linux 2.6.34 source code: `/home/marek/Ext/reverse_engineering/NissanMaps/linux-2.6.32.14`. Every time you are fixing or creating a new syscall, first try to understand how the syscall works exactly in Linux 2.6.32.14. Remember that we are emulating ARM 32-bit architecture, which has its own `arch/arm` folder with files that override many generic values (for example `fcntl.h`).

## Fidelity rule: never patch user-space applications
Avoid touching Linux user-space *applications* (the Bosch process binaries:
`DAPIAPP.OUT`, `procmapengine.out`, `prochmi_out.out`, `PROCNAV.OUT`,
`procbaselx_out.out`, ...). Their code must run as-is: do not patch their
instructions, force their registers/variables, or synthesize messages they are
supposed to emit themselves. Patching *libraries* (`libosal_linux_so.so`,
`libsvg-*.so`, ...), simulating devices/drivers (`/dev/cryptnav`, `/dev/prm`,
...) and faking RTOS behaviour (spawn order, startup messages) is allowed and is
the preferred place to work around missing hardware or unrealistic timing.
Also: never block inside a hook that runs in guest context, and never hold a
host lock across a sleep - the whole emulator is serialized on those, so a
host-side "wait" freezes every process instead of staggering them.

## Fidelity rule: only fabricate what the real unit gets from outside

The emulator exists to debug the OSM->navi data converter, so a bug in the SD
card's address/POI database must surface in the emulator at the same code path
and error code as on the real unit (same binaries, same parsers). Fabricating
protocol answers can mask exactly those bugs, so:

1. Only fabricate answers of partners that are *not* among the emulated
   processes (hardware / TE side: LPM power proxy, GPS provider, CCA
   registration-id directory, encrypted registry values, camera/renfi input).
   Answers that PROCNAV/DAPI would produce on the real unit must be produced
   by PROCNAV/DAPI in the emulator too. If such a process stays silent, do not
   inject its reply - find and fix the missing precondition in its environment
   (RNW CCP connection, POI database state, service registration, ...) so it
   answers on its own.
2. Every fabrication must be visible in the log, and where it could mask a
   data error it must sit behind an environment flag (e.g.
   `EMU_FAKE_DESTENTRY=1`) that is off by default - a diagnostic run goes
   exclusively down the genuine inter-process path.

## Code hooks: narrow ranges can silently never fire (unicorn-engine 2.1.5)

`add_code_hook(begin, end)` with a narrow range (a few bytes) sometimes
registers `Ok` but is **never called**, even though the guest provably
executes that exact instruction. Cause: unicorn 2.x decides hook dispatch at
*translation* time (`accel/tcg/translator.c` `HOOK_EXISTS_BOUNDED` is checked
per instruction only while a translation block is generated). A narrow range
that overlaps only the middle of an already-translated large Thumb TB never
gets its per-instruction check emitted and nothing re-translates it later.
Whether a probe "lives" therefore depends on translation-block history, not on
the guest code - so a zero-hit probe proves nothing until you confirm it is
not this quirk (r372-r376: `0x2ebd6c`/`0x297f54`/`0x2989c4`/`0x30b98a` hooks
were dead while the code ran 2000+ times).

Do not conclude "dead code" from a zero-hit narrow hook. Confirm liveness with
a **page-range** hook (hook the whole 4 KiB page and dispatch on the exact
fetch address inside the callback), which fires reliably. This is exactly what
the `xp-watch` machinery in `src/libs/procnav.rs` does (`XP_PAGES` +
`XP_TARGETS`/`xp_watch`): use it for any probe that must observe a mid-TB
address. Live narrow hooks elsewhere keep working, so only convert probes you
have verified are false-negative. When picking a hook address, take the real
*fetch* address: `.dynsym`/`.symtab` values carry the Thumb bit (`|1`) so mask
it off, and do not trust symbol layout blindly - exported `err_vSetError`
`0x30b733` is a different (unwind-side) function, while every `bl` site calls
`0x30b98a`. Cross-check against Ghidra (its VAs equal the emulator's runtime
VAs for PROCNAV, image base `0x8000`) or against a live page-hook before
believing a miss. Full write-up: `docs/issues/design.md`.

You can keep track of any issues (adding and removing them) with the following files:
- `docs/issues/correctness.md`
- `docs/issues/robustness.md`
- `docs/issues/performance.md`
- `docs/issues/design.md`

Commit to git frequently. Every time you make a meaningful change that you believe moves the emulator forward (a fix, a new hook, a new syscall, a working feature slice), commit it. Small, focused commits make it easy to bisect regressions later. Do not commit work-in-progress that does not build.

## Running/killing the emulator

The emulator process does not reliably terminate on SIGTERM (`timeout` alone often
leaves it stuck holding GPU memory). Always kill it with
`pkill -9 -x nissan_connect3` (the comm name is truncated to 15 chars; `pkill -f`
on the full name can match your own shell and kill the session).
