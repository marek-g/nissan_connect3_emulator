# Message queues

Two independent queue layers exist in the guest. This document describes how
the emulator represents each, what the guest sees at the API boundary, and
the invariants that keep them coherent across the parallel per-process VMs.

| Layer | Guest API | Owner | Emulator state |
|---|---|---|---|
| Kernel POSIX mq | `mq_open`, `mq_send`, `mq_timedreceive` | kernel (namespace) | `MqState` (`src/common/queues.rs`) |
| OSAL "message handle" | `OSAL_s32MessageCreate`, `OSAL_s32MessageDelete`, `OSAL_pu8MessageContentGet` | per-process libosal | `OsalMessagePool` (`src/os/syscalls/sys_calls_state.rs`) |

Both layers cooperate. A guest `OSAL_s32MessageQueuePost` first creates an
OSAL message handle (per-process), then pushes the handle's 8-byte
*reference* onto a kernel POSIX mq (shared). The receiver pops the
reference from the kernel queue, materialises it in *its own* VM, and only
then calls `OSAL_pu8MessageContentGet` to reach the payload.

## Kernel POSIX mq (`MqState`)

`MqState` lives inside the shared `SystemNamespace` (`Arc<Mutex<…>>`) so
every process' host thread can post to and receive from the same queue.
Per queue we track:

- `name`, `maxmsg`, `msgsize` - as passed to `mq_open`
- `creator` (`Guest` or `Rtos`) and `guest_open_count` - distinguishes a
  queue that only the RTOS pre-created from one actually opened by a Linux
  process (boot-readiness gates depend on this).
- `messages: Vec<MqMessage>` - sorted ascending by priority, FIFO within a
  priority. Pop takes the *last* element.
- `open_count`, `unlinked` - kernel reference counting.
- `notify_owner` + `notify` - the `struct sigevent` from `mq_notify`, with
  the three flavours from POSIX (`SIGEV_NONE`, `SIGEV_SIGNAL`,
  `SIGEV_THREAD`).
- `waiters: HashMap<queue_id, Vec<tid>>` - blocked senders and receivers,
  distinguished by their `BlockReason`.
- `staged: HashMap<tid, (queue_id, Vec<u8>, priority)>` - bytes copied out
  of the sender's private VM at block time. When another host thread frees
  a slot, that thread moves the staged bytes into the queue. This lets a
  sender in one Unicorn VM hand data to a receiver in another VM without
  any cross-VM memory read.

Canonical name lookup goes through `canonical_mq_name` so `/mbx_0`,
`mbx_0` and `./mbx_0` all resolve to the same queue.

## OSAL message pool (`OsalMessagePool`)

Every process' `SysCallsState` owns one `OsalMessagePool`. `amt_tcl*`
message classes in the guest allocate their payloads through this pool:

- On first `OSAL_s32MessageCreate`, the hook allocates a contiguous region
  of `1024 * 0x1000` bytes on the process' heap and calls
  `set_base(base)`. That region is the **static pool**: 1024 fixed-size
  slots of `0x1000` bytes each, tracked by `used: Vec<bool>`.
- Requests that fit `chunk_size` take a slot. Requests that don't fit
  (or when the pool is exhausted) are allocated directly on the process'
  heap and their address is inserted into `dynamic: HashSet<u32>` via
  `mark_dynamic(content)`.

`release(content)` handles both kinds:

1. If `content` is in `dynamic`, remove it and return `true`.
2. Otherwise check whether `content` lies on a slot boundary inside
   `[base, base + slots*chunk)` and the slot is currently marked used.
   Free it and return `true`.
3. Otherwise return `false`.

`release` returning `false` is not benign. The guest's
`amt_tclMappableMessage::bDelete` (procmap+0x388ef0) is exactly:

```c
if (*(int *)(this + 4) == 0) return 0;
iVar1 = OSAL_s32MessageDelete(*(undefined4 *)(this + 0xc),
                              *(undefined4 *)(this + 0x10));
if (iVar1 != 0)
    OSAL_vAssertFunction("ALWAYS", "amt_MMObj.cpp", 0x800000b6);
```

A `false` from `release` means our `OSAL_s32MessageDelete` hook falls
through to libosal's real Delete, which cannot find the slot and returns
non-zero, which trips the assert and aborts the process. Any host-side
path that fabricates an OSAL message handle must therefore register the
payload address with `mark_dynamic` before handing it to the guest.

## The 8-byte message reference

The kernel mq carries only an 8-byte *reference*:

```text
0x00: u32 type        (1 = "direct heap pointer", see TYPE_HEAP)
0x04: u32 content_ptr (guest VA inside the recipient's own VM)
```

This is what `OSAL_s32MessageQueueWait` writes to its `buf` argument and
returns as its byte count (`8`). Payload bytes are *not* in the kernel
queue. Only the recipient process' libosal knows how to dereference
`content_ptr` (and only from its own VM, since each process has its own
address space).

Consequence for host-side producers: we cannot pre-generate a
`content_ptr` that will be valid inside some other process' VM. Anything
we want the recipient to see must be materialised inside the recipient's
address space at the moment it calls `Wait`.

## PWR-proxy delivery path

The proxy (`src/rtos/pwr_proxy.rs` + `pwr_proxy_service_handle` in
`src/libs/libosal_linux/message.rs`) never intercepts the guest's Post
hook. It runs entirely inside the recipient's Wait hook:

1. The hook observes the recipient is about to Wait on `mbx_<app_id>`.
2. It pops the next pending `PendingPowerMessage` from the proxy.
3. It calls `mmu.heap_alloc` **inside the recipient's VM** to reserve
   `0x20` bytes and `mem_write` the encoded `PowerMessage` body.
4. It calls `state.osal_messages.mark_dynamic(content)` so a later
   `bDelete` from the guest finds the slot.
5. It writes `[1, content_ptr]` (the OSAL 8-byte reference) to the
   recipient's `buf` and returns 8 as if a real message arrived.

The `PowerMessage` body itself is 0x20 bytes, laid out by
`encode_power_message` to match `amt_tclPowerMessage::amt_tclPowerMessage`
at procmap+0x3882f8:

```text
0x00: u16 sender_app_id
0x02: u16 target_app_id
0x04: u32 message_length (= 0x20)
0x08: u16 message_type  (= 2 for PowerMessage)
0x0a: u8  (0)
0x0b: u8  flags         (= 0x40 marks this as a PowerMessage)
0x0c: u16 sub_id
0x0e: u16 param7        (= 0)
0x10: u32 param8        (= 0, timestamp in practice)
0x14: u16 power_type
0x16: u16 (0)
0x18: u32 power_data1   -- procmap reads this as r6 (new_state for REQ)
0x1c: u32 power_data2   -- procmap reads this as r4 (secondary data)
```

**Semantic mapping** (verified against procmap+0x667a30
`bHandleMsgPowerMessage` case 0x10). Procmap's disassembly at
0x667a5c/0x667a60 does:

```asm
ldrne r4, [r0, #0x1c]        ; r4 = power_data2
ldrne r6, [r0, #0x18]        ; r6 = power_data1
```

so `power_data1` (offset 0x18) is the field procmap treats as
*new_state*, and `power_data2` (offset 0x1c) is the secondary parameter.
`STATE_CHANGE_REQ` must set `power_data1 = APP_STATE_NORMAL (3)`; leaving
it at 2 makes `vOnNewAppState` see `param_1 == param_2` and skip.

## Boot-critical queue names

| Queue | Producer | Consumer | Purpose |
|---|---|---|---|
| `mbx_0` | every app | (real) PWR proxy | inbound CCA power messages |
| `mbx_<app_id>` | PWR proxy | app's entry-thread `vAppEntry` | outbound power messages to that app |
| `MQB_<app_id>` | app's entry-thread | app's body-thread `vAppBody` | body-thread forward queue |
| `NOIOSCM` | procmap `MapDataManager` | procmap `MapData` thread | internal job dispatch, no IOSC |
| `NOIOSCR` | procmap `AddElement` | procmap `RndCtrlThread` | render-control job dispatch |

Apps that participate in the power handshake today:

| app_id | binary | mbx queue |
|---|---|---|
| `0x0400` | `procmapengine.out` | `mbx_1024` |
| `0x0007` | `DAPIAPP.OUT` | `mbx_7` |
| `0x0109` | `prochmi_out.out` | `mbx_265` |

## Failure modes to watch for

- **`OSAL_vAssertFunction("ALWAYS", "amt_MMObj.cpp", 0x800000b6)`** →
  something fabricated a message handle without calling `mark_dynamic`,
  or released it twice.
- **`OSAL_s32MessageCreate emulated pool exhausted`** → static pool has no
  free slots. Should not happen since we fall back to `heap_alloc`, but
  indicates pathological allocation patterns upstream.
- **Wait returning non-8 byte counts** → recipient is expecting a payload
  directly on the queue (a raw mq_send path, not OSAL). Different layer.
- **Two `Wait` callers see the same `content_ptr`** → the sender wrote
  into the recipient's VM twice or a message was popped without a
  corresponding `bDelete`.