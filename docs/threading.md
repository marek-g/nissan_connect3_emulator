# Threading model

The emulator runs a multi-process guest firmware. This document describes how
processes, threads and memory are mapped onto host resources, and the invariants
that keep the design sound.

## Goal

Run several guest processes at the same time with **true parallelism** across
processes, while their guest threads and address spaces behave like Linux
2.6.32 on ARM.

## Mapping

- **Guest process → one host thread + one `Unicorn` VM.** Each process has its
  own address space and is scheduled by its own `std::thread`, so two processes
  run on two CPU cores simultaneously.
  - Entry point: `Emulator::run_processes` spawns one thread per `ProcessSpec`
    and joins them all.
  - Per-process entry: `scheduler::run_process_loop`, which runs that process'
    VM until it exits.
- **Guest threads *within* a process → cooperative round-robin on that process'
  single host thread.** They share the process' one VM; the host thread switches
  their CPU state in/out with `context_save`/`context_restore`. Each guest
  thread gets a `TIMESLICE_INSTRUCTIONS` slice only when another guest thread in
  the same process is also runnable; a lone runnable thread runs until it blocks
  or exits.

## Shared vs. private state

Only `Sync` state is shared between host threads:

- `file_system` (`Arc<Mutex<MountFileSystem>>`)
- `namespace` (`Arc<Mutex<SystemNamespace>>`) - the "kernel": message queues,
  IOSC objects, named shared memory, and the per-process doorbells
- `next_thread_id` (`Arc<AtomicU32>`) - one global counter so guest thread ids are
  unique across every process

Everything else is private to one process' host thread: its `Mmu`/address space,
its `SysCallsState`, its guest-thread list, and its `Unicorn`.

## Why the VM must be built on its own host thread

`Unicorn::Context` (a saved guest CPU state from `context_init`/`context_alloc`)
is a raw FFI pointer tied to the `uc_engine` that allocated it. It is therefore
neither `Send` nor `Sync`, and saving on one VM and restoring on another is
simply invalid. Because each guest thread's context is only ever created and
restored on its own VM, it never needs to cross a thread boundary - but the
compiler does not know that, so the process' private state (including the
guest-thread list holding those contexts, and the `Unicorn` itself) is
constructed *inside* the process' host thread, in `Process::setup`. Nothing
non-`Send` is ever moved across a thread.

## Sleeping, waking, and cross-process IPC

Each process owns a `Wake` doorbell (`Mutex<bool>` + `Condvar`), registered in
`namespace.wakes`.

- When a process has no runnable guest thread, its host thread **parks** on its
  doorbell. The wait is bounded by the nearest blocked-thread deadline, clamped
  to a `POLL_INTERVAL` tick, so a parked thread wakes on its own timeout even if
  a wake is missed.
- When any host thread opens a shared object (posts an mq message, releases an
  IOSC mutex/event/semaphore, frees a queue slot), it calls
  `namespace.notify_waiters()`, which rings every doorbell. Each parked process
  wakes and re-checks the objects it is blocked on.
- The doorbell's flag closes the lost-wakeup race: the waker sets it under the
  same mutex the waiter holds while deciding to park.

### Completing an IPC wait happens in the waiter's own VM

A process' guest memory is private to its VM. So a blocked waiter is completed
by **its own** host thread (the `finish_mq_wait` / `finish_iosc_wait` re-check
helpers in `advance_blocked`), never by a peer thread writing foreign memory.
Concretely:

- **Message queues.** Payloads live in the namespace queue (a host buffer), not
  in either process' memory. A blocked sender copies its message into a
  namespace staging buffer when it blocks (`MqState::staged`); whichever host
  thread later frees a slot moves those staged bytes into the queue. A receiver
  pops from the queue and copies into its own buffer in its own VM. This
  replaces the old single-address-space "pipelined send/receive" which assumed
  it could read/write another thread's buffers directly.
- **IOSC.** The object state (lock flag, event value, semaphore count) lives in
  the namespace; the completer only opens it, and the waiter grabs it atomically
  under the namespace lock when it re-checks in its own VM. When several
  processes wake on one object, exactly one wins.
- **Named shared memory (`/dev/shm/*`, `mmap(MAP_SHARED)`)** is a single native
  host allocation that every process maps into its own VM via
  `Unicorn::mem_map_ptr`, so the mapping aliases the same bytes in all
  processes.

### Termination

A process finishes when it requests exit (`exit_group`/`reboot`) or when all its
guest threads have exited. `run_processes` joins every host thread and returns
the first error, if any.

## Known limitations

- **Cross-process futex** (a `FUTEX_WAKE` without `FUTEX_PRIVATE_FLAG`) is not
  implemented - `futex::wake_waiters` only scans the current VM's threads, so a
  waiter in another process is not woken and sleeps until its deadline. Same-
  process (pthread) futexes are unaffected. See `issues/correctness.md`.
- A missed cross-process wake is only possible in principle; the poll tick bounds
  the resulting latency to `POLL_INTERVAL`.
