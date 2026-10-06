# RTOS firmware: `triton_mid.bin` / `triton_dualos`

This document records the current reverse-engineering status of the RTOS side of the
Nissan Connect 3 / Bosch LCN2KAI firmware. The RTOS image is not merely a bootloader:
it contains a Bosch/Triton real-time OS, OSAL-style message queues, IOSC shared-memory
transport, registry paths, DL/core services, and process-control surfaces. The Linux
side (`procbaselx`, `prochmi`, etc.) is not the master controller by itself; it waits
for the RTOS side to provide IPC state and commands.

## Artifact

Original image:

```text
/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605/triton_mid.bin
```

Parsed u-boot legacy uImage header:

| Field | Value |
|---|---:|
| Magic | `0x27051956` |
| Name | `triton_dualos` |
| Load address | `0x80000000` |
| Entry point | `0x80000290` |
| Data size | `5430596` bytes |
| Build timestamp | `2019-05-17T14:15:20Z` |

The uImage header is 64 bytes. The raw payload used in Ghidra is:

```text
/tmp/triton_mid_raw.bin
```

It can be regenerated without modifying the firmware tree:

```sh
dd if=/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605/triton_mid.bin \
   bs=1 skip=64 of=/tmp/triton_mid_raw.bin
```

In Ghidra the raw image is imported as:

```text
/firmware/triton_mid_raw.bin
```

with ARM little-endian 32-bit language and image base `0x80000000`. Because this is a
raw binary, most functions still have Ghidra default names such as `FUN_8013f2b0`.

## High-level view

The firmware has at least two cooperating sides:

- Linux side (`lx001.tar.gz`)
  - runs Linux userspace processes such as `procbaselx_out.out` and `prochmi_out.out`;
  - uses POSIX message queues, `/dev/iosc`, `/dev/shm`, and pthread/futex primitives;
  - `procbaselx` is a base/service process, not an autonomous GUI starter.
- RTOS side (`triton_mid.bin`)
  - runs `triton_dualos`;
  - owns Bosch OSAL-style message queues and IOSC transport;
  - contains process-control and registry surfaces;
  - is the natural source of startup commands that Linux processes wait for.

For the emulator, the current missing piece is not a single syscall. It is the RTOS-side
service that feeds the Linux processes with IOSC/OSAL state and, in the normal boot,
the IPC commands that tell `procbaselx` to start other processes.

## Entry point

Ghidra function:

```text
RTOS_ENTRY @ 0x80000290
```

Observed behaviour:

- writes to CP15 peripheral/system registers;
- reads CP15 identification register 5;
- waits while that value is not `2`;
- if `param_1 == 0`, performs early memory/MMU-like setup and reaches indirect startup
  calls through globals around `DAT_80000514` and `DAT_80000518`;
- if `param_1 != 0`, enters an infinite idle loop.

This looks like low-level core bring-up / secondary-handoff code rather than the whole
RTOS subsystem initializer. The higher-level RTOS task startup tables are not yet fully
resolved.

## OSAL message queues

The RTOS contains OSAL message-queue primitives with recognizable names:

| Address | String / function | Meaning |
|---|---|---|
| `0x8014b450` | `OSAL_s32MessageQueuePost` | post path debug name |
| `0x8015074c` | `OSAL_s32MessageQueueWait` | wait path debug name |
| `0x80155138` | `MQ_EVENT_NOTIFY` | queue notification type |
| `0x80155148` | `MQ_RINGBUFFFER` | queue/ringbuffer type name, typo in firmware |
| `0x8015aa44` | `u32SendToMessageQueue` | queue send path |
| `0x801607e4` | `u32GetFromMessageQueue` | queue receive path |

Key functions:

| Function | Confidence | Observed role |
|---|---|---|
| `FUN_8014a468` | high | open/register a queue-like handle; allocates or attaches a queue object and returns it through an out-param |
| `FUN_8014b088` | high | message post implementation; compares status against `0x72000`; validates a 4-byte handle header and a queue pointer; rejects priority values greater than `7` |
| `FUN_8014ff48` | high | message wait implementation; consumes queue slot, copies payload, updates queue statistics, compares success to `0x72000` |
| `FUN_8015701c` | medium | likely queue create/init helper reached from queue setup paths |
| `FUN_8013f2b0` | high | creates/uses `TE_TERM_MQ` and `LI_TERM_MQ`; posts a small message and enters a wait/dispatch loop |
| `FUN_80153a54` | high | creates `OSAL_CB_HDR_LI_MAIN` and `OSAL_CB_HDR_TE`; maintains global callback-header dispatcher state |
| `FUN_8013ef74` | high | creates or registers a task/object named `IOSC_HDR` |

Queue API notes:

- The success status `0x72000` is repeatedly compared in queue post/wait functions.
- Queue handles begin with a magic/header check against globals.
- Queue object type is read from a 16-bit field and appears to be one of at least three
  modes:
  - type `1`: normal send/receive queue;
  - type `2`: notification/event-like queue;
  - type `3`: ringbuffer/ring-like transport.
- Post takes a 32-bit priority/payload-selector field and rejects values `> 7`.
- Wait has a timeout field and appears to support multiple queue consumers/slots.

## Named RTOS queues and tasks

Important RTOS strings:

| Address | Name | Notes |
|---|---|---|
| `0x8013f5d0` | `TE_TERM_MQ` | TEngine terminal/message queue |
| `0x8013f5f4` | `LI_TERM_MQ` | Linux terminal/message queue |
| `0x80153be8` | `OSAL_CB_HDR_LI_MAIN` | Linux main callback-header queue |
| `0x80153c14` | `OSAL_CB_HDR_TE` | TEngine callback-header queue |
| `0x8013efd4` | `IOSC_HDR` | IOSC header task/object |
| `0x8013ed0c` | `LI_PRM_REC_MQ` | Linux PRM receive queue |
| `0x8048e888` | `LI_PRM_REC_MQ` | second use of PRM receive queue name |

`FUN_8013f2b0` is the best-documented queue task so far:

- It references both `TE_TERM_MQ` and `LI_TERM_MQ`.
- It uses the OSAL queue registration/post/wait helpers.
- It posts a short message whose first word is `0xe` when a queue pointer is available.
- It then repeatedly waits on messages and dispatches small command codes.
- It has no direct xrefs found in the current Ghidra analysis, so it is probably started
  through an indirect task table or RTOS object config.

`FUN_80153a54` is likely the callback-header dispatcher:

- It creates `OSAL_CB_HDR_LI_MAIN` and `OSAL_CB_HDR_TE`.
- It stores global queue/header state around `DAT_80153c24`.
- It waits on `DAT_80153c24 + 0xc`.
- Message type `7` is routed through a queue table whose entries appear to be stride
  `0x164` bytes.

## Linux OSAL callback dispatch details

The Linux `libosal_linux_so.so` side has a callback dispatcher that is the missing
bridge between a generic IOSC/OSAL queue message and the system command handler.

Important Linux-side functions:

| Function | Address | Observed role |
|---|---:|---|
| `vCallbackHandler` | `0x485052e4` | creates `OSAL_CB_HDR_LI_MAIN`, `NOIOSC_CB_HDR_LI_0` through `NOIOSC_CB_HDR_LI_29`, and `OSAL_CB_HDR_TE`; waits on the main callback-header queue and forwards callback messages to per-process local queues |
| `vNewCallbackHandler` | `0x48505060` | opens `OSAL_CB_HDR_LI_MAIN` and the local `NOIOSC_CB_HDR_LI_%d` queue for a process, waits on the local queue, then executes registered callbacks |
| `u32SendToMessageQueue` | `0x4850a878` | posts a new IOSC queue message and, when the queue owner is the current process or notification settings require it, emits a callback-header message with type `7` |
| `vSysCallbackHandler` | `0x4851dcc0` | Bosch system callback command dispatcher; command `0x1a` starts a process |

Observed callback message pattern for the callback-header queues:

```c
struct osal_callback_header_message {
    uint32_t type;       // message word 0
    uint32_t proc_id;    // message word 1
    uint32_t arg;        // message word 2; callback pointer/id for type 6/0xf paths
    uint8_t rest[0x50 - 12];
};
```

Observed handling in `vCallbackHandler` and `vNewCallbackHandler`:

- message size is `0x50`;
- type `6` and `0xf` are handled as direct local callback execution;
- type `7` selects a callback-table entry and executes the registered callback under a
  semaphore;
- non-local type `7` messages are forwarded to the owning process' local
  `NOIOSC_CB_HDR_LI_%d` queue.

In the current emulator trace, `procbaselx` creates the queue names without the leading
slash in POSIX mqueue (`NOIOSC_CB_HDR_LI_0`...`NOIOSC_CB_HDR_LI_29`), even though the
source path creates `/OSAL_CB_HDR_LI_MAIN` style names in Ghidra. The emulator treats
either bare or slash-prefixed boot-ready queues as ready.

## IOSC shared memory and IPI

IOSC is the shared-memory/mailbox layer used between Linux and RTOS.

Key functions:

| Function | Confidence | Observed role |
|---|---|---|
| `FUN_8009a1e0` | high | IOSC shared-region initializer |
| `FUN_8009a3a0` | high | IOSC IRQ/IPI bring-up |
| `FUN_8009bb4c` | medium | mailbox/queue creation helper reached from IOSC init |

`FUN_8009a1e0`:

- runs only once, using a ready flag at `*DAT_8009a340`;
- stores the configured shared region size;
- calls `FUN_8009a3a0`;
- allocates/initializes several fixed regions in the shared memory block;
- calls `FUN_8009bb4c` and `FUN_8009bcb4`;
- marks IOSC ready by writing `1` to `*DAT_8009a340`.

`FUN_8009a3a0`:

- clears/initializes the IOSC control block over a bounded region;
- stores start/end fields in `DAT_8009a468`;
- calls `thunk_FUN_80094ff4(0x2f, &enabled)` with `enabled = 1`;
- looks up a named priority with the string `IPI_IOSC_INT_PRIO`;
- calls `FUN_800edee0(0x2f, priority)`.

This establishes `IRQ 0x2f` as the IOSC interrupt/IPI path used by RTOS code.

The Linux emulator currently implements `/dev/iosc` locally in the shared namespace.
That is enough for local Linux-Linux primitives, but it does not connect to the RTOS
state, mailboxes, or IPI side.

## OSAL and subsystem init

RTOS init functions observed:

| Function | Confidence | Observed role |
|---|---|---|
| `FUN_8013118c` | medium | OSAL init wrapper |
| `FUN_80131568` | medium | core OSAL init body |
| `FUN_803e7cac` | high | DL core init creates DL message queues |

String evidence around DL init:

- `dl_MessageQueueMain`
- `dl_MessageQueueLuaHandler`
- `dl_MessageQueueClock`
- `dl_MessageQueueToDLCore`

These queues are on the RTOS/DL side. They are not currently implemented in the emulator,
but they indicate that DL, Lua handler, clock, and DL-core messaging are all RTOS-owned.

## Registry and process control strings

The RTOS contains many Bosch registry paths, for example:

```text
/dev/registry/LOCAL_MACHINE
/dev/registry/LOCAL_MACHINE/SOFTWARE
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/SYSTEM
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/VERSIONS
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/VERSIONS/OSAL
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/VERSIONS/BOARDCFG
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/BASE
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/LBASE
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/CONFIG
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/PROCMW
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/NAVAPP
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/MAP
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/HMI
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/VIDEO
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/VOICE
/dev/registry/LOCAL_MACHINE/SOFTWARE/BLAUPUNKT/PROCESS/SDS
```

Process-related strings include:

```text
/nor0/processes/ProcBase.out
ProcMP1.out
ProcHMI.out
ProcMM.out
ProcCHN.out
ProcMAP.out
ProcMW.out
ProcBase.out
ProcDiag.out
ProcVideo.out
ProcSds.out
ProcNav.out
ProcBase
LI_PRM_REC_MQ
OSAL_ProcessSpawn %s Start Successful
OSAL_ProcessSpawn Error %s
OSAL_ProcessSpawn for Sysprog %s Start Successful
OSAL_ProcessSpawn for Sysprog Error %s
OSAL_START_PROC <path to bin>
OSAL_START_SYSPROC <path to bin>
OSAL_START_LODSPG <path to bin>
```

These strings prove that RTOS owns a process model for both RTOS processes and Linux
process names. The registry paths under `SOFTWARE/BLAUPUNKT/PROCESS` are likely used to
read start configuration for `BASE`, `LBASE`, `HMI`, `MAP`, `NAVAPP`, and other modules.

## Start-process command surfaces

There are two related but not identical command surfaces:

### Linux `procbaselx` start-process command

The Linux side handler is in `libosal_linux_so.so`:

| Item | Address |
|---|---:|
| `vSysCallbackHandler` | `0x4851dcc0` |
| `vStartProc` thunk | `0x484eb03c` |
| `vStartProc` real | `0x4851caec` |
| `OSAL_ProcessSpawn` thunk | `0x484ea16c` |
| `OSAL_ProcessSpawn` real | `0x48516d0c` |

Observed command header layout:

```c
struct osal_callback_message {
    uint8_t unknown0[2];   // bytes 0 and 1
    uint8_t command;       // byte 2
    uint8_t payload[];     // starts at byte offset 3
};
```

For Linux `procbaselx`, command `0x1a` is the start-process command:

```asm
; inside vSysCallbackHandler, command byte [r0 + 2]
; case 0x1a
add  r0, r0, #0x3       ; r0 = payload pointer
mov  r1, #0x3           ; option = 3
bl   vStartProc         ; 0x484eb03c thunk -> 0x4851caec real
```

`vStartProc(path, option)` currently only supports `option == 3`. It copies `path` into a
100-byte local buffer with `snprintf(buffer, 100, "%s", path)`, so the practical payload
path limit is 99 characters. For `option == 3` it builds an OSAL process descriptor on the
stack:

```c
struct osal_process_descriptor {
    char *app_name;          // copied path/name
    uint32_t prio;           // 0 from vStartProc
    char *binary_or_module;  // path string, for example /opt/bosch/processes/prochmi_out.out
    char *cmdline;           // empty/default argument string from vStartProc
    char *cgroup;            // default cgroup pointer from vStartProc
};
```

Then it calls `OSAL_ProcessSpawn(&desc)`.

`OSAL_ProcessSpawn` semantics:

- If `desc->binary_or_module` does **not** end in `.out` or `.OUT`, it is treated as a
  shared library/module and is `dlopen()`ed. If the module is `libprocbase_so.so`, its
  entry point is called in-process. Otherwise `OSAL_ThreadSpawn()` runs the module entry
  point on a thread.
- If it **does** end in `.out` or `.OUT`, it tokenises `desc->cmdline` on `" \t\n"` into
  `argv`, checks a maximum OSAL process count, calls `fork()`, and in the child:
  - applies CPU affinity if `argv` contains `bindcpu0` or `bindcpu1`;
  - applies the cgroup and nice level;
  - calls `execvp(argv[0], argv)`;
  - on `exec` failure writes to ErrMem, asserts, and `_exit(1)`.

Therefore the minimum Linux start-process payload can be quite small:

```c
struct linux_start_proc_message {
    uint8_t unknown0[2];
    uint8_t command;       // 0x1a
    char path[];           // NUL-terminated process path or .out binary
};
```

The current emulator cannot faithfully complete that path because `fork`/`execve` is not
implemented in the required way. For GUI bring-up it is better to intercept the logical
intent ("start `prochmi_out.out`") and spawn the target through the emulator's
multi-process runner.

### RTOS TEngine start-process command

The RTOS TEngine/OSAL dispatcher is:

```text
FUN_80489270 @ 0x80489270
```

It uses the same generic header pattern: command byte at message offset `+2`, payload at
offset `+3`.

Important observed command mappings:

| RTOS command byte | Handler target | Observed role |
|---:|---|---|
| `0x1a` | default branch `0x80489b44` | not the RTOS start-process command in this dispatcher |
| `0x20` | `0x8048945c` -> `FUN_804915f4(path, 0, 0)` | directory/list helper |
| `0x21` | `0x804894ac` -> `FUN_8048b488(path, 3)` | start process / starter path |
| `0x22` | `0x804894b4` -> `FUN_8048b488(path, 2)` | variant start-process path |
| `0x23` | `0x804894bc` -> `FUN_8048b488(path, 1)` | variant start-process path |

`FUN_8048b488(path, option)` builds a starter request:

```c
struct rtos_starter_request {
    uint32_t field_00;   // 0xffffffff
    uint32_t field_04;   // DAT_8048b4f4 = 0x1041
    uint32_t field_08;   // DAT_8048b4f8 = 0x7168, replaced by DAT_8048b4fc = 0x70bc for option 3
    uint32_t field_0c;   // 0x3c, likely priority or service class
    uint32_t field_10;   // 0x2000, likely stack size
    char name[8];        // "Starter"
};
```

It then uses `ADTKLIB_SC_TBL` through helper `FUN_80491ba0`:

1. calls the service table "create/open" method with the starter request;
2. if that returns a positive handle, calls the service table "start" method with the
   path string from the message payload;
3. calls a cleanup/delete-like method with timeout `100`.

This strongly matches the `OSAL_START_PROC`, `OSAL_START_SYSPROC`, and
`OSAL_START_LODSPG` debug command family. The exact meaning of service IDs `0x1041`,
`0x7168`, and `0x70bc` is not yet decoded.

## Current emulator RTOS boot backend

The repository now contains a minimal host-side RTOS boot backend in
`nissan_connect3_emulator/src/rtos/`.

Current behavior:

- `Emulator::run_processes` starts the configured initial Linux processes.
- The RTOS backend is enabled by default, including when `EMU_PROCESSES` is set; set
  `EMU_RTOS=0|off|false` to disable it.
- `RtosQueueInteraction` mirrors the RTOS queue tasks mapped from `triton_mid_raw.bin`:
  - the terminal task path (`FUN_8013f2b0`) creates `TE_TERM_MQ` and `LI_TERM_MQ`
    (`maxmsg=10`, `msgsize=0x50`), posts the initial ready word `0x0e` to
    `LI_TERM_MQ`, and consumes terminal commands from `TE_TERM_MQ`;
  - the callback-header task path (`FUN_80153a54`) creates `OSAL_CB_HDR_LI_MAIN`
    (`maxmsg=0xf0`) and `OSAL_CB_HDR_TE` (`maxmsg=0x78`), both with `msgsize=0x50`,
    consumes callback headers from `OSAL_CB_HDR_TE`, and only type `7` would dispatch
    the RTOS callback table;
  - startup start-process messages are queued before initial Linux processes spawn so
    waiting processes can consume them immediately.
- The default RTOS start-process path is queue-driven: it posts the Linux OSAL callback
  command `[0, 0, 0x1a, path..., 0]` to `OSAL_CB_HDR_LI_MAIN`.
- The libosal guest bridge intercepts an intercepted `OSAL_s32MessageQueueWait` for
  `OSAL_CB_HDR_LI_MAIN`; when it sees command `0x1a`, it allocates the guest path buffer
  and invokes the real OSAL helper `vStartProc(path, 3)` inside the waiting process.
- `OSAL_ProcessSpawn` interception then launches `.out` targets through the emulator's
  multi-process runner; the guest observes the normal `Start Process succeeded` trace.
- Direct host-side spawn is disabled by default and kept only as an explicit fallback.
- Default dynamically started targets:
  - `/opt/bosch/processes/prochmi_out.out`
  - `/opt/bosch/processes/procmapengine.out`
- Environment knobs:
  - `EMU_RTOS=1|off` enables/disables the RTOS backend; it is enabled by default;
  - `EMU_RTOS_START=path1:path2` selects start-process targets;
  - `EMU_RTOS_START_QUEUE=NAME` selects the RTOS-to-Linux queue for startup commands;
  - `EMU_RTOS_START_MESSAGE_FORMAT=terminal|callback` selects the startup message layout;
  - `EMU_RTOS_QUEUE_BOOT=0` disables the queue-driven start-process path;
  - `EMU_RTOS_DIRECT_SPAWN=1` re-enables the direct host-side spawn fallback;
  - `EMU_RTOS_WAIT_TE_READY=1|off` controls whether startup queue messages wait for the
    Linux terminal-ready message, with `EMU_RTOS_TE_READY_TIMEOUT_MS=N` as a safety timeout;
  - `EMU_RTOS_TERMINAL_ACK=1` posts an optional RTOS terminal-ready ack after Linux
    sends its `TE_TERM_MQ` ready message;
  - `EMU_RTOS_READY_QUEUE=NAME` overrides the boot-ready queue used by the fallback
    boot service;
  - `EMU_RTOS_READY_TIMEOUT_MS=N` and `EMU_RTOS_START_DELAY_MS=N` tune fallback timing.

This is intentionally a boot-controller backend, not a full RTOS guest. It now performs
RTOS-side queue traffic and reaches `vStartProc`/`OSAL_ProcessSpawn`, but the exact
RTOS start-process producer still uses the Linux callback command layout as an emulator
bridge rather than a fully decoded RTOS callback-header table dispatch.

## Relation to `procbaselx`

The Linux-side behavior is summarized in `docs/processes/procbaselx.md`:

- `procbaselx_out.out` initializes OSAL/IOSC/device subsystems;
- it spawns internal worker threads, not child processes, during its own startup;
- its main loop waits on OSAL/IOSC/registry state;
- its Linux OSAL library dispatches command `0x1a` to `vStartProc`;
- `OSAL_ProcessSpawn` starts `.out` targets through `fork()` + `execvp()`.

This means a bare Linux-only emulator run cannot naturally produce a GUI because no
external RTOS-side client sends the start-process command and the emulator lacks the
real `fork`/`execve` path.

## Emulator integration direction

Short term:

- Add an RTOS stub/backend that directly injects the minimum IPC/IOSC/queue traffic
  needed by `procbaselx`.
- Prefer launching GUI processes through the emulator's existing multi-process model
  rather than faithfully emulating `fork` + `execve`.

Medium term:

- Create a host-side `src/rtos` service layer in the emulator.
- Back `/dev/iosc` objects with RTOS-side state.
- Add named queue adapters for at least:
  - `LI_TERM_MQ`
  - `TE_TERM_MQ`
  - `OSAL_CB_HDR_LI_MAIN`
  - `OSAL_CB_HDR_TE`
  - `LI_PRM_REC_MQ`
  - `IOSC_HDR`
- Add a minimal registry backend for paths queried during boot.

Long term:

- Either reverse enough of `triton_mid.bin` to run a native RTOS guest VM in parallel
  with the Linux VM, or implement a faithful RTOS emulation backend for the subset used
  by Linux userspace.

The medium-term backend is probably cheaper and more controllable for GUI bring-up than
running the real RTOS image, but it needs more RTOS-side layout/message-format data.

## Known gaps

Current blockers before `docs/rtos.md` can become an implementation-ready spec:

1. RTOS task startup tables are not decoded.
   - `FUN_8013f2b0`, `FUN_80153a54`, `FUN_8009a1e0`, `FUN_8013118c`, and similar
     initializers currently have no obvious direct callers in Ghidra.

2. IOSC shared-memory layout is not mapped.
   - The emulator currently allocates IOSC shared buffers with `IOSC_SHARED_MALLOC`.
   - Real RTOS expects fixed regions and control structures.
   - Linux-side size hints are known (`0x15d0c`, `0x1054`, etc.), but their semantic
     layout is not yet decoded.

3. OSAL message format is not fully decoded.
   - Queue post/wait functions are visible, but the payload header and per-message struct
     are not yet understood.

4. The RTOS-to-Linux boot trigger is not yet implemented faithfully.
   - Linux `procbaselx` accepts command `0x1a` with a NUL-terminated path at payload
     offset `+3`.
   - RTOS `FUN_80489270` has its own start-process-like command byte `0x21`, but the
     producer that emits the boot-time message has not been found.
   - The ADTK Starter service IDs used by `FUN_8048b488` are still not fully decoded.
   - The current `src/rtos` backend mirrors the mapped RTOS terminal and callback
     header queues. The `0x1a` start-process producer on `triton_mid` has not yet been
     mapped to those queues; optional `EMU_RTOS_QUEUE_BOOT=1` only exercises that
     hypothesis.

5. Registry backend behavior is unknown.
   - Many registry paths exist, including `SOFTWARE/BLAUPUNKT/PROCESS/BASE` and
     `PROCESS/LBASE`.
   - Some Linux processes appear to expect `/dev/registry`, but the emulator currently
     has no meaningful registry service.

6. Queue notification semantics need exact mapping.
   - RTOS supports `MQ_EVENT_NOTIFY` and ringbuffer-like queue types.
   - Linux side uses both POSIX mqueue notification and `/dev/iosc` objects.
   - The bridge between those two representations is still missing.

## Recommended next analysis targets

High-value Ghidra follow-ups:

- Find startup table entries or indirect references that start:
  - `FUN_8013f2b0`
  - `FUN_80153a54`
  - `FUN_8009a1e0`
  - `FUN_8013118c`
  - `FUN_803e7cac`
- Decode the IOSC control block written by `FUN_8009a1e0` / `FUN_8009a3a0`.
- Trace callers of the queue registration function `FUN_8014a468`.
- Identify the command producer that posts the Linux `0x1a` start-process command into
  the Linux OSAL callback path. For the current emulator workaround, the RTOS backend
  launches the target directly, but the faithful target is still a callback-table or
  queue notification that reaches `vSysCallbackHandler`.
- Implement a guest-side delivery path for `0x1a` by either:
  - injecting a callback-header message with the registered `vSysCallbackHandler`
    callback id/pointer, or
  - adding an emulator-side `OSAL_ProcessSpawn` interception that creates a real child
    guest process and updates `procbaselx` wait state.
- Trace RTOS command producer that calls `FUN_80489270` and understand whether its
  `0x21` starter command is used during boot or only by debug/TCL clients.
- Decode `ADTKLIB_SC_TBL` service IDs `0x1041`, `0x7168`, and `0x70bc` used by
  `FUN_8048b488`.
- Map the `OSAL_CB_HDR_*` message header format, especially type `7`.
- Correlate Linux `libiosclib_so.so` ioctl numbers with the RTOS-side handlers for the
  corresponding IOSC operations.
