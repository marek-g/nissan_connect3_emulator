# Triton_mid monitor SWI service table — recovered

Harness: `/tmp/opencode/triton_probe/probe_table.py` (run: `COUNT=30000000 /tmp/opencode/uli-venv/bin/python probe_table.py`)
Log: `/tmp/opencode/triton_probe/run10.log`. Image: `triton.bin` (triton_mid.bin minus 64B header, base 0x80000000).

## Dispatch path
SWI handler @ **0x800eaa04** (ARM vector 0x80000008 → pool → here). On entry the trampoline
pushed `{r3,r12,lr}` then `{r3,spsr}` → `[sp]=r3, [sp,#4]=SPSR, [sp,#0x10]=LR(return)`.
It range-checks **id 4..0x1b**, then at 0x800eaaa8:
`ldr lr,[0x80531ff0] ; ldr lr,[lr] ; ldr lr,[lr,id<<2] ; cmp lr,#0 ; bxne lr`.
Out-of-range/NULL → fallback 0x800eaaf4 (returns -26). Return trampoline 0x800eaad0:
`ldmia sp!,{r3} ; ldmia sp!,{r12} ; msr spsr_cxsf,r12 ; ldmia sp!,{r3,r12,pc}^`.
Service id = svc immediate (`svc #id`, 24-bit).

## Table
- Pointer variable: **0x80531ff0**, the only write is at pc **0x80008a08** (store of
  FUN_8001fcbc's return = DAT_8001fd58, .data image value).
- **Table base = 0x8052f678** (static .data constant, confirmed at runtime: `[0x80531ff0]=0x8052f678`).
- Generic registrar: **FUN_8002077c(id r0, handler r1)** = `ldr r2,[0x80531ff0]; ldr r2,[r2]; str r1,[r2,r0<<2]` (verified literally).
- Second copy of the pointer-var address exists in .data at 0x80515564 (value 0x80531ff0).

## id → handler
| id | handler | registered by (writer pc) | evidence |
|----|---------|---------------------------|----------|
| 0x00 | 0x800005dc | 0x800094cc | runtime [W] + dump |
| 0x01 | — | never (core0) | all writers enumerated |
| 0x02 | — | never (core0) | " |
| 0x03 | 0x80033514 | FUN_80030ac0 @0x80030ce0 (r0=3, lit@0x80030d30) | static |
| 0x04 | 0x80000640 | 0x800094d4 | runtime [W] + dump |
| 0x05 | 0x8000bf74 | FUN_8000af0c @0x8000af18 | runtime [W] + dump |
| 0x06 | 0x80037184 | FUN_8002b848 @0x8002b868 (lit 0x8002ba3c) | static |
| 0x07 | 0x800370d8 | FUN_8002b848 @0x8002b874 (lit 0x8002ba40) | static |
| 0x08 | 0x80036c78 | FUN_8002b848 @0x8002b880 (lit 0x8002ba44) | static |
| 0x09 | 0x800373a8 | FUN_8002b848 @0x8002b88c (lit 0x8002ba48) | static |
| 0x0a | 0x8000bfc8 | FUN_8000af3c @0x8000af48 | runtime [W] + dump |
| 0x0b–0x1b | — | never in this image | see below |
| 0x1c,0x1d | — | never | |
| 0x1e | 0x800203e8 | FUN_8002b848 @0x8002b898 (lit 0x8002ba4c) | static |
| 0x1f–0x27 | (beyond handler's 0x1b limit) | — | |

Completeness argument: every load of literal 0x80531ff0 was inspected
(0x80008b8c store-of-base; 0x8000af38/0x8000af54 id5/id10 setters; 0x8002077c registrar —
xrefs only from FUN_8002b848 (ids 6,7,8,9,0x1e) and FUN_80030ac0 (id3); 0x80020c60 is a
getter; 0x80036278 is unrelated data; 0x800eab14 = the dispatcher itself; 0x80515564 = data).
**ids 0xb–0x1b are never registered by core-0 code** — they exist for the second core /
other component images (range-check bound 0x1b is shared-kernel ABI); nothing in
triton_mid fills them. FUN_8002b848 itself runs only from FUN_800a2ca4 (late init, after
the message pump unblocks — see blockers), which is why ids 6–9/0x1e were 0 in all runs.

## id-6 handler 0x80037184 contract (thread/context yield)
- Reads SPSR at [sp,#4]; computes cpsr_c = (spsr&0x80)|0x13 (keeps I bit of caller, IRQ mode).
- `mrc p15,0,r12,c13,c0,3` → TIB (=0x80000000|threadslot); saves [TIB+4] (TCB ptr) to stack,
  writes ([TIB+4]<<16) back to TIB+4 (thread-id tagged form).
- `msr cpsr_c`; validates r1 against limit [0x80037504] (0xffff806e-class); on fail r0 =
  0xffe50000 (-27<<16) or 0xfff60000 (-10<<16 EFAULT family) via slot *[0x80511ab4]=0x8003729c.
- Worker (*[0x80511ab0] = 0x800371b4) stores r1 into [TCB+0x20] (yield/timeout value),
  switches stack to [TCB+0xe0] context frame when set.
- Tail `ldr r12,[0x800374c0]; ldr pc,[r12]` (reschedule entry). Returns via SPSR trampoline.
Related: id-7 (0x800370d8) builds a full mode-transition context frame on the target stack;
id-8 (0x80036c78) saves all-mode + VFP state into [TCB+0xc8] frame (blocking switch);
id-0x1e (0x800203e8) = lazy VFP enable/restore (vmrs/vmsr fpexc, vldmia s0-s31).

## id-9 handler 0x800373a8 contract
- `mrc p15,0,r12,c13,c0,3` (TIB); cpsr_c = (spsr&0xc0)|0x13.
- Validates ([TIB+4]&3) vs [0x80511734] and r1 flag bit 0x40000000;
  idx = r1>>16; result/index = [r1 + 0x20000 + idx*4] (per-thread array access service).
- Same error-slot/trampoline return conventions as id-6.

## id-0 / id-4 (bonus, system task svc)
- id-4 (0x80000640): switches IRQ mode, takes service-stack from [[0x805118e0]] (entry 0),
  task# = (r1 &~0x80000000)>>16, `ldr ip,[0x80053a54 + task*4]!; mov lr,pc; bx ip` —
  static task table: 0=0x80008c10(nop), 1=0x80009a6c(message pump), 2=0x8000b97c,
  3=0x8000b9f0, 4=0x8000ba4c, 5=0x8000bb54, 6=0x8000b85c, 7..10=abort stubs, 11=0x80009b6c.
- id-0 (0x800005dc): builds a full SVC/SYS context frame on the service stack and calls
  0x80008ca8 (kernel ctx entry) with r1=r12-of-caller.
- id-5/id-10 (0x8000bf74/0x8000bfc8): mode-gated secondary dispatchers: user-mode → r0=-26;
  kernel: idx=(r1&0x7fffffff)>>16, `ldr ip,[0x80055710,idx*0x10]; mov lr,pc; bx ip`.

## Harness notes (unicorn 2.1.4 quirks, all verified)
- ARM `svc #imm` arrives as UC_HOOK_INTR intno=2 with PC = address AFTER svc; LR untouched
  → harness synthesizes LR=pc, SPSR=cpsr(without I), re-dispatches via table → 0x800eaa04.
- Unicorn lacks ARM banked-CP15 TIB: all mcr/mrc/mcrr/mrrc must be NOPed (capstone-driven
  list, 656 insns; regex masks fail — encodings overlap with bl/svc), MRC c13,c0,{3,4} sites
  (56) patched by CODE-hook returning TIB=0x80540000 / TCB=0x80540100.
- `ldmia {pc}^` pops LR normally (no SPSR restore) — OK since harness enters handlers in
  SVC mode with real CPSR; but svc inside a handler (0x800eaa5c) re-dispatches correctly.
- Patch-ins (wiped by init's RAM-clear, re-injected at handler entry): [0x80517004]=1 (skip
  dual-core handoff blob 0xffffe000), [0x80517008]=0x71000000 (device registry),
  [0x805118e0]=0x8053e000→[0x8053f000] (id0/id4 service stack).

## Blockers (why init never registers late ids / never finishes boot)
1. Message pump (svc4 task1, 0x80009a6c → FUN_8000b978/queue [0x800098d0-ptr] → wrappers
   0x800a3314/0x800a3340 = svc#10 family) spins forever: its producer is core1 and/or real
   IPC peripherals (MMIO 0x5ff00000, 0x59c00000 polled/written by the core1 handoff blob).
   Dual-core start was deliberately skipped via [0x80517004]=1.
2. Real boot pre-fills several RAM globals before entry (we stubbed them; others unknown).
3. Banked registers (SP/LR per mode, SPSR restore on `mov pc,^`) partially emulated —
   msr-mode + push/pop works in unicorn2/QEMU, but the id-6/7/8 handlers' TCB-saved context
   switches would need real TCB structures ([TCB+0x20/0xc8/0xe0]) to go anywhere.
4. For the emulator proper: ids 0xb–0x1b can simply be NULL/ENOSYS for this image —
   nothing in core0 calls them; the observable service ABI is ids 0,3,4,5,6,7,8,9,10,0x1e.

## Harness final output (run10, 30M insns, stop = budget)
```
[svc] id=5 at 80110fe8 -> handler=0x8000bf74
[svc] id=10 at 80108354 -> handler=0x8000bfc8
[svc] id=4 at 8010c81c -> handler=0x80000640
[svc4] r0=80a00408 r1=80010000            (task#1 message pump)
[R] [0x80531ff0] = 8052f678
  id 0x00: 800005dc <-0x800094cc   id 0x04: 80000640 <-0x800094d4
  id 0x05: 8000bf74 <-0x8000af18   id 0x0a: 8000bfc8 <-0x8000af48
  (all other ids 0 — late registration phase never reached)
```
## Embedded `sysprog*.uout` overlays (source of the unregistered objects 0x64/0x6e)

`triton_mid.bin` is monolithic: after the kernel image it stores **18 loadable
overlay modules back to back as ordinary little-endian 32-bit ARM `ET_EXEC` ELF
files** (first at file off `0x120ec0` = `0x80120e80`, last `0x4eed80`). The
monitor relocates and runs them at boot, which is why objects 0x64 and 0x6e have
no static registration in the flat disassembly: their handlers are installed by
an overlay's init. Carve them with `extract_sysprog_overlays.py`.

Identified by the `SysProg*.cpp` string each embeds:

| ELF (order) | overlay | size | role |
|---|---|---|---|
| blob0  | **sysprogosal** | 0x52600 | OSAL kernel service: message queues, pools, events, IO control |
| blob4  | sysprogvirtio | 0xcf80  | virtio host transport |
| blob6  | sysprogsensor | 0x755c0 | |
| blob10 | sysprogdnl    | 0x7fd00 | DNL (navigation layer) kernel service |
| blob12 | sysproganalysis | 0x1cb3c | performance analyser / SI (hooks `TMaxSemId`, SVC extended) |
| blob14 | sysprogosalutil (cand.) | 0x12e00 | OSAL diagnostic shell (`MSGPOOL_*`, `OSAL_FS_*`, `EM_TRACE`) |
| ...    | can/flash/rvc/usbf/audio/drv/filesys/spm/od/input | | device overlays |

**`sysprogosal` (blob0) is the OSAL message substrate.** Its strings expose the
message header and queue internals the whole system shares:
- Header fields (`Check Msg:` format): `Sender, Receiver, Size, Blocks, Type,
  s-sub, d-sub, Time, Serv-ID, Func-ID, OpCode`.
- Queue/pool: `FFD_MSGQ`, `FFD_READ_MESSAGE_QUEUE`,
  `OSAL_s32MessagePoolGetMinimalSize`, block allocator states
  `Used Block / Deleted marked block / Unknown queue`.
- Sync: `Event : %d used for OSAL Current/Max`, `IOSC_EVENT_INVALID`,
  semaphore creation (`..._SEM`).

This is the object PROCNAV hits 181x (`0x64`) and the counterpart of
`libosal_linux`. Decoding blob0's 0x64 registration + header layout is the
concrete path to bridging PROCNAV's IPC to the DAPIAPP `mbx_0` requests.

### 0x64 decode status (this session)

- **`sysprogosal.elf` (blob0) is now decodable in Ghidra** — the old "1 function /
  crippled load" note was just *auto-analysis never having been triggered*.
  Creating the `entry` function at `0x0000f0a4` kicked analysis over to **1319
  functions** (all `FUN_*`, no named symbols). Program saved back to the project,
  so it persists for later sessions. The overlay is base-0 here; at runtime it is
  relocated, so runtime handler addresses seen in the emulator must be re-based
  to this ELF before they can be cross-walked.
- **Dispatch key for object 0x64 = `r1 >> 16`** (the "cmd"), matching the object
  `0x0a` bank service which also keys on `id >> 16` (`0x8004829c`). PROCNAV
  drives cmds `0x1b, 0x03, 0x43, 0x0a, 0x16, 0x17, 0x20, 0x21, ...` (scattered up
  to `0x43`) → the handler is a **command switch of ~68 arms**, i.e. the whole
  OSAL message/pool/event/sem/IOCTL surface, not one call. The `cmd` lives in
  `r1>>16`; the `[15:8]` byte is a sub-arg/arg.
- **Dead end recorded**: the object id is *not* findable by grepping immediate
  `0x64`/`100` — every `mov r1,#0x64` in blob0 is `FUN_0002bdf4(buf, 100, fmt, …)`,
  i.e. **100 is the OSAL log channel** (`FUN_0002bdf4` = OSAL printf, `FUN_000284c4`
  / `FUN_0004543c` = emit). Useful corollary: the overlay is densely instrumented,
  so its **log format strings are the map** to each command's semantics (e.g.
  `OSAL_s32IOReadAsync of Task:%d with Invalid Callback`). Use strings, not the
  `100` immediate, to navigate.
- Object 0x64's service routine is installed by the **overlay init** (which the
  emulator never runs, per the "late registration" blockers above), so it is not
  reachable statically from `triton_mid`'s flat disassembly — it must be located
  inside blob0 via its log strings / the kernel-object-create call it makes.

**Scope reality**: faithfully serving 0x64 = decode + reimplement the whole OSAL
command surface in the monitor's `complete()` (as the `0x0a` bank / pool services
already do), *and* afterwards still provide a `cmb_sensorfi` GPS source before
PROCNAV publishes NavPosition. This is a multi-session subsystem, not a single
fix; implement per-cmd as validated slices against the r168 baseline.

## Object 0x64 fully decoded — registration, dispatch ABI, cmd map

### Registration mechanism (`sysprogosal`, blob0 = *tengine_osal*)
- Kernel registrar **`FUN_00027b00(id r0, desc r1)`**; `desc` is a `0x28`-byte
  block `{ +0x00 flags=0x80000000, +0x24 handler_table_ptr, +0x28 entry_count }`.
  Calling `FUN_00027b00(id, 0)` tears the service object down.
- Service objects registered by blob0 init (each its own `FUN_…`):
  | obj | registrar | table ptr | count |
  |---|---|---|---|
  | **`0x64` (100)** | `FUN_000134a0` | **`0x0004c134`** | **99** |
  | `0x6e` | `FUN_00011bfc` | `0x0004eaf4` | 19 |
  | `0x77` | `FUN_00013560` | `0x0004eb40` | 12 |
  (`FUN_00011bbc` / `FUN_00011bfc` are the create/destroy wrappers.)

### Dispatch ABI (svc #6 → object 0x64)
`r1 = (cmd << 16) | (sub << 8) | 0x64`. The kernel finds the object's
`handler_table`, indexes `entry = table[cmd]` (stride 4, cmds `0..count-1`), and
jumps to it. Each `entry` is a thin **shim** called with `r0 = &request_block`;
it unpacks block fields into `r0..r3` and **tail-branches** (`b`, not `bl`) to the
real `OSAL_*` implementation, which returns `0`/error and writes any out-params
back into the block. **Request block layout** (observed across shims):
`{ u32 @0, u16 @4, u32 @8, u16 @0xc, … }`.
OSAL helpers: `FUN_0002df74`=get task id, `FUN_0002c668`=get last error,
`FUN_0002ce9c`/`FUN_0002cde4`/`FUN_0002c1fc`=trace (severity+class+name),
`DAT_00072000/02/03/07/14`=error-code constants (`0x72000`=OK).

### cmd → op (PROCNAV's hot cmds, resolved)
| cmd | shim | op | semantics |
|---|---|---|---|
| `0x02` | - | get last error | per-thread monitor error code (`0x72011` after the refused SCD probe); PROCNAV's get-last-error wrapper chain lands here (`0x20064`) |
| `0x03` | `0xc048` | `FUN_0002e49c` | set current-task record `[+0x60] = block[0]` |
| `0x0a` | `0xbc8c` | `FUN_0002c3e4` | status query (`FUN_00041b7c`) |
| `0x16` | `0xbcc8` | `FUN_0002df74` | get current task/service id |
| `0x17` | `0xbf64` | `FUN_00044e20(id, out*)` | registry App-id→descriptor lookup |
| **`0x1b`** | `0xbf08` | **`FUN_00036734`** | **`OSAL_s32SemaphoreCreate`** (`osal_core/osalsmphr.cpp:0x2e0`); handle written to `block[1]`, r0 = status |
| `0x1c` | `0x2ec06c` | semaphore open | block `[0]=name, [4]=optional handle slot`; **the handle is returned in r0** (`0` = not found). Names the unit found pre-provisioned (`AIL_TRACE_SEMAPHORE`) are lazily created by the emulator. |
| `0x20` | `0xbf2c` | `FUN_00029500` | release/put current obj (trace class 6) |
| `0x21` | `0xbf34` | `FUN_00029414(a,b)` | post/acquire current obj w/ param (class 5) |
| `0x31` | `0x58b0` | event create | named event, handle written to `block[1]` |
| `0x33`/`0x34` | - | event delete/close | status only |
| `0x35` | `0x2ec3d4` | event wait | block `[0]=handle, [0xc]=timeout` (`0xffffffff` = forever); **must park** - see the NAVEND note below |
| `0x43` | `0xbc20` | `FUN_0002cf88` | get/clear a status flag |

The former prime blocker, cmd `0x1b` SemaphoreCreate, is modelled (guest control
block + shared name→handle map + `block[1]` output), as are Wait/Post (`0x20`/
`0x21`).

## Object 0x6e fully decoded — the OSAL IO (file) services

PROCNAV's registry client (`reg_tclRegKey`) and SCD service map (`scd`) speak to
`/dev/registry/...` through the OSAL IO wrappers, whose service literals sit as a
table right behind the wrappers (PROCNAV `0x32b7f4..0x32b818`, all little-endian
words `0x00cc116e` = `(cmd << 16) | (sub << 8) | 0x6e`):

| literal | wrapper | request block | status |
|---|---|---|---|
| `0x1026e` | `OSAL_IOOpen(path r0, u16 flags r1)` `0x32b66c` | `[0]=path ptr, u16 @4` | handle in r0, `-1` = failure |
| `0x2016e` | `OSAL_s32IOClose(handle r0)` `0x32b694` | `[0]=handle` | r0 |
| `0x3026e` | `OSAL_IOCreate(path, u16 flags)` `0x32b6b0` | like open | handle |
| `0x4016e` | `OSAL_s32IORemove(path)` `0x32b6d8` | `[0]=path ptr` | r0 |
| `0x5036e` | `PRC_OSAL_s32IOControl(handle, request, arg)` `0x32b6f4` | `[0]=handle, [4]=request, [8]=arg` | r0 |
| `0x6036e` | `PRC_OSAL_s32IOWrite(handle, buf, len)` `0x32b718` | `[0]=handle, [4]=buf, [8]=len` | written |
| `0x7036e` | `PRC_OSAL_s32IORead(handle, buf, len)` `0x32b73c` | like write | read |
| `0x8016e`/`0x9016e` | async write/read `0x32b760`/`0x32b77c` | unobserved | - |

`OSAL_s32IOControl` (`0x32cfb4`) validates the handle *client-side* before the
svc: `*[handle]` must equal `0xD0E1D0E1` (literal `0x32d11c`) or it fails locally
with error `0x7200c`; `[handle + 0x1c] == 0x22 && request == 3` is answered
locally too, and *everything else tail-branches into the `PRC_OSAL_s32IOControl`
wrapper* (the branch keeps the caller's LR, so the svc continuation lands back in
the ioctl's caller). The handle therefore has to be a real guest object, not a
table index: the emulator allocates a zero page `{ +0: 0xD0E1D0E1, +0x1c: type,
+0x20: device path }` and reads the path back in iocontrol to route requests, so
handles are self-describing and per-process by construction.

### Registry ioctl protocol (from the client call sites)

- request `1` (`bQueryU32` `0x273d2c`, `bQueryString` `0x273dac`): buffer
  `[0]=value-name ptr, [4]=size, [8]=out indicator, [0xc]=out data ptr` (the
  server *dereferences* `[0xc]`). The two clients disagree on the indicator:
  `bQueryU32` requires `1` after success, `bQueryString` requires `0` - the
  emulator writes `1` for DWORD values and `0` for strings, which satisfies
  both. A missing value answers status 0 with indicator 0 (client default
  branch, no error code).
- request `3` (`bSetU32` `0x273cbc`, `bSetString`): buffer `[0]=name, [4]=size,
  [0xc]=pointer to the data to store`.
- request `0xa`/`0xb` probes (`scd_bTestLookupIoControl` `0x34e340`): buffer
  `+0x100` holds a scratch u32; the probe compares the OSAL last-error (`0x20064`
  = the get-last-error cmd, per-thread map in `OsalState`) against constants at
  `0x34e718`/`0x34e71c` (`0x72011`/`0x72015`). The emulator refuses both probes
  with `-1` *and* last-error `0x72011`, which classifies the platform as "can NOT
  handle SCD Lookup" and pins PROCNAV to SCD mode 0, the static registry-walk.
  Mode 1 (an SCD-hosted IOSC queue server) would demand a full SPM emulation and
  was judged disproportionate. Refusing with `-1` but a stale last-error still
  selects mode 1, so the two must move together.
- request `0x3eb` subkey enumeration (`reg_tclRegKey` iterator `0x2c4738`, used
  by the mode-0 SCD walk to iterate `PROCESS` -> apps -> `SERVICES`): the ioctl
  argument *is* the caller's 0x108-byte request object (`+0` io handle, `+4` zero
  flag, `+8..` a 0x100-byte name buffer) and the client hands out `object + 8` as
  the name pointer (`0x2c475a`), so the answer is written into that inline buffer.
  The call returns the subkey *following* the name currently stored there (empty
  or stale buffer = restart at the first child), `-1` when it is the last one;
  status `0` keeps the loop going. The walk is stateless on the emulator side -
  keyed purely on the name the caller hands over - which is what the client's
  re-open-per-step iterator (`0x653e8` closes and re-opens the request, moving to
  a fresh io handle each child) requires.
- request `0xb` service→app lookup (`scd_bGetAppIdFromServiceId` `0x34f012` at
  `0x34f084`): the service id is a u32 at `arg + 0x100`; the app id is read back
  from the *same* word (`strh r0,[r5]` of `(ushort)`). The emulator resolves it
  the way the registry publishes it: any key with `SERVICEID == id` under
  `...\PROCESS\<p>\<app>\SERVICES\<svc>`, then `APPID` two levels up - service
  `0xb` (SENSORS) -> `PROCESS\PROCMW\SENSOR` -> app id `10041`.

Evidence: `run_r174/r175` - PROCNAV reaches SCD mode 1, `MAXPOOLSIZE` queries
return `0xFA000` from `base.reg`, the probe resolves service `0x100` to
`PROCESS\BASE\SPM` (APPID 0), and unreadable/unknown paths fail the open exactly
like a device without that key (which also retires the stale-path
`/dev/registry/P/` retries the client makes after failed opens on the real unit
would fail the same way). `/dev/trace` opens get handles too; its ioctl `0x3`
(class-mask setup) and writes are acknowledged/consumed. The `PROCESS\NAVAPP`
keys live in the encrypted region files (`ni2k_*/NAVAPP.ERG`), which are not
decrypted in the emulator's registry; the required shapes are fabricated in
`data/registry_compat.reg` instead - see the FC bring-up section below.

## PROCNAV FC bring-up reaches its main loop (r191-r204)

With mode 0 pinned and the registry walk served, the whole `vStartApp` chain now
runs. The gates, in the order the code demands them (`0x34f14e` gate, then each
component's `bInitInstance`):

1. `scd_bGetAppPath(appid)` non-NULL *and* `/dev/registry/.../PROCESS/<p>/<app>`
   openable *and* an `APPID` u32 hit - else the component prints "startup
   suppressed" (all-suppressed is what fired the old Shutdown-Event). AppIDs are
   compiled into the init gates: NAVAPP/FC_NAVI=1, APP03=3, FC_TEA=19, FC_STAT=22,
   WDB=23, FC_LBS=284.
2. `scd_bCreateQueue` (an OSAL message-queue create - already modelled).
3. `scd_bGetAppInterfaceConfiguration` (`0x34f272` family): opens `<app>\AIF_CONFIG`
   and queries `MSG_COUNT_INTERMEDIATE_MBX` (+ watchdog); a failed open fails
   `bInitInstance` ("Msg %d Wdt %d" prints the uninitialised outs). `80`/`0` works.
4. `scd_s32GetThreadConfiguration` (`0x34f2d0`): opens `<app>\APP_THREAD`, queries
   `<name>_PRIO`/`<name>_STCK`. NAVI asks `AE` then `AB`; the other components ask
   `AE` only (TIMA/TIMA `AB`). Names are passed by the callers (no static name
   table), so they were recovered empirically from the trace misses; unknown thread
   names log "failed to find key <name>_PRIO".
5. `scd_OpenQueue success for AppId = 0x..` then the worker threads: OSAL cmd
   `0x16`-family create/spawn with entries `0x248af5` (AE, a vtable thunk) and
   `0x24de67` (AB) - **odd entries**, which is what exposed the scheduler bug that
   `uc_emu_start(begin)` forces ARM on an even `begin` regardless of the saved
   CPSR. Fixed by OR-ing the restored T bit into `begin`.
6. TIMA/APP03 reads its whole `APP_CONFIG/*` tree through
   `ccatima_registry.cpp(635)` and falls back to coded defaults (`value: X *`) for
   every miss - no registry work needed there.
7. NAVEntry (`navapp_exeenv.cpp`): opens `FC_LBS\APP_CONFIG`, queries
   `ON_TENGINE` - a *miss* aborts ProcNav with the Shutdown-Event ("No LBS-
   'ON_TENGINE'-Value found"); `0` selects "Don't start LBS on tengine" and
   defers FC_LBS, `1` would start it on the engine-on message.
8. The tail of `vStartApp` (`0x53e76`) creates the `NAVEND` event (OSAL `0x31`)
   and waits on it (OSAL `0x35`, timeout word `0xffffffff` at `block + 0xc`) -
   that wait *is* the main loop: SPM only signals NAVEND when it tears the
   software block down. Answering it reads as "shutdown requested" (the code then
   unconditionally logs "Shutdown-Event occured --> remove NAVAPP sofwareblock"
   and deinits every component), so cmd `0x35` parks the thread until woken.

Run r204 (90 s): 48 apps registered once each (incl. the six fabricated NAVAPP
apps), NAVI/STAT/TEA/APP03/CTMC/WDB pass `bInitInstance`, `scd_OpenQueue`
succeeds per app, `AE_1`/`AB_1`/`AE_16`/`AE_13`/`AE_3`/`AB_3`/`AE_17` and the
AIO2 pool park on their receives (`0x10240513`), the main thread idles at
`0x350564`, and no Shutdown-Event fires. Remaining PROCNAV thread deaths are the
one-shot helper (`exited(1)`) and the TSE command-loop thread (`exited(139)`,
whose literal-pool "return address" teardown matches the real kernel's illtrap
path, see the `0x60112` note in `yield_thread`).
