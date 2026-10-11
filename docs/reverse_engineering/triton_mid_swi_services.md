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
| `0x03` | `0xc048` | `FUN_0002e49c` | set current-task record `[+0x60] = block[0]` |
| `0x0a` | `0xbc8c` | `FUN_0002c3e4` | status query (`FUN_00041b7c`) |
| `0x16` | `0xbcc8` | `FUN_0002df74` | get current task/service id |
| `0x17` | `0xbf64` | `FUN_00044e20(id, out*)` | registry App-id→descriptor lookup |
| **`0x1b`** | `0xbf08` | **`FUN_00036734`** | **`OSAL_s32SemaphoreCreate`** (`osal_core/osalsmphr.cpp:0x2e0`) |
| `0x20` | `0xbf2c` | `FUN_00029500` | release/put current obj (trace class 6) |
| `0x21` | `0xbf34` | `FUN_00029414(a,b)` | post/acquire current obj w/ param (class 5) |
| `0x43` | `0xbc20` | `FUN_0002cf88` | get/clear a status flag |

**Prime blocker = cmd `0x1b` SemaphoreCreate** (PROCNAV's single most-frequent 0x64
op, 53×, seen next to `wdb_safecnt_*`): it allocates a semaphore from the free
list (`DAT_00053310` list head, `DAT_0004c114`=created / `DAT_0004c118`=max
counters), fills the slot `{type@+4, state@+10, count@+8, max@+0xc, name@+0x15}`,
and **writes the new handle into `*param_2` (= request block field `@4`)**,
returning 0. The emulator's catch-all returns r0=0 **without writing that output
handle**, so PROCNAV's create "succeeds" with a null semaphore; its first wait on
it parks forever on object 0x64 - which is exactly why it never reaches the main
loop that registers `posfi` / subscribes the combined-sensor and publishes
NavPosition. Narrow first implementation target: back a semaphore object + fill
`block[1]` with its handle in `complete()` (mirrors `SERVICE_CREATE_POOL`), then
implement Wait/Post (`0x20`/`0x21`) and the rest per-cmd.
