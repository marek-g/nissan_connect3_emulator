# Robustness: Panics on Guest-Controlled Input

The emulator must never panic because of something the guest program does. Every item below crashes the whole host process from a syscall handler or callback triggered by guest behavior.

## Other panics in syscall handlers on guest-controlled input

| Location | Problem | Fix |
|---|---|---|
| `os/syscalls/fcntl.rs:183` | `panic!("unsupported command")` for unknown fcntl commands | return `-EINVAL` |
| `os/syscalls/resource.rs:49` | panics on any getrlimit resource except RLIMIT_STACK — glibc probes several at startup | return `-EINVAL`/`-ENOSYS` for unhandled resources |
| `os/syscalls/mman.rs:154` | panics on unaligned mmap | return `-EINVAL` |
| `os/syscalls/socket.rs:66` | `String::from_utf8(...).unwrap()` on socket payload | handle non-UTF8 gracefully |
| `file_system/tmp_file_system.rs:311`, `file_system/proc_file_system.rs:130` | `todo!()` in ioctl | return `-ENOTTY`/`-ENOSYS` |
| `file_system/std_file_system.rs:130,145` | `mem_write().unwrap()` on guest address | propagate the error |
| `emulator/utils.rs:8-20` (`load_binary`) | `panic!("Cannot load file")` on open failure; `vec![0u8; size as usize]` from `get_length()` (a `u64`, 0 for unknown fds) can over-allocate for special files | return a `Result`; validate/bound the length before allocating |
| `emulator/elf_loader.rs:126,154,171,182` | `.expect()` on malformed/failed ELF loads | return `Err` for bad input |
## writev swallows errors and reports partial success

- **Location:** `nissan_connect3_emulator/src/os/syscalls/uio.rs:24-46`
- **Problem:** Unbounded `iovcnt*8` allocation, per-iovec alloc + `mem_read().unwrap()`, and `Err(_) => {}` (line 43) swallows write errors while still returning a partial byte count as success.
- **Fix:** bound `iovcnt`, propagate read/write errors as proper errno.

## read/write: unbounded per-syscall allocation sized by guest length

- **Location:** `nissan_connect3_emulator/src/os/syscalls/unistd.rs:133, 169`
- **Problem:** `vec![0u8; length as usize]` on every read/write: a huge guest-supplied length triggers multi-GB allocations/OOM. Plus the pointless `file_system.clone()` Arc bump (lines 134/171) and double lock (`is_open` then `read`). Lines 176-178 also needlessly write back into the guest's source buffer after `write`.
- **Fix:** cap length against a bounded reusable buffer; remove the redundant `is_open` pre-check and the write-back.

## getdents64 loses iteration state on error path

- **Location:** `nissan_connect3_emulator/src/os/syscalls/unistd.rs:404`
- **Problem:** The `not_enough_space && res.len() == 0` branch discards the pending entry list, losing readdir iteration state for subsequent calls.
- **Fix:** restore `sys_calls_state` on the EINVAL path.

## Startup flakiness: AE pump (tid77) never leaves its IO-control cycle in starved runs

~3 of 4 PROCNAV boots end with tid77 parked in a repeating `0x5036e`/`0x2016e`/`0x1026e`
(object `0x6e`) cycle instead of progressing to its `0x2d0564` mailbox wait; the AE pump
then never consumes `mbx_19` and every tid77-keyed probe goes silent (0 hits). The same
runs show tid74/75/76/78-82 parked normally, so it is one task's bring-up racing the
registry/trace IO answers, not a global crash. Suspect: the `0x5036e` (IO control) answer
semantics differ from the device when the target device is absent, sending the pump into a
retry loop. Reproduce: `run_r326.log` (all-alive but starved) vs `run_r345.log` (good).
Fixing this collapses the good-run lottery and stabilizes regression testing.

## Boot-rate lottery: DAPI cryptnav open loop stretches ~12x under host GPU load

* r711/r711b/r711c (and earlier r704) reached 340-470 s of wall time without
  ever emitting `MOUpd copy-active`: the same ~71-78 `DAPI filemgmt open`
  cryptnav-probe iterations that complete in ~150 s in lucky runs (r705-710)
  spread over ~440 s, ~27-35 s between successive open pairs, so the map
  object update (and every downstream street probe) misses the run window.
* Correlation: `nvidia-smi` showed 57 % GPU utilization by a co-tenant
  (`gb10_inference`) during the failing runs; GB10's iGPU is shared with the
  emulator's host GL, and the DAPI worker iterations block on guest GL calls
  that queue behind the co-tenant.  Nothing in the r709-r711 diffs touches
  the boot path (`eglgp`/`stubmd`/synth-shader lines are all simply never
  reached).
* Mitigations used: longer run windows (470 s), run retries.  Candidate real
  fixes: make `gpu_void_clear` round-trips cheap (avoid a host round-trip per
  trivial GL call from DAPI's worker threads - batch or answer lock-step for
  calls whose result needs no backend state), or detect the DAPI-side
  stall and re-prioritize; tracked here so a failing run gets retried
  before being read as a regression.

## r743b: hook gating changed boot timing? (RESOLVED - not timing)

Gating `add_fastmap_channel_trace_hooks` behind EMU_PROCMAP_TRACE stalled boot
(copy=0, LoadBlocks=0) in three consecutive runs; restoring it fixed boot at
first try. The cause was functional (`FASTMAP_STATUS_APPLIED` flag written by
that family's observer), not the removal of per-instruction pacing. Recorded
so future hook cleanups classify by callback side effects, not naming.

## Flaky boot: state 6 never closes (n234/n240/n241/n242, ~40% of runs)

- Symptom identical across runs: tid 65 reaches state 6 (v36 s) and sleeps
  forever; the state-6 gate walk (s9gate/g-list on tid 65, ~90 ms period)
  keeps seeing member actions state=1 until the run dies.
- Mechanism: the gate walk only POLLS [member+4]; a member flips DONE in
  bOnRequest/bNotifyWhenRunning when a leg answers it (orca1 pPop hit ->
  bOnRequest(ev=0) flips via the error-complete LAB 0x171aa50).  In BAD
  boots several late members (the re-minted registrations around v36.7)
  get NO answering leg: their (svc 0x06, txn) pushes pile up on mbx_289
  unconsumed (n241: txn 11 re-fired 60x; n242: txn 4 x18, txn 5 x24),
  only the timeout legs (ev=0x80000215/0x80000218) reach the action,
  which RESETS it for retry (bOnRequest warning path) -> never DONE.
- GOOD boots differ exactly there: every member txn gets a consumed
  ev=0x0 leg within 1-2 s (n239 bonr: all svc-6 funcs ev=0, + a tolerated
  0x8000/0x1 straggler).  The consumer that drains mbx_289 in that window
  is not pinned down yet (tid-81 rx legs vs tid-84 enConnect theft).
- Mitigations landed (r799n33h): (a) 1 Hz navi-server ServiceStatus
  re-publish while state 6 (cca_post_deferred_status, bounded 15) for the
  early-READY-edge variant; (b) the state-9 1 Hz pump-first SENSORFI kick
  + era re-fire now also run in state 6 - the unit's SENSORFI broadcast
  re-ticks the FSM for its whole life, and without a pump event nothing
  drains mbx_289 late in state 6.  (b) fixed the func-0x1 member in n242
  but not the whole set - the remaining BAD variant is still open.
- NOT the cause (checked): the state-4 sleep length (normal in n240-242);
  navsrv-svcready/vSvcReady guards (identical GOOD vs BAD); gate action
  itself (reaches DONE=2 in n241 while s7 still missing - the blockers
  are the svc-0x6 members + their parent procs 0x..11670/0x..11498).

## r799n33n: BAD-rate spiked to ~90% - environmental shift + pump-first re-fire

- Bisect result: the binary that produced GOOD boots at ~17:45 (commit
  626a0b4) now boots BAD 3/3 - the regression is NOT any of the code
  changes since; the host timing distribution shifted (exact cause
  unidentified; zombies and load checked clean).
- Every BAD since shares ONE signature: state-6 member re-mints (e.g.
  func 0x1, key 4) get their ServiceData answers pushed to mbx_289 but
  nothing ever consumes them (n241 txn11 x60, n242 txn4/5, n250 txn4
  x17, uniq_removed=3). GOOD boots consume those frames during the
  navif registration round-trip's client-side Wait - when the re-mint
  lands after that era, the queue lane is dead.
- Decoded root (r799n31 note re-read): host `guest_post` pushes never
  enter the GUEST-side message list walked by the HMINavServerLib pump
  dispatcher (0x17c1c50), and tid-81's OSAL receive (enConnect router)
  drops them. The pump lane (pump_push_completion, used by the 1 Hz
  GPS kicks) is the only drain-visible + thread-waking lane.
- Fix: refire_fi_era_pending now fires ALL pending era entries (not
  just svc 0x1f) as pump-lane data(9)+conf(8) envelopes, falling back
  to the plain mbx push. Pump completer matches the key loosely
  (n174) and the key space is single-per-era, so member keys resolve
  unambiguously. EMU_FI_ERA_REFIRE=0 disables.

## r799n33p: BAD anatomy FINAL - state 6 needs the svcdone guest-call lane

- r799n33n (pump-first re-fire for all svcs) did NOT fix the BAD rate:
  the pumped gps envelopes DO reach tid 65's dispatcher while it is
  awake, but the CL lookup misses them - the dispatcher derives the
  service from the frame (msgid/mapping), our envelope asks for
  `want_ord=0xf02/service 1-2` against a table holding (svc 0xa func
  0xf02 key 3) - `found entry=0x0` (n253_1 36185/36312).  The historic
  r799n30/n31 "pump lane delivered state-9 confs" reading was WRONG:
  those removals (lr=0x1c7d1e4, all 111 of them in n252_2) came from
  the DIRECT bHandleResponseSVCData guest-calls, not from pumped
  frames.  Pumped frames are walk-visible but completer-inert.
- Why GOOD survives and BAD does not (same code, same era): GOOD keeps
  tid-81's mbx_289 relay alive with live wire traffic; every relay
  post (`post8 lr=0x17cb6dc`, 32 in 1.9 s at n252_2 37.7 s) posts into
  the pump queue and WAKES tid 65 to drain another frame.  In BAD the
  wire is quiet after 36.3 s: tid 81 sleeps, tid 65 sleeps
  (SleepUntil, never re-woken - zero tid-65 events after 36313 in
  n253_1), and the pending member key (e.g. svc 0xa func 0xf02 key 3)
  rots in the CL table while its answers pile up unread.
- Fix: `svcdone_window()` now = {9, 6, 0-after-S9}.  In state 6 rqadd
  binds SVCDONE_PENDING and the 1 Hz window block fires the legs via
  `guest_call_bhandle_svcdone` (the ONLY delivery independent of the
  tid-65/tid-81 wake choreography; already proven for the trio keys in
  state 9).  S9_DONE latches only on a state-9 tick (a state-6 tick
  must not arm the state-0 clause - n249).  The old state-6-only
  ticker is merged into the window block.
- Pump envelopes stay (inert but harmless wake-candidates); the FI-era
  pending map is still cleared by the `remove` probe.

## r799n33r/s: state-6 wake + fan-out tuning - GOOD survives, lottery remains

- r799n33r: vSignalSelf wake gate now also covers open member-era svcdone
  re-arms (was 0x1f-only).  event.rs parks infinite OSAL event waits and
  re-arms them on wake - an event sleeper is woken ONLY by a real
  EventPost, pump-queue posts never reach it (why the 1 Hz pump kicks are
  inert for the parked enavi thread).  n257_4 shows s9-wake firing but
  tid 65 was already dead before the first wake in BAD eras, so this is
  mitigation, not cure.
- r799n33s: blind cookies {1,2,3} restricted to non-state-9 windows
  (n257_4: thrown during the cascade they flipped foreign slots, key
  0x18 removed x4 by our own legs, cascade stalled at sp2a).
- Status on the new binary: n258_1 = FULL GOOD on run 1 (state 4->9,
  cascade final, 52 keys removed, 9->0 re-walk at 47630 on worker
  tid).  n258_2/3 still BAD (state-6 death, removed=2/3) and n256 at
  EMU_REALTIME_FACTOR=0.5 is BAD 2/2 with identical signature - the
  era lottery is NOT host-clock-sensitive, it is decided inside the
  first registration rounds.  GOOD-rate now ~1/3.
- Post-cascade truth (n258_1): state-0 re-walk starts (1 key removed),
  then tid 65 idle again; the CMNAVI cl-entry for 0x8076000 is STILL
  obj+4=0 at 38 s and the popup SM never triggers (no SM-DEC after
  boot).  Confirms the r799n7 root: popup-hide + city list + CMNAVI
  readiness all wait on the CL provisioning pass (vfn+0x18 add_element
  loop, executed exactly once per run) + the ReqPrepareDestEnv write
  chain.  That is the next big chase, independent of the boot lottery.
