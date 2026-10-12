//! Read-only observation hooks for PROCNAV.OUT's amt message dispatch.
//!
//! PROCNAV is statically linked, so none of the libosal library-level traces
//! cover its internal message routing. These hooks only *read* guest memory at
//! known decode points (see docs/reverse_engineering/procnav_gps_transport.md)
//! and log what each accepted message looks like, so the wire geometry the
//! dispatcher expects can be confirmed against real traffic instead of guessed
//! from static disassembly:
//!
//! * the `rtf_tclPropertyDispatcher::s32ProcessMessage` loop compares every
//!   handler entry against `[[msg+4]+0x14]` (serviceId u16) and
//!   `[[msg+4]+0x18]` (propertyId u16),
//! * `pc_tclSensorMsgHandler::s32OnStatusMsg`'s GPS sink (ARM code!) is the
//!   end of the GPS-data path: reaching it proves an injected GPS message was
//!   deserialized and dispatched.
//!
//! The hook at the pump's field helper (`0x258b6a`) shows the reference-
//! resolution result even for messages that never reach the dispatcher.

use crate::emulator::context::Context;
use unicorn_engine::{RegisterARM, Unicorn};

const ORIGINAL_BASE: u32 = 0x0000_8000;
/// `rtf_tclPropertyDispatcher::s32ProcessMessage(amt_tclServiceData&)` (THUMB).
const PROPERTY_DISPATCHER: u32 = 0x002e_5526 - ORIGINAL_BASE;
/// `rtf2::tclMultiThreadAppThreadContext::bDispatchServiceDataMsg(amt_tclServiceData&)`.
const DISPATCH_SERVICE_DATA_MSG: u32 = 0x0005_ac30 - ORIGINAL_BASE;
/// `rtf2::tclThreadContext::vDispatchServiceDataMsg(amt_tclServiceData&)`.
const VDISPATCH_SERVICE_DATA_MSG: u32 = 0x0011_bd60 - ORIGINAL_BASE;
/// `pc_tclJobManager::s32ProcessServerAnswer(amt_tclServiceData&)`.
const PROCESS_SERVER_ANSWER: u32 = 0x002c_f240 - ORIGINAL_BASE;
/// `pc_tclThreadDispatchJobs::poConvertFIMessage(amt_tclServiceData&, u16)`.
const CONVERT_FI_MESSAGE: u32 = 0x001b_f600 - ORIGINAL_BASE;
/// The AE task's message handler entry (`vMsgHandler`).
const VMSG_HANDLER: u32 = 0x005e_8d98 - ORIGINAL_BASE;
/// `ail_tclInternalDispatch::ail_bHandleMsgServiceData` (class 0x45 handler).
const HANDLE_SERVICE_DATA: u32 = 0x00bd_50c - ORIGINAL_BASE;
/// `ail_tclAppInterfaceRestricted::bPostIpcMessage` (the class-0x45 re-post route).
const POST_IPC_MESSAGE: u32 = 0x0024_9614 - ORIGINAL_BASE;
/// `ail_tclInternalDispatch::bDispatchCCAMessages` - the AE loop's CCA router.
const CCA_DISPATCHER: u32 = 0x00be_396 - ORIGINAL_BASE;
/// `ail_tclAppInterfaceRestricted::bSendCCAPowerMsg` - builds the class-0x40
/// CCA power/state messages PROCNAV both sends and must receive to advance its
/// component state machine.
const SEND_CCA_POWER_MSG: u32 = 0x0024_9364 - ORIGINAL_BASE;
/// Field helper invoked by the AE pump after a queue accept (THUMB):
/// double-dereferences the OSAL reference-resolution answer.
const PUMP_FIELD_HELPER: u32 = 0x0025_8b6a - ORIGINAL_BASE;
/// Branch inside `ail_tclInternalDispatch::bDispatchCCAMessages` that hands a
/// class-0x40 power message to `ail_bHandleMsgPowerMessage` (taken only when the
/// dispatcher's third argument is -1 and the `[inner+0x6c]` flag is set).
const POWER_ON_INIT_BRANCH: u32 = 0x00be_6b6 - ORIGINAL_BASE;
/// Branch that re-posts an unprocessable power message (the retry spin).
const POWER_REPOST_BRANCH: u32 = 0x00be_6ca - ORIGINAL_BASE;
/// `STAT_tclApplication::bOnInit` - runs on the mode-3 (START_CONF) power message.
const STAT_ON_INIT: u32 = 0x0057_210 - ORIGINAL_BASE;
// Path probes through `bDispatchCCAMessages` (see `0xbe396`): the entry hook only
// proves the call, these mark which exit the message takes (trace branch, defer
// call, class switch, class-0x40 case, drop stub).
const PROBE_TRACE_EXIT: u32 = 0x00be_44c - ORIGINAL_BASE;
const PROBE_MID_FLOW: u32 = 0x00be_596 - ORIGINAL_BASE;
const PROBE_DEFER_CALL: u32 = 0x00be_5a0 - ORIGINAL_BASE;
const PROBE_SWITCH: u32 = 0x00be_5aa - ORIGINAL_BASE;
const PROBE_CLASS40: u32 = 0x00be_660 - ORIGINAL_BASE;
const PROBE_DROP_STUB: u32 = 0x00be_6b4 - ORIGINAL_BASE;
/// Inside `ail_bCheckExistenceRequirement` (`0xbe358`): right after the field getter
/// (`0xbe368`) - `r0` is the compared id; and right after the gate call at `0xbe68a`.
const PROBE_CHECK_GETTER: u32 = 0x00be_36e - ORIGINAL_BASE;
const PROBE_AFTER_GATE: u32 = 0x00be_68e - ORIGINAL_BASE;
/// The adapter's `vAppStateChanged` thunk (`vtable + 0x68`) invoked by the gate.
const PROBE_VT68: u32 = 0x0024_b786 - ORIGINAL_BASE;
/// After the `bOnInit` vtable call in the power handler (`0xbe088 blx`): `0xbe08a`
/// sees its return value, `0xbe094` stores the state-3 advance, `0xbe144` the failure.
const PROBE_INIT_RESULT: u32 = 0x00be_08a - ORIGINAL_BASE;
const PROBE_STATE_STORE: u32 = 0x00be_094 - ORIGINAL_BASE;
const PROBE_INIT_FAIL: u32 = 0x00be_144 - ORIGINAL_BASE;
/// `STAT_tclApplication::bOnInit` (`0x57210`) sub-step results: after the base init
/// check (`0x5722e`, must return 1), the channel factory (`0x570f4`), the components
/// (`0x5708e`) and the run step (`0x57032`) - the first non-matching value fails all.
const PROBE_ONINIT_BASE: u32 = 0x0057_232 - ORIGINAL_BASE;
const PROBE_ONINIT_FACTORY: u32 = 0x0057_23c - ORIGINAL_BASE;
const PROBE_ONINIT_COMPONENTS: u32 = 0x0057_246 - ORIGINAL_BASE;
const PROBE_ONINIT_RUN: u32 = 0x0057_250 - ORIGINAL_BASE;
/// `s32InitializeComponents` (`0x5708e`): right after the object creation call
/// (`0x570b0 bl 0x31814c`) - `r0` is the created component object or null.
const PROBE_ONINIT_CREATE: u32 = 0x0057_0b4 - ORIGINAL_BASE;
/// The nothrow `operator new` body (`0xb9a2c`): `r0` is the requested size on entry,
/// the returned pointer (or null) right after the allocator `blx` at `0xb9a38`.
const PROBE_NEW_ENTRY: u32 = 0x000b_9a2c - ORIGINAL_BASE;
const PROBE_NEW_EXIT: u32 = 0x000b_9a38 - ORIGINAL_BASE;
/// Inside `STAT_tclComponent::s32Initialize` (`0x59c30`): after `new(0x550)`
/// (`0x59c58`), after the app-interface attach (`0x59c7c`, must return 0) and after
/// the component start (`0x59caa`) - the first non-zero/`-1` decides the result.
const PROBE_STAT_ALLOC: u32 = 0x0059_c58 - ORIGINAL_BASE;
const PROBE_STAT_ATTACH: u32 = 0x0059_c7c - ORIGINAL_BASE;
const PROBE_STAT_START: u32 = 0x0059_caa - ORIGINAL_BASE;
/// `rtf::tclComponent::s32SetupThreads` (`0x199674`): after `u32CreateThreads`
/// (`0x1996a4`) and `u32InitializeThreads` (`0x1996b0`) - `r0` is the per-step
/// count, `[component+8]` the number the component expects.
const PROBE_STAT_CREATED: u32 = 0x0199_6a4 - ORIGINAL_BASE;
const PROBE_STAT_INITED: u32 = 0x0199_6b0 - ORIGINAL_BASE;
/// Decision point of `s32SetupThreads` (`0x19969a`): `r2` (0 = a config slot was
/// null), `r6` (configured thread count) and `r1` (the init-config array).
const PROBE_STAT_SETUP: u32 = 0x0199_69a - ORIGINAL_BASE;
/// `bpstl::less<unsigned short>::operator()` (`0x359528`): tid 76 faults inside a
/// map lookup during `s32InitializeChannelFactory` (READ_UNMAPPED at `0x35952a`).
const PROBE_LESS_U16: u32 = 0x0359_528 - ORIGINAL_BASE;
/// `s32InitializeChannelFactory` (`0x570f4`): after the inner-app virtual
/// (`0x57126`, channel id in `r2`) and after the factory open (`0x57140`, `r4`
/// must be 0) - the probe chain around the faulting map operation.
const PROBE_FACTORY_VIRT: u32 = 0x0057_126 - ORIGINAL_BASE;
const PROBE_FACTORY_OPEN: u32 = 0x0057_140 - ORIGINAL_BASE;
/// app-19 adapter `bOnInit` (`0x57f66`): right after the factory (`0x57f7a`),
/// components (`0x57f84`) and run (`0x57f8e`) sub-steps - `r0` must be 0 each.
/// Its state gate rejects the GPS class-69 messages while the init returns false.
const PROBE_T19_FACTORY: u32 = 0x0057_f7a - ORIGINAL_BASE;
const PROBE_T19_COMPONENTS: u32 = 0x0057_f84 - ORIGINAL_BASE;
const PROBE_T19_RUN: u32 = 0x0057_f8e - ORIGINAL_BASE;
/// Decision points inside `ail_bHandleMsgServiceData` (`0xbd50c`): after the
/// field-validity call (`0xbd56e`, zero answer drops to the trace path), after
/// the handler-scan loop (`0xbd5fa`, `r6` = match flag), the match path
/// (`0xbd6b8`), the re-post path result (`0xbd64a`) and the two drop exits
/// (`0xbd6c8` validity-fail, `0xbd7c4` lock-fail).
const PROBE_SD_UNPACK: u32 = 0x00bd_56e - ORIGINAL_BASE;
const PROBE_SD_LOOP: u32 = 0x00bd_5fa - ORIGINAL_BASE;
/// After the per-candidate `ail_tclServiceReference::bIsDataSet` (`0xbd5da`):
/// `r1`/`r3` carry the message's service id and type, `r0` the match answer.
const PROBE_SD_CMP: u32 = 0x00bd_5da - ORIGINAL_BASE;
const PROBE_SD_HIT: u32 = 0x00bd_6b8 - ORIGINAL_BASE;
const PROBE_SD_POST: u32 = 0x00bd_64a - ORIGINAL_BASE;
const PROBE_SD_OUT: u32 = 0x00bd_6c8 - ORIGINAL_BASE;
const PROBE_SD_NOLOCK: u32 = 0x00bd_7c4 - ORIGINAL_BASE;
/// On the re-post branch: after the source payload accessor (`0xbd604`, `r0`
/// must be non-null or `0x30b266` drops the built message) and after the
/// built-message validity check (`0xbd624`, `r0`).
const PROBE_SD_PAYLOAD: u32 = 0x00bd_604 - ORIGINAL_BASE;
const PROBE_SD_BUILD: u32 = 0x00bd_624 - ORIGINAL_BASE;
/// Entry of the repost message builder: `0xbd618` is the `add r0,sp,#0x10` that sets up
/// the stack wrapper (the `bl` itself starts at 0xbd61a - a hook there or at 0xbd61c
/// straddles the 4-byte branch and breaks its execution). Captures the wrapper pointer
/// for the dump at `PROBE_SD_BUILD`.
const PROBE_SD_BENTRY: u32 = 0x00bd_618 - ORIGINAL_BASE;
/// Right after the payload parse `bl 0x44a3b8` inside the builder leaf
/// `0x2b651e`: shows whether the parse zeroed the wrapper's data pointer.
const PROBE_SD_PARSE: u32 = 0x002b_6540 - ORIGINAL_BASE;
/// In `0x30b2f0`: right after the fastpool probe `blx 0x2ec890` (`r0` = its answer).
const PROBE_SD_FITS: u32 = 0x0030_b306 - ORIGINAL_BASE;
/// In `0x30b2f0`: right after the malloc `blx 0x2ec8dc` (`r0` = block pointer).
const PROBE_SD_MALLOC: u32 = 0x0030_b316 - ORIGINAL_BASE;
/// In `0x30b2f0`: the return sequence `mov r0,r5` (`r5` = 1 on success).
const PROBE_SD_FRET: u32 = 0x0030_b324 - ORIGINAL_BASE;
/// Right after `bl 0x247c70` in the repost path: `r0` is the pump object taken from
/// `adapter+0x54` that the traced send leaf `0x19089e` unwraps (`+0xc` byte must be
/// non-zero) before it issues the real `0x4b0364` post.
const PROBE_SD_PUMP: u32 = 0x00bd_638 - ORIGINAL_BASE;
/// Traced monitor-call leaf `0x19089e`: right after the first message-validity guard
/// (`beq` -> zero-return path). `r0` = unwrapped message field pointer.
const PROBE_SD_TG1: u32 = 0x0019_09a6 - ORIGINAL_BASE;
/// Same leaf: after the `blx 0x2ec28c` real service call; success iff the answer is `-1`.
const PROBE_SD_TGS: u32 = 0x0019_09c2 - ORIGINAL_BASE;
/// Same leaf: the `0x1909e2` zero-return (guard failed) - dumps the wrapper again.
const PROBE_SD_TGF: u32 = 0x0019_09e2 - ORIGINAL_BASE;
/// In the builder `0x2b7a6c` right before the 12-byte field copy (`ldm r0,{r0,r1,r2}`,
/// `r0` = copy source).
const PROBE_SD_CPY0: u32 = 0x002b_7a9e - ORIGINAL_BASE;
/// In the builder right after the copy (before `bl 0x2b651e`): dumps built+0xc..0x14.
const PROBE_SD_CPY1: u32 = 0x002b_7aaa - ORIGINAL_BASE;
/// In the builder right after `bl 0x30b2f0`: `r0` = init status, `r4` = the builder's
/// own wrapper pointer (cross-check against the entry-captured one).
const PROBE_SD_BINIT: u32 = 0x002b_7a8a - ORIGINAL_BASE;
/// GPS-data sink inside `pc_tclSensorMsgHandler::s32OnStatusMsg` (ARM).
const GPS_STATUS_SINK: u32 = 0x0068_ff8c - ORIGINAL_BASE;

fn word(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u32> {
    let mut bytes = [0u8; 4];
    unicorn
        .mem_read(address as u64, &mut bytes)
        .ok()
        .map(|_| u32::from_le_bytes(bytes))
}

fn halfword(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u16> {
    let mut bytes = [0u8; 2];
    unicorn
        .mem_read(address as u64, &mut bytes)
        .ok()
        .map(|_| u16::from_le_bytes(bytes))
}

fn read_byte(unicorn: &mut Unicorn<'_, Context>, address: u32) -> Option<u8> {
    let mut byte = [0u8; 1];
    unicorn.mem_read(address as u64, &mut byte).ok().map(|_| byte[0])
}

fn hexdump(unicorn: &mut Unicorn<'_, Context>, address: u32, len: usize) -> String {
    let mut bytes = vec![0u8; len];
    match unicorn.mem_read(address as u64, &mut bytes) {
        Ok(()) => bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
        Err(_) => "<unmapped>".to_string(),
    }
}

pub fn procnav_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let repost_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let new_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let sd_hot = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let built_ptr = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    for (name, offset, size) in [
        ("dispatcher", PROPERTY_DISPATCHER, 2u64),
        ("field-helper", PUMP_FIELD_HELPER, 2),
        ("gps-sink", GPS_STATUS_SINK, 4),
        ("dispatch-msg", DISPATCH_SERVICE_DATA_MSG, 2),
        ("vdispatch-msg", VDISPATCH_SERVICE_DATA_MSG, 2),
        ("server-answer", PROCESS_SERVER_ANSWER, 2),
        ("convert-fi", CONVERT_FI_MESSAGE, 2),
        ("vmsghandler", VMSG_HANDLER, 2),
        ("svc-data", HANDLE_SERVICE_DATA, 2),
        ("post-ipc", POST_IPC_MESSAGE, 2),
        ("cca-router", CCA_DISPATCHER, 2),
        ("power-branch", POWER_ON_INIT_BRANCH, 2),
        ("power-repost", POWER_REPOST_BRANCH, 2),
        ("stat-oninit", STAT_ON_INIT, 2),
        ("probe-trace-exit", PROBE_TRACE_EXIT, 2),
        ("probe-mid-flow", PROBE_MID_FLOW, 2),
        ("probe-defer-call", PROBE_DEFER_CALL, 2),
        ("probe-switch", PROBE_SWITCH, 2),
        ("probe-class40", PROBE_CLASS40, 2),
        ("probe-drop-stub", PROBE_DROP_STUB, 2),
        ("probe-check-getter", PROBE_CHECK_GETTER, 2),
        ("probe-after-gate", PROBE_AFTER_GATE, 2),
        ("probe-vt68", PROBE_VT68, 2),
        ("probe-init-result", PROBE_INIT_RESULT, 2),
        ("probe-state-store", PROBE_STATE_STORE, 2),
        ("probe-init-fail", PROBE_INIT_FAIL, 2),
        ("probe-oninit-base", PROBE_ONINIT_BASE, 2),
        ("probe-oninit-factory", PROBE_ONINIT_FACTORY, 2),
        ("probe-oninit-components", PROBE_ONINIT_COMPONENTS, 2),
        ("probe-oninit-run", PROBE_ONINIT_RUN, 2),
        ("probe-oninit-create", PROBE_ONINIT_CREATE, 2),
        ("probe-new-entry", PROBE_NEW_ENTRY, 2),
        ("probe-new-exit", PROBE_NEW_EXIT, 2),
        ("probe-stat-alloc", PROBE_STAT_ALLOC, 2),
        ("probe-stat-attach", PROBE_STAT_ATTACH, 2),
        ("probe-stat-start", PROBE_STAT_START, 2),
        ("probe-stat-created", PROBE_STAT_CREATED, 2),
        ("probe-stat-inited", PROBE_STAT_INITED, 2),
        ("probe-stat-setup", PROBE_STAT_SETUP, 2),
        ("probe-less-u16", PROBE_LESS_U16, 2),
        ("probe-factory-virt", PROBE_FACTORY_VIRT, 2),
        ("probe-factory-open", PROBE_FACTORY_OPEN, 2),
        ("probe-t19-factory", PROBE_T19_FACTORY, 2),
        ("probe-t19-components", PROBE_T19_COMPONENTS, 2),
        ("probe-t19-run", PROBE_T19_RUN, 2),
        ("probe-sd-unpack", PROBE_SD_UNPACK, 2),
        ("probe-sd-loop", PROBE_SD_LOOP, 2),
        ("probe-sd-cmp", PROBE_SD_CMP, 2),
        ("probe-sd-hit", PROBE_SD_HIT, 2),
        ("probe-sd-post", PROBE_SD_POST, 2),
        ("probe-sd-out", PROBE_SD_OUT, 2),
        ("probe-sd-nolock", PROBE_SD_NOLOCK, 2),
        ("probe-sd-payload", PROBE_SD_PAYLOAD, 2),
        ("probe-sd-bentry", PROBE_SD_BENTRY, 2),
        ("probe-sd-parse", PROBE_SD_PARSE, 2),
        ("probe-sd-fits", PROBE_SD_FITS, 2),
        ("probe-sd-malloc", PROBE_SD_MALLOC, 2),
        ("probe-sd-fret", PROBE_SD_FRET, 2),
        ("probe-sd-pump", PROBE_SD_PUMP, 2),
        ("probe-sd-tg1", PROBE_SD_TG1, 2),
        ("probe-sd-tgs", PROBE_SD_TGS, 2),
        ("probe-sd-tgf", PROBE_SD_TGF, 2),
        ("probe-sd-cpy0", PROBE_SD_CPY0, 2),
        ("probe-sd-cpy1", PROBE_SD_CPY1, 2),
        ("probe-sd-binit", PROBE_SD_BINIT, 2),
        ("probe-sd-build", PROBE_SD_BUILD, 2),
    ] {
        let address = (base_address + offset) as u64;
        let repost_count = repost_count.clone();
        let new_count = new_count.clone();
        let sd_hot = sd_hot.clone();
        let built_ptr = built_ptr.clone();
        let result = unicorn.add_code_hook(address, address + size, move |uc, _, _| {
            let tid = uc.get_data().inner.thread_id();
            if name == "power-repost" || (name.starts_with("probe-") && name != "probe-less-u16") {
                let (counter, limit) = if name.starts_with("probe-new-") {
                    (&new_count, 200)
                } else if matches!(
                    name,
                    "probe-sd-unpack"
                        | "probe-sd-payload"
                        | "probe-sd-parse"
                        | "probe-sd-fits"
                        | "probe-sd-malloc"
                        | "probe-sd-fret"
                ) {
                    (&sd_hot, 420)
                } else {
                    (&repost_count, 600)
                };
                if counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= limit {
                    return; // dispatch retries and startup allocations flood the log
                }
            }
            match name {
                "dispatcher" => {
                    let table = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let fields = word(uc, msg.wrapping_add(4));
                    let (svc, prop) = match fields {
                        Some(base) => (
                            halfword(uc, base.wrapping_add(0x14)),
                            halfword(uc, base.wrapping_add(0x18)),
                        ),
                        None => (None, None),
                    };
                    let svc_hex = svc.map(|v| format!("{v:#x}"));
                    let prop_hex = prop.map(|v| format!("{v:#x}"));
                    log::info!(
                        "PROCNAV[{tid}] dispatcher msg={msg:#010x} fields={fields:?} svc={svc_hex:?} prop={prop_hex:?} table={table:#010x} head=[{}]",
                        hexdump(uc, msg, 0x10),
                    );
                }
                "field-helper" => {
                    let data = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let inner = word(uc, data.wrapping_add(4));
                    log::info!(
                        "PROCNAV[{tid}] field helper data={data:#010x} +4={inner:?} head=[{}]",
                        hexdump(uc, data, 0x18),
                    );
                }
                "dispatch-msg" | "vdispatch-msg" | "server-answer" | "convert-fi"
                | "vmsghandler" => {
                    let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let fields = word(uc, obj.wrapping_add(4));
                    let (svc, prop) = match fields {
                        Some(base) if base != 0 => (
                            halfword(uc, base.wrapping_add(0x14)),
                            halfword(uc, base.wrapping_add(0x18)),
                        ),
                        _ => (None, None),
                    };
                    let svc_hex = svc.map(|v| format!("{v:#x}"));
                    let prop_hex = prop.map(|v| format!("{v:#x}"));
                    log::info!(
                        "PROCNAV[{tid}] {name}: obj={obj:#010x} fields={fields:?} svc={svc_hex:?} prop={prop_hex:?} head=[{}]",
                        hexdump(uc, obj, 0x18),
                    );
                }
                "cca-router" => {
                    let adapter = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let arg2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    let inner = word(uc, adapter).unwrap_or(0);
                    let state = read_byte(uc, inner.wrapping_add(0x30));
                    let app = halfword(uc, inner.wrapping_add(0x10));
                    let vt = word(uc, inner).unwrap_or(0);
                    let gate = word(uc, vt.wrapping_add(0x30)).unwrap_or(0);
                    let defer_flag = read_byte(uc, inner.wrapping_add(0x6c));
                    let content = word(uc, msg.wrapping_add(4)).unwrap_or(0);
                    let class = read_byte(uc, content.wrapping_add(0xb));
                    let svc = halfword(uc, content.wrapping_add(0x14));
                    log::info!(
                        "PROCNAV[{tid}] cca-router: adapter={adapter:#010x} inner={inner:#010x} vt={vt:#010x} gate={gate:#010x} state={state:?} app={app:?} msg={msg:#010x} class={class:?} svc={svc:?} arg2={arg2:#010x} lr={lr:#010x} defer={defer_flag:?}"
                    );
                }
                "power-branch" => {
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] power message taken for processing (lr={lr:#010x})");
                }
                "power-repost" => {
                    let msg = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    let content = word(uc, msg.wrapping_add(4)).unwrap_or(0);
                    let class = read_byte(uc, content.wrapping_add(0xb));
                    log::info!(
                        "PROCNAV[{tid}] power message reposted: msg={msg:#010x} class={class:?}"
                    );
                }
                "stat-oninit" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] STAT bOnInit entered: object={r0:#010x}");
                }
                "probe-trace-exit"
                | "probe-mid-flow"
                | "probe-defer-call"
                | "probe-switch"
                | "probe-class40"
                | "probe-drop-stub" => {
                    let msg = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    let content = word(uc, msg.wrapping_add(4)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] {name}: msg={msg:#010x} content={content:#010x} src={:?} dst={:?} mode={:?}",
                        halfword(uc, content),
                        halfword(uc, content.wrapping_add(2)),
                        halfword(uc, content.wrapping_add(0x14)),
                    );
                }
                "probe-check-getter" => {
                    let getter_result = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let own = word(uc, 0x9022_6f90).and_then(|inner| halfword(uc, inner + 0x10));
                    log::info!(
                        "PROCNAV[{tid}] probe-check-getter: r0={getter_result:#x} own={own:?}"
                    );
                }
                "probe-after-gate" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-after-gate: gate result r0={r0:#x}");
                }
                "probe-vt68" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] probe-vt68: vAppStateChanged object={r0:#010x}");
                }
                "probe-init-result" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-init-result: bOnInit returned {r0:#x}");
                }
                "probe-state-store" => {
                    log::info!("PROCNAV[{tid}] probe-state-store: writing state 3 (ACTIVE)");
                }
                "probe-init-fail" => {
                    log::info!("PROCNAV[{tid}] probe-init-fail: bOnInit failed");
                }
                "probe-oninit-base"
                | "probe-oninit-factory"
                | "probe-oninit-components"
                | "probe-oninit-run"
                | "probe-t19-factory"
                | "probe-t19-components"
                | "probe-t19-run" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) & !1;
                    log::info!("PROCNAV[{tid}] {name}: returned {r0:#x} lr={lr:#010x}");
                }
                "probe-oninit-create" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-oninit-create: new(0x30) -> {r0:#010x}");
                }
                "probe-new-entry" => {
                    let size = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    if size >= 0x20 {
                        log::info!(
                            "PROCNAV[{tid}] probe-new-entry: allocation of {size:#x} bytes"
                        );
                    }
                }
                "probe-new-exit" => {
                    let ptr = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    if ptr == 0 {
                        log::info!("PROCNAV[{tid}] probe-new-exit: allocation FAILED (null)");
                    }
                }
                "probe-stat-alloc"
                | "probe-stat-attach"
                | "probe-stat-start" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] {name}: result {r0:#010x}");
                }
                "probe-stat-created" | "probe-stat-inited" => {
                    let component = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let got = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let expected = word(uc, component.wrapping_add(8)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] {name}: component={component:#010x} got {got} expected {expected}"
                    );
                }
                "probe-stat-setup" => {
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0);
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    log::info!(
                        "PROCNAV[{tid}] probe-stat-setup: flag={r2:#x} count={r6} config={r1:#010x} entry0={:?} entry1={:?}",
                        word(uc, r1),
                        word(uc, r1.wrapping_add(0x14)),
                    );
                }
                "probe-less-u16" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    // Only interesting when the key pointer is unmapped: log exactly
                    // the calls that would fault two instructions later. (`r1` is a
                    // plain value at the hot `0x11423c` caller, so check `r0` only.)
                    if word(uc, r0 & !3).is_none() {
                        log::info!(
                            "PROCNAV[{tid}] probe-less-u16: UNMAPPED key {r0:#010x} lr={:#010x}",
                            lr & !1
                        );
                    }
                }
                "probe-sd-unpack" | "probe-sd-out" | "probe-sd-nolock" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] {name}: r0={r0:#010x}");
                }
                "probe-sd-loop" => {
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0);
                    // r4 = the service-reference container, r6 = match count so far.
                    // The candidate chain head lives in the adapter vtable slot 0x50;
                    // nodes are {u16 svc, u16 type, node* next} starting at head->next.
                    let vt = word(uc, r4).unwrap_or(0);
                    let head = word(uc, vt.wrapping_add(0x50)).unwrap_or(0);
                    let mut nodes = Vec::new();
                    let mut cur = word(uc, head.wrapping_add(4)).unwrap_or(0);
                    for _ in 0..8 {
                        if cur == 0 {
                            break;
                        }
                        let pair = word(uc, cur).unwrap_or(0xDEAD);
                        nodes.push(format!("{pair:#010x}@{cur:#08x}"));
                        cur = word(uc, cur.wrapping_add(4)).unwrap_or(0);
                    }
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-loop: this={r4:#010x} matched={r6} vt={vt:#010x} head={head:#010x} chain={nodes:?}"
                    );
                }
                "probe-sd-hit" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-hit: handler={r0:#010x}");
                }
                "probe-sd-cmp" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0);
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-cmp: matched={r0:#x} svc={r1:#x} a2={r2:#x} a3={r3:#x}"
                    );
                }
                "probe-sd-post" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-post: repost-status={r0:#x}");
                    built_ptr.store(0, std::sync::atomic::Ordering::Relaxed);
                }
                "probe-sd-payload" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-payload: payload={r0:#010x} parsed={}",
                        hexdump(uc, sp.wrapping_add(0x54), 0x20)
                    );
                }
                "probe-sd-build" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-build: built={r0:#x} wrapper={wrapper:#x} body={}",
                        hexdump(uc, wrapper, 0x20)
                    );
                    let _ = sp;
                }
                "probe-sd-bentry" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    built_ptr.store(r0, std::sync::atomic::Ordering::Relaxed);
                    log::info!("PROCNAV[{tid}] probe-sd-bentry: wrapper={r0:#x} sp={sp:#x}");
                }
                "probe-sd-binit" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    if wrapper != 0 {
                        log::info!(
                            "PROCNAV[{tid}] probe-sd-binit: init-status={r0:#x} r4={r4:#x} entry-wrapper={wrapper:#x}"
                        );
                    }
                }
                "probe-sd-parse" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-parse: r0={r0:#x} wrapper_body={}",
                        hexdump(uc, wrapper, 0x20)
                    );
                }
                "probe-sd-fits" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-fits: answer={r0:#x}");
                }
                "probe-sd-malloc" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-malloc: block={r0:#x}");
                }
                "probe-sd-fret" => {
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-fret: r5={r5:#x}");
                }
                "probe-sd-pump" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-pump: object={r0:#010x} body={}",
                        hexdump(uc, r0, 0x18)
                    );
                }
                "probe-sd-tg1" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    if wrapper != 0 && r0.wrapping_sub(wrapper) < 0x400 {
                        log::info!(
                            "PROCNAV[{tid}] probe-sd-tg1: unwrapped={r0:#010x} byte={:02x}",
                            word(uc, r0).unwrap_or(0) & 0xff
                        );
                    }
                }
                "probe-sd-tgs" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-tgs: svc-answer={r0:#x}");
                }
                "probe-sd-tgf" => {
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    if wrapper != 0 {
                        log::info!(
                            "PROCNAV[{tid}] probe-sd-tgf: guard-failed wrapper={wrapper:#x} body={}",
                            hexdump(uc, wrapper, 0x18)
                        );
                    }
                }
                "probe-sd-cpy0" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    if r0 > 0x8000_0000 && wrapper > 0x8000_0000 {
                        log::info!(
                            "PROCNAV[{tid}] probe-sd-cpy0: src={r0:#010x} src_body={} wrapper_body={}",
                            hexdump(uc, r0, 0x18),
                            hexdump(uc, wrapper, 0x18)
                        );
                    }
                }
                "probe-sd-cpy1" => {
                    let wrapper = built_ptr.load(std::sync::atomic::Ordering::Relaxed);
                    if wrapper > 0x8000_0000 {
                        log::info!(
                            "PROCNAV[{tid}] probe-sd-cpy1: wrapper={wrapper:#x} body={}",
                            hexdump(uc, wrapper, 0x18)
                        );
                    }
                }
                "probe-factory-virt" | "probe-factory-open" | "probe-factory-add" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] {name}: r0={r0:#010x} r2={r2:#x}");
                }
                "svc-data" => {
                    // `this` is the adapter (r0); the message arrives in r1.
                    let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let fields = word(uc, msg.wrapping_add(4));
                    let (svc, prop, opcode) = match fields {
                        Some(base) if base != 0 => (
                            halfword(uc, base.wrapping_add(0x14)),
                            halfword(uc, base.wrapping_add(0x18)),
                            word(uc, base.wrapping_add(0x18)).map(|v| v >> 16 & 0xff),
                        ),
                        _ => (None, None, None),
                    };
                    log::info!(
                        "PROCNAV[{tid}] svc-data: this={this:#010x} msg={msg:#010x} fields={fields:?} svc={svc:?} prop={prop:?} opcode={opcode:?}",
                    );
                }
                "post-ipc" => {
                    let app = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32 & 0xffff;
                    let msg = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let fields = word(uc, msg.wrapping_add(4));
                    let svc = fields.filter(|v| *v != 0).and_then(|base| halfword(uc, base.wrapping_add(0x14)));
                    log::info!(
                        "PROCNAV[{tid}] post-ipc: app={app:#x} msg={msg:#010x} svc={svc:?}"
                    );
                }
                "gps-sink" => {
                    let sp = uc.reg_read(RegisterARM::R13).unwrap_or(0) as u32;
                    let msg = word(uc, sp.wrapping_add(0xf0)).unwrap_or(0);
                    let svc = halfword(uc, msg.wrapping_add(0x14));
                    let prop = halfword(uc, msg.wrapping_add(0x16));
                    log::info!(
                        "PROCNAV[{tid}] GPS status sink hit: r0={:#010x} msg={msg:#010x} svc={:?} prop={:?} head=[{}]",
                        uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32,
                        svc.map(|v| format!("{v:#x}")),
                        prop.map(|v| format!("{v:#x}")),
                        hexdump(uc, msg, 0x20),
                    );
                }
                _ => {}
            }
        });
        match result {
            Ok(_) => log::info!("PROCNAV: observation hook '{name}' at {address:#x}"),
            Err(error) => log::warn!("PROCNAV: cannot hook {name} at {address:#x}: {error}"),
        }
    }

    if std::env::var("EMU_SENSORFI")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        spawn_gps_provider(unicorn);
    }
}

/// Push periodic `cmb_sensorfi_tclMsgGpsDataStatus` messages into PROCNAV's
/// application mailbox (`mbx_19`), acting as the sensorfi server GPS provider.
///
/// The blob is pre-snapshotted (`[u32 flag, u32 len, content]`) so the monitor's
/// `0x2d` queue-wait materializes it into PROCNAV's own VM, and the `0x57`
/// reference resolves to the content base exactly like a Linux-side poster's.
/// The content's amt header uses the offsets the `amt_tclBaseMessage` accessors
/// read (`u16GetValue`: serviceId @ +0x14, propertyId @ +0x18, opCode @ +0x1a,
/// class @ +0xb), pinned by `cmb_sensorfi_tclMsgGpsDataStatus`'s getters:
/// service `0xb`, property `5`, opcode `6` (Status). The `cmb_fi_tcl_GpsBigdata`
/// payload stays zero until its wire layout is decoded - reaching the GPS sink
/// is the structural milestone this provider tests for.
fn spawn_gps_provider(unicorn: &mut Unicorn<'_, Context>) {
    let namespace = unicorn.get_data().namespace.clone();
    let result = std::thread::Builder::new()
        .name("gps-provider".into())
        .spawn(move || {
            // Let FC bring-up finish (component threads, queue opens, handler
            // registration) before the first push.
            std::thread::sleep(std::time::Duration::from_secs(10));
            let mut seq: u16 = 0;
            loop {
                seq = seq.wrapping_add(1);
                let blob = gps_status_message(seq);
                let posted = {
                    let mut guard = namespace.lock().unwrap();
                    crate::common::osal_queues::OsalQueueService::ensure_queue(
                        &mut guard.mq,
                        "mbx_19",
                    );
                    let posted = crate::common::osal_queues::OsalQueueService::guest_post(
                        &mut guard.mq,
                        "mbx_19",
                        blob,
                        0,
                    );
                    if posted {
                        guard.notify_waiters();
                    }
                    posted
                };
                if posted {
                    log::info!("GPS provider: posted GpsDataStatus #{seq} to mbx_19");
                } else {
                    log::debug!("GPS provider: mbx_19 full, skipping #{seq}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    match result {
        Ok(_) => log::info!("PROCNAV: GPS sensorfi provider started (EMU_SENSORFI)"),
        Err(error) => log::warn!("PROCNAV: cannot start GPS provider: {error}"),
    }
}

/// One `GpsDataStatus` snapshot blob: envelope + amt header, zero payload.
fn gps_status_message(seq: u16) -> Vec<u8> {
    const LEN: usize = 0x80;
    let mut content = vec![0u8; LEN];
    content[0x00..0x02].copy_from_slice(&0xfffeu16.to_le_bytes()); // source app (unknown provider)
    content[0x02..0x04].copy_from_slice(&0x0013u16.to_le_bytes()); // target app: PROCNAV (19)
    content[0x04..0x08].copy_from_slice(&(LEN as u32).to_le_bytes()); // snapshot/object length
    // amt message class (read as u8GetValue(0xb) by ail_tclInternalDispatch::
    // bDispatchCCAMessages): 0x45 = ServiceData. 0x40 = state/LPM, 0x41 =
    // ServiceRegister, 0x42 = Unregister, 0x43 = RegisterConf, 0xff = DebugApp -
    // class 0x41 mis-injected r214-r216 as a (rejected) ServiceRegister.
    content[0x0b] = 0x45;
    content[0x0c..0x0e].copy_from_slice(&0xfffeu16.to_le_bytes()); // no source sub-id
    content[0x0e..0x10].copy_from_slice(&0xfffeu16.to_le_bytes()); // no target sub-id
    content[0x14..0x16].copy_from_slice(&0x000bu16.to_le_bytes()); // serviceId: sensorfi
    content[0x18..0x1a].copy_from_slice(&0x0005u16.to_le_bytes()); // propertyId: GPS data
    content[0x1a] = 0x06; // opCode: Status (server message)
    content[0x1c..0x1e].copy_from_slice(&seq.to_le_bytes()); // cmd counter
    let mut blob = Vec::with_capacity(8 + LEN);
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(LEN as u32).to_le_bytes());
    blob.extend_from_slice(&content);
    blob
}