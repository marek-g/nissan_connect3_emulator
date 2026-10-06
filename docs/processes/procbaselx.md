# procbaselx

Binary: `/opt/bosch/processes/procbaselx_out.out`
Libs analysed (Ghidra): `libosal_linux_so.so` (base `0x484d8000`), `libiosclib_so.so` (base `0x48118000`).

## Role

`procbaselx` is a **foundation / base process**, not the GUI itself. It brings up the
OSAL (OS Abstraction Layer) and the full set of I/O device drivers, then sits in a
service main loop waiting for commands over IPC. There is no "GUI starting" marker in
its own code path — it is the substrate that other processes (e.g. `prochmi`, `proccgs`)
run on top of / alongside.

Observed behaviour in the emulator (stable run, 60 s, no reboot):

1. **Init** (`bOnProcessAttach` @ `0x485084c0` and friends) — a large device/subsystem
   bring-up: `vInitOsalCoreIOSC`, `s32OsalDrvInit`, `IoscRegistryInit`,
   `vMqueueLiMqMapInit`, `vInitMessagePool`, plus per-device init such as
   `KDS_s32IODeviceInit`, `DEV_FFD_s32IODeviceInit`, `BT_UGZZC_s32IODeviceInit`,
   `ACOUSTIC{IN,OUT,SRC}_s32Init`, `prm_vInit` / `prm_CreateLibUsbConnectionTask`
   (USB/PRM), `DRV_DIAG_EOL_s32IODeviceInit`, `bDrvBtAsipInit`.
2. **Threads** — it spawns worker threads via pthread `clone` (e.g. the IOSC header
   task from `vStartIoscHdrTsk`). In the trace these are threads `[1]..[14]`; there is
   no process-level `fork`/`execve` during its own startup.
3. **Main loop** — the main thread idles in a poll loop:
   `OSAL_IOOpen` → registry lock (`bLockRegistry`/IOSC enter/leave mutex) →
   `OSAL_s32IOControl`/`OSAL_s32IOWrite` (trace out) → `OSAL_s32ThreadWait` →
   `nanosleep` → repeat. This is its healthy waiting state, not a hang.

## It listens for commands (IPC command dispatcher)

`vSysCallbackHandler` @ `0x4851dcc0` is a large `switch` on a command code
(`param_1[2]`) dispatched from an IPC system-callback message queue. The payload lives
at `param_1+3`. Observed (implemented, non-empty) cases:

| Code | Handler | Purpose |
|------|---------|---------|
| 0x11 | `u32Read_Rem_Dir` | list remote directory |
| 0x12 | `vGetResourceData` | fetch resource data |
| 0x14 | `vCopyDir` | copy directory |
| 0x15 | `vMkDir` | make directory |
| 0x16 | `vRmDir` | remove directory |
| 0x17 | `vCopyFile` | copy file |
| 0x18 | `vRmFile` | remove file |
| 0x19 | `u32Read_Rem_Dir` | read remote dir (variant) |
| **0x1a** | **`vStartProc(payload, 3)`** | **start a process** |
| 0x1f | `vGetMsgQueueStatus` | message-queue status |
| 0x20 | `vRmFileSelection` | remove file selection |
| 0x22 | `vGetMsgQueueMaxFillLevels` | MQ max fill levels |
| 0x26 | `vReadFile` | read file |
| 0x51 | `OSAL_vSetAssertMode` | set assert mode |
| 0x53 | `OSAL_vAssertFunction` | trace/assert hook |
| 0x5f | `vSetTraceFlagForChannel` | per-channel trace flag |
| 0x6a | `vShowCurrentResourceSitutaion` | dump resource situation |
| 0x76 | `vSetEmptyPoolInvestigation` | pool investigation flag |
| 0xa0 | `vSetTraceFlagForSem` | per-sem trace flag |
| 0xa1 | `vSetTraceFlagForEvent` | per-event trace flag |
| 0xa2 | `vActivateTimerTrace` | activate timer trace |
| 0xa3 | `TraceSpecificPoolInfo` | trace specific pools |
| 0xa4 | `vSetTraceFlagForShMem` | per-shmem trace flag |
| 0xb0 | *(global store)* | set flag byte |
| 0xb1 | `vSetFilterForCcaMsg` | CCA message filter |
| 0xf0 | `vDisplayManual` | display manual |

So `procbaselx` acts as a **service / diagnostics hub**: most commands are trace,
resource, file/directory and message-queue introspection; one command starts processes.

## It can start other processes (on demand)

Call chain for the 0x1a "start process" command:

```
vSysCallbackHandler (case 0x1a)
  -> vStartProc(name, option=3)          real @ 0x4851caec, thunk @ 0x484eb03c
       -> OSAL_ProcessSpawn(desc)         real @ 0x48516d0c, thunk @ 0x484ea16c
            -> fork() + execvp(argv)      if desc[2] ends in ".out"/".OUT"
```

`OSAL_ProcessSpawn(desc)` behaviour:

- `desc` layout (words): `[0]=AppName`, `[1]=Prio`, `[2]=module/binary name`,
  `[3]=command line / args`, `[4]=cgroup`.
- It tokenises `desc[3]` on `" \t\n"` to build `argv`.
- If `desc[2]` **does not** end in `.out`/`.OUT`: it is a module — `pvLoadModule`
  (`dlopen`) it; if the name is `libprocbase_so.so` it calls an entry point in-process,
  otherwise it runs it on a new thread via `OSAL_ThreadSpawn`.
- If `desc[2]` **does** end in `.out`/`.OUT`: it enforces a max-process count
  (`"Max Count OSAL processes reached"`), then `fork()`s. In the child it applies CPU
  affinity (if `argv` contains `bindcpu0`/`bindcpu1` → `s32ProcessSetAffinity`),
  `vSetCgroup(desc[4])`, `vSetNiceLevel(desc)`, and `execvp(argv)`. On exec failure it
  writes to the error mem, asserts, and `_exit(1)`.

Key point: **procbaselx does not start processes autonomously at startup.** It only
launches one when it *receives* a 0x1a command carrying the target binary name (e.g.
`prochmi_out.out`). `OSAL_ProcessSpawn` is never invoked in a bare single-process run,
which is why no child is spawned by itself.

Current emulator behavior: the default RTOS backend pre-queues a Linux OSAL callback
command `[0, 0, 0x1a, path..., 0]` on `OSAL_CB_HDR_LI_MAIN`. The libosal message bridge
delivers that queued command to the guest by invoking `vStartProc(path, 3)`, which then
calls `OSAL_ProcessSpawn`. The existing `OSAL_ProcessSpawn` interception launches `.out`
binaries as separate emulator processes.

Supporting process-table APIs also exist: `s32ProcessTableCreate`,
`tProcessTableGetFreeIndex`, `vAddProcessEntry`, `OSAL_s32ProcessControlBlock`,
`OSAL_s32ProcessJoin`, `OSAL_s32ProcessDelete`, `vOnProcessDetach`,
`s32ProcessSetAffinity`.

## Implications for the emulator

- The emulator does **not** implement `fork`/`execve`, and the architecture is one VM
  per process with a shared kernel namespace. Therefore emulating procbaselx's own
  `fork`+`exec` path is out of scope.
- To get the GUI running, launch the target process (e.g. `prochmi_out.out`) as a
  **separate process alongside** procbaselx via the multi-process runner
  (`Emulator::run_processes` / `ProcessSpec`), sharing the namespace so IPC (message
  queues / IOSC) between them works.
- Alternatively, inject a 0x1a "start process" command into procbaselx's system-callback
  queue and intercept `OSAL_ProcessSpawn`. This is now the default bring-up path: the
  RTOS backend queues the command, libosal delivery calls `vStartProc`, and the host
  launcher starts the requested guest binary without implementing guest `fork`/`execve`.
