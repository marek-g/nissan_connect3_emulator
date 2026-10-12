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
// Generic intrusive-container erase helper (Thumb, ~50 call sites).  Faulting
// runs die in its tail (`strb r6,[r0]` at 0x5a43f1 with `[slot]==NULL`); this
// entry probe fires only when that fatal precondition holds, logging the
// caller (lr) so the offending container can be identified.
const PROBE_LIST_ERASE: u32 = 0x005a_4360 - ORIGINAL_BASE;
/// Unicorn 2.1.5 silently never fires some narrow-range `UC_HOOK_CODE` hooks
/// whose target sits inside a large Thumb translation block: r372-r376 show
/// `0x2ebd6c`/`0x2989c4` hooks with zero hits while page-range hooks over the
/// very pages logged every fetch of those exact addresses (registration
/// succeeded for all, the closure type is shared, and swapping the startup
/// order changed nothing - the quirk is inside the engine, not our side).
/// The `xp-watch` machinery below is the workaround: whole pages are hooked
/// and dispatched by exact fetch address in the callback.
const XP_TARGETS: &[(u32, &str)] = &[
    (0x0029_7d10, "ail-open"),
    (0x0029_7d32, "t1-dispatch"),
    (0x0029_7f54, "interpret"),
    (0x0029_81e6, "getnext-entry"),
    (0x0029_8210, "getnext-check"),
    (0x0029_84b4, "getnext-errbl"),
    (0x0029_89c4, "rde-formatter"),
    (0x002c_4780, "traceout"),
    (0x002e_bd6c, "veneer"),
    (0x002e_5526, "dispatcher"),
    (0x0030_b98a, "err-set"),
    (0x0068_ff8c, "sink"),
    (0x0011_dbd8, "ventry-entry"),
    (0x0011_dc56, "ventry-step"),
];

/// The pages covering `XP_TARGETS`, one hook each (see the note above).
const XP_PAGES: &[(u32, u32)] = &[
    (0x0011_d000, 0x1000),
    (0x0029_7000, 0x2000),
    (0x002c_4000, 0x1000),
    (0x002e_5000, 0x1000),
    (0x002e_b000, 0x1000),
    (0x0030_b000, 0x1000),
    (0x0068_f000, 0x1000),
];

/// Control hook proving the observation framework itself is alive: the
/// shared svc thunk (`0x62ef90`, retired by every monitor call).
const PROBE_CTL_THUNK: u32 = 0x0062_ef90 - ORIGINAL_BASE;
/// In `ail_tclInternalDispatch::bDispatchCCAMessages` (`0xbd50c`), right on the
/// `bl 0x24ea05` (`bIsDataSet`) whose answer gates the delivery `blx r2`: dumps
/// the match key (`r0`, a 12-byte `{u16@0,2,4, u8@6, u16@8}` service-reference
/// payload copy) and the four message-side comparators (`r1` = `[sp+0x6c]`,
/// `r2` = `r7`, `r3` = `[sp+0x68]`, `[sp]` = `[sp+0x3c]` from the prologue) so
/// a fabricated reference entry can be pinned to the exact tuple the STAT pump
/// compares against.
const PROBE_DS_TUPLE: u32 = 0x00bd_5d6 - ORIGINAL_BASE;
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
/// `pc_tclProcessMessage::s32ProcessMessage` entry: the only caller chain that
/// reaches the property dispatcher (`0x2e5526`) and its registered sinks.
const PROBE_SD_PROCMSG: u32 = 0x004d_3c38 - ORIGINAL_BASE;
/// CCA router's tail vtable dispatch (`0xbe448`, `ip = [r4]`, callee `[ip+0x14]`):
/// dumps the resolved callee - the class-0x45 route's real handler entry.
const PROBE_SD_RTDISP: u32 = 0x00be_44a - ORIGINAL_BASE;
/// Property dispatcher entry (`0x2e5526`): walks the live registration list at
/// `[arg0+0xc]` and calls `[[node]+8]` on a prop/service match - the only road
/// to the sinks in the static table at `0xa38d78` (incl. `0x68ff8c`).
const PROBE_DISPATCHER: u32 = 0x002e_5526 - ORIGINAL_BASE;
/// Registrar `0x2e6824` right after the object probe `bl 0x2e65cc`: `r0 == 1`
/// means the vtable `[obj]+8` acquire answered and the record insert at
/// `0x2e6684` runs; anything else silently drops the registration.
const PROBE_REGPROBE: u32 = 0x002e_6838 - ORIGINAL_BASE;
/// `bHandleMsgServiceData` gate after `bl 0x2b7624` (`0xbd56e`): `r0` is the
/// opcode acceptor's answer - 0 drops the message before the ownership walk.
const PROBE_SD_GATE: u32 = 0x00bd_56e - ORIGINAL_BASE;
/// Registrar clone `0x2e6948` right after `ldr r0,[pc,#0x138]`/`adds r0,#8`
/// (`0x2e695e`): dumps what the 'registry object' pointer really resolves to.
const PROBE_REGOBJ: u32 = 0x002e_695e - ORIGINAL_BASE;
/// `rtf_tclChannelFactory::pGetCommMemberInfo` return (`0x6558e`): `r0=0` with a
/// non-null `[factory+0x4c]` list means the service's comm-member record is
/// missing from the boot directory - the STAT sensor channel open dies here.
const PROBE_COMMFIND: u32 = 0x0006_558e - ORIGINAL_BASE;
/// `rtf_tclChannelFactory::vSetCommMemberList` (`0x6563e`): who seeds the
/// comm-member list (`r1` = list, `r2` = owner) and from where (LR).
const PROBE_COMMSET: u32 = 0x0006_563e - ORIGINAL_BASE;
/// STAT member-open `0x1f6d26` step 1: just after `blx` on the found record's
/// member object `[rec+0x20]->vtable[0x18]` (`ip` = member id, `r7` = old size).
const PROBE_MEMOPEN1: u32 = 0x001f_6d68 - ORIGINAL_BASE;
/// STAT member-open step 2: the record-vector slot write after the growth call
/// (`0x1f6da0`): dumps the resolved member id and the destination vector.
const PROBE_MEMOPEN2: u32 = 0x001f_6da0 - ORIGINAL_BASE;
/// Shared return path of the method-table registrar family (`0x2e6904`/`0x2e6948`
/// and clones all tail-branch here after `bl 0x2e6824`): `r0` = register answer.
const PROBE_REGISTRAR: u32 = 0x002e_6874 - ORIGINAL_BASE;
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
/// Server-message scan in `ail_bHandleMsgServiceData`: the `bl 0x24ea04` candidate
/// comparator call site (`0xbd5d6`, THUMB). `r0` = iterator, candidate node =
/// `[r0+8]`, payload at `node+8` = `{u16@0, u16@2, u16@4, u8 state@6, u16@8}`;
/// `r1`/`r2`/`r3` carry the message tuple (svc candidate key, src app, reg id).
const PROBE_SD_REFCMP: u32 = 0x00bd_5d6 - ORIGINAL_BASE;
/// Client-message scan: the `bl 0x24ee68` registry comparator call site (`0xbd6f6`).
/// Node payload = `{u16 regid, u16 svc, u16 srcapp, u16 srcsub, u8 state@8}`.
const PROBE_SD_REGCMP: u32 = 0x00bd_6f6 - ORIGINAL_BASE;
/// After the server-message reference scan (`0xbd5fa`): `r6` = match flag -
/// nonzero means the subscription list matched and delivery will be attempted.
const PROBE_SD_REFRES: u32 = 0x00bd_5fa - ORIGINAL_BASE;
/// After the client-message registry scan (`0xbd758`): `r6` = match flag.
const PROBE_SD_REGRES: u32 = 0x00bd_758 - ORIGINAL_BASE;
/// Delivery branch decision (`0xbd87e`, `cmp r5,#0`): `r5 != 0` takes the road
/// to the app's message pump through the `vtable[0x20]` call.
const PROBE_SD_DLVCHK: u32 = 0x00bd_87e - ORIGINAL_BASE;
/// The actual delivery `blx r2` (`0xbd8b0`): `r0` = inner ail object, `r1` =
/// message wrapper, `r2` = the app's pump entry (`vtable+0x20`).
const PROBE_SD_DELIVER: u32 = 0x00bd_8b0 - ORIGINAL_BASE;
/// `ail_tclAppInterfaceRestricted::u16RegisterService` entry (`0x24a39a`): the
/// client-side service subscription call - blocks on a semaphore until the
/// provider app answers with a ServiceRegisterConf.
const PROBE_REGSVC: u32 = 0x0024_a39a - ORIGINAL_BASE;
/// `scd_bGetAppIdFromServiceId` entry (`0x34f012`): the service-config database
/// lookup that resolves a service id to its provider app (fails => 0).
const PROBE_SCD: u32 = 0x0034_f012 - ORIGINAL_BASE;
/// `ail_bHandleMsgServiceRegister` entry (`0xbdc36`): provider-side handler of
/// incoming subscription requests (inserts into the registry list at `[obj+0x58]`).
const PROBE_MSGREG: u32 = 0x000b_dc36 - ORIGINAL_BASE;
/// `ail_bHandleMsgServiceRegisterConf` entry (`0xbd918`): client-side handler of
/// subscription answers - on state 1 it inserts the ServiceReference at `[obj+0x50]`
/// that the server-message scan above matches against.
const PROBE_MSGCONF: u32 = 0x000b_d918 - ORIGINAL_BASE;

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

fn xp_cstr(unicorn: &mut Unicorn<'_, Context>, address: u32) -> String {
    let mut bytes = [0u8; 48];
    match unicorn.mem_read(address as u64, &mut bytes) {
        Ok(()) => {
            let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        }
        Err(_) => "<unmapped>".to_string(),
    }
}

/// Exact-address dispatch for the page-granular `xp-watch` hooks (see
/// `XP_TARGETS` for why narrow hooks cannot be trusted at these VAs).
fn xp_watch(unicorn: &mut Unicorn<'_, Context>, tid: u32, address: u64) {
    let Some((_, tag)) = XP_TARGETS.iter().find(|(a, _)| *a as u64 == address) else {
        return;
    };
    // Per-tag budget so a hot target cannot starve the others; the
    // self-deduplicating sites (unique `lr` reporting) are exempt.
    static BUDGET: std::sync::Mutex<Vec<(&'static str, u32)>> =
        std::sync::Mutex::new(Vec::new());
    if !matches!(*tag, "err-set" | "traceout") {
        let mut budget = BUDGET.lock().unwrap();
        match budget.iter_mut().find(|(t, _)| *t == *tag) {
            Some(entry) => {
                entry.1 += 1;
                if entry.1 > 16 {
                    return;
                }
            }
            None => budget.push((tag, 1)),
        }
    }
    let r0 = unicorn.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
    let r1 = unicorn.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
    let r2 = unicorn.reg_read(RegisterARM::R2).unwrap_or(0);
    let r3 = unicorn.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
    let lr = (unicorn.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1;
    let sp = unicorn.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
    match *tag {
        "err-set" => {
            // err_vSetError(handler, code, 0, message, line, file): log every
            // unique (code, lr) once - this names each distinct RDE emitter.
            static SITES: std::sync::Mutex<Vec<(u32, u32)>> =
                std::sync::Mutex::new(Vec::new());
            let line = word(unicorn, sp).unwrap_or(0);
            let msg = xp_cstr(unicorn, r3);
            let mut sites = SITES.lock().unwrap();
            if !sites.contains(&(r1, lr)) && sites.len() < 48 {
                sites.push((r1, lr));
                log::info!(
                    "PROCNAV[{tid}] xp-err-set: code={:#x} lr={lr:#010x} line={line} msg={msg:?}",
                    r1
                );
            }
        }
        "traceout" => {
            let record = r1 as u32;
            let text_ptr = word(unicorn, record.wrapping_add(0xc)).unwrap_or(0);
            let text = xp_cstr(unicorn, text_ptr);
            if text.contains("Code=0x") || text.contains("MsgMan") || text.contains("Unknown") {
                log::info!(
                    "PROCNAV[{tid}] xp-traceout: handle={r0:#010x} sev={:#x} lr={lr:#010x} text={text:?}",
                    word(unicorn, record.wrapping_add(4)).unwrap_or(0)
                );
            }
        }
        "interpret" => {
            // The vector entry is `{kind, osal-msg, size}`; the OSAL message
            // at `[entry+4]` is `{kind, content, size}` and `bEnableAccess`
            // maps `[msg+4]`, so dump the content block too - byte `0xb` is
            // `u8GetType`'s source (`u8GetValue(0xb)` at `0x36c7d2`) and must
            // be `0x40..=0x45` or the switch defaults to "Unknown type".
            let p1 = word(unicorn, (r1 as u32).wrapping_add(4)).unwrap_or(0);
            let content = word(unicorn, p1.wrapping_add(4)).unwrap_or(0);
            log::info!(
                "PROCNAV[{tid}] xp-interpret: mgr={r0:#010x} entry=[{}] req=[{r2:#x} {r3:#x} {} {}] body@{p1:#010x}=[{}] content@{content:#010x}=[{}]",
                hexdump(unicorn, r1 as u32, 0x10),
                word(unicorn, sp).unwrap_or(0),
                word(unicorn, sp.wrapping_add(4)).unwrap_or(0),
                hexdump(unicorn, p1, 0x10),
                hexdump(unicorn, content, 0x20),
            );
        }
        "dispatcher" => {
            let table = r0;
            let msg = r1 as u32;
            let fields = word(unicorn, msg.wrapping_add(4)).unwrap_or(0);
            log::info!(
                "PROCNAV[{tid}] xp-dispatcher: table={table:#010x} msg={msg:#010x} fields={fields:#010x} svc={:#x} prop={:#x} head=[{}]",
                halfword(unicorn, fields.wrapping_add(0x14)).unwrap_or(0),
                halfword(unicorn, fields.wrapping_add(0x18)).unwrap_or(0),
                hexdump(unicorn, msg, 0x10),
            );
        }
        "sink" => {
            // `pc_tclSensorMsgHandler::s32OnStatusMsg` (ARM): the message is
            // `[sp+0xf0]` per the `gps-sink` narrow-hook model.
            let msg = word(unicorn, sp.wrapping_add(0xf0)).unwrap_or(0);
            log::info!(
                "PROCNAV[{tid}] xp-sink: r0={r0:#010x} msg={msg:#010x} svc={:#x} prop={:#x} head=[{}]",
                halfword(unicorn, msg.wrapping_add(0x14)).unwrap_or(0),
                halfword(unicorn, msg.wrapping_add(0x16)).unwrap_or(0),
                hexdump(unicorn, msg, 0x20),
            );
        }
        "getnext-entry" => {
            // s32GetNextMessage(this, out, r2, tuple r3 + five stack words):
            // manager channel/vector window plus the six-word request tuple.
            log::info!(
                "PROCNAV[{tid}] xp-getnext: mgr={r0:#010x} mgr=[{}] tuple=[{}] extra=[{}]",
                hexdump(unicorn, r0, 0x38),
                hexdump(unicorn, sp, 0x18),
                hexdump(unicorn, sp.wrapping_add(0x18), 0x10),
            );
        }
        "getnext-check" => {
            log::info!("PROCNAV[{tid}] xp-getnext-check: bCheckParameters={r0:#x}");
        }
        "getnext-errbl" => {
            log::info!(
                "PROCNAV[{tid}] xp-getnext-errbl: r0={r0:#010x} sp=[{}]",
                hexdump(unicorn, sp, 0x10)
            );
        }
        "t1-dispatch" => {
            let r4 = unicorn.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let svc = word(unicorn, r4.wrapping_add(0x24)).unwrap_or(0);
            let svc_vt = word(unicorn, svc).unwrap_or(0);
            log::info!(
                "PROCNAV[{tid}] xp-t1-dispatch: service={svc:#010x} r1={r1:#x} r2={r2:#x} r3={r3:#x} sp=[{}] wrapper@{r4:#010x} vt={svc_vt:#010x} method={:#010x}",
                hexdump(unicorn, sp, 8),
                word(unicorn, svc_vt.wrapping_add(0x20)).unwrap_or(0),
            );
        }
        "ventry-entry" => {
            log::info!("PROCNAV[{tid}] xp-ventry-entry: this={r0:#010x} arg={r1:#010x} lr={lr:#010x}");
        }
        "ventry-step" => {
            let r4 = unicorn.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
            let map = word(unicorn, r4.wrapping_add(0x3c)).unwrap_or(0);
            log::info!(
                "PROCNAV[{tid}] xp-ventry-step: this={r4:#010x} vt={:#010x} method={r1:#010x} map@{map:#010x} map=[{}]",
                word(unicorn, r4).unwrap_or(0),
                hexdump(unicorn, map.wrapping_add(0x18), 0x18),
            );
        }
        "rde-formatter" => {
            let s4 = word(unicorn, sp.wrapping_add(4)).unwrap_or(0);
            let s14 = word(unicorn, sp.wrapping_add(0x14)).unwrap_or(0);
            log::info!(
                "PROCNAV[{tid}] xp-rde-formatter: sp={sp:#010x} sp=[{}] s4={s4:#010x}:{:?} s14={s14:#010x}:{:?}",
                hexdump(unicorn, sp, 0x20),
                xp_cstr(unicorn, s4),
                xp_cstr(unicorn, s14),
            );
        }
        _ => {
            log::info!("PROCNAV[{tid}] xp-{tag}: r0={r0:#010x} lr={lr:#010x}");
        }
    }
}

/// Walk an `fwl_List` (list object at `obj+off`, first node at `[list+4]`,
/// `next` at `node+4`, payload at `node+8`) and dump up to 16 payloads.
/// One-shot marker for the fabricated SENSORFI subscription entry (see
/// `fabricate_sensorfi_reference`).
static FABRICATION_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stand in for the sensorfi server's `RegisterConf` bookkeeping: insert one
/// service-reference node into the STAT ail object's reference list (`+0x50`)
/// carrying exactly the tuple `bIsDataSet` (`0x24ea04`) compares against the
/// provider envelope (`serviceId 0xb @0`, `source app 0xfffe @2`, `functionId
/// 5 @4`, `available @6`, `source sub-id 0xfffe @8`).  This is the state the
/// client list would hold after a real `u16RegisterService` round-trip with a
/// sensorfi server app; that server never runs here and its subscription
/// config lives in the encrypted ERG, so the entry is fabricated on the host
/// side (device simulation) instead of patching the guest binary.
fn fabricate_sensorfi_reference(unicorn: &mut Unicorn<'_, Context>, this: u32) {
    let mut list = word(unicorn, this.wrapping_add(0x50)).unwrap_or(0);
    let mmu_arc = {
        let data = unicorn.get_data();
        data.mmu.clone()
    };
    let node = {
        let mut alloc = mmu_arc.lock().unwrap();
        if list == 0 {
            list = alloc.heap_alloc(
                unicorn,
                0x10,
                unicorn_engine::unicorn_const::Prot::READ
                    | unicorn_engine::unicorn_const::Prot::WRITE,
                "[stat-ref-list]",
            );
            let _ = unicorn.mem_write(this.wrapping_add(0x50) as u64, &list.to_le_bytes());
        }
        alloc.heap_alloc(
            unicorn,
            0x20,
            unicorn_engine::unicorn_const::Prot::READ | unicorn_engine::unicorn_const::Prot::WRITE,
            "[stat-sensorfi-ref]",
        )
    };
    let head = word(unicorn, list.wrapping_add(4)).unwrap_or(0);
    // Node: `{prev, next}` header, 16-byte payload at `+8` (the dispatch loop
    // only follows `+4`, `prev` points back at the list head).
    let mut node_bytes = Vec::with_capacity(24);
    node_bytes.extend_from_slice(&list.to_le_bytes()); // prev
    node_bytes.extend_from_slice(&head.to_le_bytes()); // next
    node_bytes.extend_from_slice(&[
        0x0b, 0x00, // +0  serviceId = 0xb (sensorfi)
        0x0a, 0x00, // +2  typebase id low half (envelope u16 @wire+0x00)
        0x05, 0x00, // +4  functionId (envelope u16 @wire+0x16)
        0x01, 0x00, // +6  availability = true
        0xff, 0xff, // +8  source sub-id wildcard (envelope u16 @wire+0x0e = 0xffff)
        0x01, 0x00, // +a  registerId
        0xfe, 0xff, // +c  server app (sentinel)
        0x00, 0x00, // +e  user sub-id
    ]);
    let _ = unicorn.mem_write(node as u64, &node_bytes);
    let _ = unicorn.mem_write(list.wrapping_add(4) as u64, &node.to_le_bytes());
    log::info!(
        "PROCNAV: fabricated sensorfi service-reference {node:#010x} into STAT ail object {this:#010x} (list {list:#010x})"
    );
}

fn dump_fwl_list(
    unicorn: &mut Unicorn<'_, Context>,
    obj: u32,
    off: u32,
    width: usize,
) -> Vec<String> {
    let list = word(unicorn, obj.wrapping_add(off)).unwrap_or(0);
    let mut cur = word(unicorn, list.wrapping_add(4)).unwrap_or(0);
    let mut v = Vec::new();
    for _ in 0..16 {
        if cur == 0 {
            break;
        }
        v.push(format!("{}", hexdump(unicorn, cur.wrapping_add(8), width)));
        cur = word(unicorn, cur.wrapping_add(4)).unwrap_or(0);
    }
    v
}

pub fn procnav_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let repost_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let reg_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let new_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let sd_hot = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let built_ptr = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let ail_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
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
        ("probe-list-erase", PROBE_LIST_ERASE, 2),
        ("probe-ctl-thunk", PROBE_CTL_THUNK, 4),
        ("probe-ds-tuple", PROBE_DS_TUPLE, 2),
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
        ("probe-sd-procmsg", PROBE_SD_PROCMSG, 2),
        ("probe-sd-rtdisp", PROBE_SD_RTDISP, 2),
        ("probe-dispatcher", PROBE_DISPATCHER, 2),
        ("probe-regprobe", PROBE_REGPROBE, 2),
        ("probe-sd-gate", PROBE_SD_GATE, 2),
        ("probe-regobj", PROBE_REGOBJ, 2),
        ("probe-commfind", PROBE_COMMFIND, 2),
        ("probe-commset", PROBE_COMMSET, 2),
        ("probe-memopen1", PROBE_MEMOPEN1, 2),
        ("probe-memopen2", PROBE_MEMOPEN2, 2),
        ("probe-registrar", PROBE_REGISTRAR, 2),
        ("probe-sd-pump", PROBE_SD_PUMP, 2),
        ("probe-sd-tg1", PROBE_SD_TG1, 2),
        ("probe-sd-tgs", PROBE_SD_TGS, 2),
        ("probe-sd-tgf", PROBE_SD_TGF, 2),
        ("probe-sd-cpy0", PROBE_SD_CPY0, 2),
        ("probe-sd-cpy1", PROBE_SD_CPY1, 2),
        ("probe-sd-binit", PROBE_SD_BINIT, 2),
        ("probe-sd-build", PROBE_SD_BUILD, 2),
        ("probe-sd-refcmp", PROBE_SD_REFCMP, 2),
        ("probe-sd-regcmp", PROBE_SD_REGCMP, 2),
        ("probe-sd-refres", PROBE_SD_REFRES, 2),
        ("probe-sd-regres", PROBE_SD_REGRES, 2),
        ("probe-sd-dlvchk", PROBE_SD_DLVCHK, 2),
        ("probe-sd-deliver", PROBE_SD_DELIVER, 2),
        ("probe-regsvc", PROBE_REGSVC, 2),
        ("probe-scd", PROBE_SCD, 2),
        ("probe-msgreg", PROBE_MSGREG, 2),
        ("probe-msgconf", PROBE_MSGCONF, 2),
    ] {
        let address = (base_address + offset) as u64;
        let repost_count = repost_count.clone();
        let reg_count = reg_count.clone();
        let new_count = new_count.clone();
        let sd_hot = sd_hot.clone();
        let built_ptr = built_ptr.clone();
        let ail_count = ail_count.clone();
        let result = unicorn.add_code_hook(address, address + size, move |uc, addr, _| {
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
                } else if matches!(name, "probe-registrar" | "probe-regprobe" | "probe-regobj") {
                    (&reg_count, 1500)
                } else if matches!(name, "probe-sd-refcmp" | "probe-sd-regcmp") {
                    (&reg_count, 1500)
                } else if matches!(
                    name,
                    "probe-sd-refres"
                        | "probe-sd-regres"
                        | "probe-sd-dlvchk"
                        | "probe-sd-deliver"
                        | "probe-regsvc"
                        | "probe-scd"
                        | "probe-msgreg"
                        | "probe-msgconf"
                ) {
                    (&ail_count, 900)
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
                "probe-ds-tuple" => {
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0) as u32;
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0) as u32;
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let this = word(uc, r4).unwrap_or(0);
                    let arg5 = word(uc, sp).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-ds-tuple: this={this:#010x} key@{r0:#010x}=[{}] a1={r1:#x} a2={r2:#x} a3={r3:#x} a4={arg5:#x}",
                        hexdump(uc, r0, 12)
                    );
                }
                "probe-ctl-thunk" => {
                    static THUNK: std::sync::atomic::AtomicU32 =
                        std::sync::atomic::AtomicU32::new(0);
                    if THUNK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                        let lr = (uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32) & !1;
                        log::info!("PROCNAV[{tid}] probe-ctl-thunk: lr={lr:#010x}");
                    }
                }
                "probe-list-erase" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    // Fatal precondition of the tail `strb r6,[r0]` (0x5a43f1):
                    // the slot word is NULL (and thus not the container itself).
                    if word(uc, r1).unwrap_or(1) == 0 {
                        log::info!(
                            "PROCNAV[{tid}] probe-list-erase: NULL slot container={r0:#010x} slot={r1:#010x} lr={:#010x}",
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
                        // The send leaf `0x19089e` validates `[wrapper+0xc]` as the
                        // message's embedded buffer. The real unit's deep task stack
                        // leaves a stale non-zero word there (the builder itself never
                        // writes +0xc); the emulator's deterministic stack leaves 0 and
                        // the guard then silently drops the post. Point it at the freshly
                        // allocated data block - the same role the embedded buffer has on
                        // a fully deserialised wrapper.
                        if let Some(data) = word(uc, wrapper.wrapping_add(4)) {
                            if data != 0 {
                                let _ = uc.mem_write(
                                    wrapper.wrapping_add(0xc) as u64,
                                    &data.to_le_bytes(),
                                );
                            }
                        }
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
                "probe-sd-gate" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    log::info!("PROCNAV[{tid}] probe-sd-gate: acceptor={r0:#x} adapter={r4:#010x}");
                }
                "probe-regobj" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0);
                    // Registry singleton (`0x01215180` literal + 8): dump its live
                    // contents - vtable, state, and the record head.
                    let obj = r0.wrapping_add(8);
                    let vt = word(uc, obj).unwrap_or(0);
                    let s0 = word(uc, vt).unwrap_or(0);
                    let s8 = word(uc, vt.wrapping_add(8)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-regobj: obj={obj:#010x} vt={vt:#010x} slot0={s0:#010x} slot8={s8:#010x} id={r4:#x} {}",
                        hexdump(uc, obj, 0x20)
                    );
                }
                "probe-commfind" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0);
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0);
                    let r7 = uc.reg_read(RegisterARM::R7).unwrap_or(0);
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                    let list = word(uc, r4.wrapping_add(0x4c)).unwrap_or(0);
                    let rec = if r5 != 0 {
                        format!(" {}", hexdump(uc, r5 as u32, 0x30))
                    } else {
                        String::new()
                    };
                    log::info!(
                        "PROCNAV[{tid}] probe-commfind: type={r6:#x} sub={r7:#x} list={list:#010x} found={r5:#010x} r0={r0:#x} lr={lr:#010x}{rec}"
                    );
                }
                "probe-commset" => {
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0);
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-commset: list={r1:#010x} owner={r2:#010x} lr={lr:#010x}");
                }
                "probe-memopen1" => {
                    let ip = uc.reg_read(RegisterARM::R12).unwrap_or(0);
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let r6 = word(uc, sp.wrapping_add(0x24)).unwrap_or(0);
                    let obj = word(uc, r6.wrapping_add(0x20)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-memopen1: rec={r6:#010x} obj={obj:#010x} memberid={ip:#x} {}",
                        hexdump(uc, obj, 0x10)
                    );
                }
                "probe-memopen2" => {
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let r4 = word(uc, sp.wrapping_add(0x1c)).unwrap_or(0);
                    let mid = word(uc, sp.wrapping_add(0x20)).unwrap_or(0);
                    let lo = word(uc, sp.wrapping_add(0x24)).unwrap_or(0);
                    let hi = word(uc, sp.wrapping_add(0x28)).unwrap_or(0);
                    let vec = word(uc, r4).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-memopen2: memberid={mid:#x} lo={lo:#x} hi={hi:#x} vec={vec:#010x}"
                    );
                }
                "probe-sd-procmsg" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-procmsg: r0={r0:#010x} r1={r1:#010x} lr={lr:#010x} obj={}",
                        hexdump(uc, r0, 0x18)
                    );
                }
                "probe-sd-rtdisp" => {
                    let ip = uc.reg_read(RegisterARM::R12).unwrap_or(0) as u32;
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0) as u32;
                    let target = word(uc, ip.wrapping_add(0x14)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-sd-rtdisp: vt={ip:#010x} target={target:#010x} r6={r6:#010x}"
                    );
                }
                "probe-dispatcher" => {
                    // Entry: `0x2e552e` reloads the object from the *pushed r0*,
                    // so r0 at entry is the dispatcher object; r1 = msg wrapper.
                    let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    let list = word(uc, obj.wrapping_add(0xc)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-dispatcher: obj={obj:#010x} list={list:#010x} wrapper={r1:#010x} lr={lr:#010x}"
                    );
                }
                "probe-registrar" => {
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let ans = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-registrar: class={r4:#x} answer={ans:#x}");
                }
                "probe-regprobe" => {
                    // Inside `0x2e6824` after `bl 0x2e65cc`; the object (`r6`'s
                    // incoming value) was pushed first, so it sits at `[sp,#0x14]`.
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let obj = word(uc, sp.wrapping_add(0x14)).unwrap_or(0);
                    let vt = word(uc, obj).unwrap_or(0);
                    let slot = word(uc, vt.wrapping_add(8)).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-regprobe: obj={obj:#010x} vt={vt:#010x} slot8={slot:#010x} probe={r0:#x}"
                    );
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
                    if tid == 76 {
                        let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                        let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                        log::info!(
                            "PROCNAV[{tid}] svc-data stack lr={lr:#010x} sp={} window={}",
                            format!("{sp:#010x}"),
                            hexdump(uc, sp, 0x80)
                        );
                    }
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
                "probe-sd-refcmp" | "probe-sd-regcmp" => {
                    // `r0` = list iterator on stack; the candidate node is `[r0+8]`,
                    // payload at `node+8`. `r1..r3` + `[sp]` carry the message tuple.
                    let iter = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let node = word(uc, iter.wrapping_add(8)).unwrap_or(0);
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0);
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0);
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let a4 = word(uc, sp).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] {name}: node={node:#010x} data=[{}] tuple=({r1:#x},{r2:#x},{r3:#x},{a4:#x})",
                        if node != 0 { hexdump(uc, node.wrapping_add(8), 12) } else { "<null>".into() },
                    );
                }
                "probe-sd-refres" | "probe-sd-regres" => {
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0) as u32;
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0);
                    let r6 = uc.reg_read(RegisterARM::R6).unwrap_or(0);
                    let this = word(uc, r4).unwrap_or(0);
                    let app = halfword(uc, this.wrapping_add(0x10));
                    let refs = dump_fwl_list(uc, this, 0x50, 12);
                    let regs = dump_fwl_list(uc, this, 0x58, 12);
                    log::info!(
                        "PROCNAV[{tid}] {name}: this={this:#010x} app={app:?} matched={r6} bvar={r5} refs=[{refs:?}] regs=[{regs:?}]"
                    );
                    // The real SENSORFI subscription list lives in the encrypted
                    // NAVAPP.ERG, so the STAT guest never calls
                    // `u16RegisterService` here and its reference list at `+0x50`
                    // stays empty - `bIsDataSet` then rejects every GPS message
                    // before the pump.  Stand in for the (absent) sensorfi server's
                    // RegisterConf by inserting the reference entry the client
                    // bookkeeping would hold after a successful round-trip:
                    // `{serviceId, sourceApp, fn, available, ..., subID}` pinned to
                    // the provider envelope (svc `0xb`, src app `0xfffe`, fn `5`,
                    // sub `0xfffe`).  The list is the same singly-linked shape the
                    // dispatch loop walks (`[list+4]` = first node, `node+4` =
                    // next, payload at `node+8`).
                    if name == "probe-sd-refres"
                        && app == Some(22)
                        && refs.is_empty()
                        && !FABRICATION_DONE.swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        fabricate_sensorfi_reference(uc, this);
                    }
                }
                "probe-sd-dlvchk" => {
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0);
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0) as u32;
                    let f6c = word(uc, sp.wrapping_add(0x6c)).unwrap_or(0);
                    log::info!("PROCNAV[{tid}] probe-sd-dlvchk: deliver={r5} f6c={f6c:#010x}");
                }
                "probe-sd-deliver" => {
                    let obj = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let msg = uc.reg_read(RegisterARM::R1).unwrap_or(0);
                    let vt = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] PROBE-SD-DELIVER: obj={obj:#010x} msg={msg:#010x} pump={vt:#010x} fields={}",
                        hexdump(uc, (msg as u32).wrapping_add(4), 0x20),
                    );
                }
                "probe-regsvc" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0);
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    let r3 = uc.reg_read(RegisterARM::R3).unwrap_or(0);
                    let r4 = uc.reg_read(RegisterARM::R4).unwrap_or(0);
                    let r5 = uc.reg_read(RegisterARM::R5).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-regsvc: r0={r0:#010x} r1={r1:#x} r2={r2:#x} r3={r3:#x} r4={r4:#010x} r5={r5:#x}"
                    );
                }
                "probe-scd" => {
                    let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0);
                    let r1 = uc.reg_read(RegisterARM::R1).unwrap_or(0);
                    let r2 = uc.reg_read(RegisterARM::R2).unwrap_or(0);
                    log::info!(
                        "PROCNAV[{tid}] probe-scd: svc={r0:#x} out={r1:#010x} flag={r2:#x}"
                    );
                }
                "probe-msgreg" | "probe-msgconf" => {
                    let this = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                    let obj = word(uc, this).unwrap_or(0);
                    let app = halfword(uc, obj.wrapping_add(0x10));
                    log::info!("PROCNAV[{tid}] {name}: this={this:#010x} obj={obj:#010x} app={app:?}");
                }
                _ => {}
            }
        });
        match result {
            Ok(_) => log::info!("PROCNAV: observation hook '{name}' at {address:#x}"),
            Err(error) => log::warn!("PROCNAV: cannot hook {name} at {address:#x}: {error}"),
        }
    }

    // Page-granular watchers replacing the narrow hooks that unicorn 2.1.5
    // silently drops (see `XP_TARGETS`); the callbacks dispatch on the exact
    // fetch address, so the extra cost is one table lookup per instruction
    // inside these pages.
    for (page, span) in XP_PAGES {
        let address = (base_address + (page - ORIGINAL_BASE)) as u64;
        let result = unicorn.add_code_hook(
            address,
            address + *span as u64,
            move |uc, addr, _| {
                let tid = uc.get_data().inner.thread_id();
                xp_watch(uc, tid, addr);
            },
        );
        match result {
            Ok(_) => log::info!("PROCNAV: xp-watch page {address:#x}+{span:#x}"),
            Err(error) => log::warn!("PROCNAV: cannot hook xp page {address:#x}: {error}"),
        }
    }

    // The sensorfi GPS provider is part of the emulator's device simulation
    // (there is no hardware receiver behind us), so it runs by default with
    // every PROCNAV; `EMU_SENSORFI=0` turns it off for control runs (e.g. the
    // r349-style "no provider" experiments).
    if std::env::var("EMU_SENSORFI")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(true)
    {
        spawn_gps_provider(unicorn);
    }
}

/// Push periodic `cmb_sensorfi_tclMsgGpsDataStatus` messages into the target
/// component's mailbox, acting as the sensorfi server GPS provider (the device
/// simulation stands in for the real sensorfi server application, which never
/// runs here).
///
/// The blob is pre-snapshotted (`[u32 flag, u32 len, content]`) so the monitor's
/// `0x2d` queue-wait materializes it into PROCNAV's own VM, and the `0x57`
/// reference resolves to the content base exactly like a Linux-side poster's.
/// The content's amt header uses the offsets the `amt_tclBaseMessage` accessors
/// read (`u16GetValue`: serviceId @ +0x14, propertyId @ +0x18, opCode @ +0x1a,
/// class @ +0xb), pinned by `cmb_sensorfi_tclMsgGpsDataStatus`'s getters:
/// service `0xb`, property `5`, opcode `6` (Status). The CIR payload follows
/// the header at +0x20 with the layout decoded from the guest's own `oRead`
/// chain (see `gps_wire_body`).
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
                let blob = gps_status_message(seq, 0x45);
                // STAT component (app 22): the CCA router at `0xbd50c` decides
                // ownership on `mbx_22` and the fabricated service reference
                // (see `fabricate_sensorfi_reference`) lets `bIsDataSet` match
                // it there.  (Envelope words now follow the real captured wire
                // format, so the old "route by the u16 at blob + 0xa" trick no
                // longer holds - the target is simply this provider's consumer.)
                let queue = "mbx_22".to_string();
                let posted = {
                    let mut guard = namespace.lock().unwrap();
                    crate::common::osal_queues::OsalQueueService::ensure_queue(
                        &mut guard.mq,
                        &queue,
                    );
                    let posted = crate::common::osal_queues::OsalQueueService::guest_post(
                        &mut guard.mq,
                        &queue,
                        blob,
                        0,
                    );
                    if posted {
                        guard.notify_waiters();
                    }
                    posted
                };
                if posted {
                    log::info!("GPS provider: posted GpsDataStatus #{seq} to {queue}");
                } else {
                    log::debug!("GPS provider: {queue} full, skipping #{seq}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    match result {
        Ok(_) => log::info!("PROCNAV: GPS sensorfi provider started (EMU_SENSORFI=0 disables)"),
        Err(error) => log::warn!("PROCNAV: cannot start GPS provider: {error}"),
    }
}

/// One `GpsDataStatus` snapshot blob: envelope + amt header, zero payload.
/// `msg_class` selects the amt class byte (`0x45` ServiceData for the AE pump's
/// re-post pipeline, `0x46` = RELUP data for direct component-queue delivery).
/// Krakow city centre - the coordinates the emulator should show on the map.
const GPS_LAT_DEG: f32 = 50.0614;
const GPS_LON_DEG: f32 = 19.9456;

/// Wire body of `cmb_sensorfi_tclMsgGpsDataStatus` (svc `0xb`, fn `5`,
/// opcode `6`), reconstructed from the guest's own deserializer
/// (`oRead` `0x5b9bad` -> `fi` operator>> `0x32b265` -> generic typebase
/// driver `0x2f55cf` -> const-data reader `0x2f510c`): the const part is 92
/// packed bytes
/// ```text
/// u16,u16, 8*u8, u32, u64,u64, 8*f32, 4*u8, 4*u32, 4*u8, 2*f32
/// ```
/// then the VAR section starts with a u32 vector count plus that many
/// elements.  A zero count leaves the `bpstl::vector` buffer NULL and the
/// guest's copy helper stores through it (run r348: `WRITE_UNMAPPED @
/// 0x5a43f0` killed the STAT pump thread), so the message always carries at
/// least one satellite element.  Coordinates are written in several plausible
/// encodings (degrees-as-f32 and arcsec*1000-as-u32) because the field
/// semantics in the const block are not yet pinned down.
fn gps_wire_body() -> Vec<u8> {
    let mut body = Vec::with_capacity(0x80);
    // Generic typebase driver (`0x2f55cf`) first consumes one u32 into the
    // typebase header field (`typebase + 4`), then the const block.
    body.extend_from_slice(&0u32.to_le_bytes());
    let lat_arcsec_x1000: u32 = (GPS_LAT_DEG * 3_600_000.0) as u32;
    let lon_arcsec_x1000: u32 = (GPS_LON_DEG * 3_600_000.0) as u32;
    // u16, u16 - unknown counters (week/second?), leave zero.
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    // 8 octets: status flags - claim a valid 3D fix with SPS.
    body.extend_from_slice(&[1, 1, 1, 0, 0, 0, 0, 0]);
    // u32 at object +0x10: latitude, arcsec*1000.
    body.extend_from_slice(&lat_arcsec_x1000.to_le_bytes());
    // two u64 at +0x18/+0x20: timestamps, leave zero (host time not modelled).
    body.extend_from_slice(&0u64.to_le_bytes());
    body.extend_from_slice(&0u64.to_le_bytes());
    // 8 f32 at +0x28..+0x44: first two carry lat/lon in degrees, the rest 0.
    for v in [GPS_LAT_DEG, GPS_LON_DEG] {
        body.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    for _ in 0..6 {
        body.extend_from_slice(&0u32.to_le_bytes());
    }
    // 4 octets at +0x48: more validity flags.
    body.extend_from_slice(&[1, 1, 1, 0]);
    // 4 u32 at +0x4c..+0x58: lat/lon arcsec*1000, then zero.
    body.extend_from_slice(&lat_arcsec_x1000.to_le_bytes());
    body.extend_from_slice(&lon_arcsec_x1000.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    // 4 octets at +0x58.
    body.extend_from_slice(&[1, 0, 0, 0]);
    // 2 f32 at +0x5c/+0x60: lat/lon degrees again (last plausible pair).
    body.extend_from_slice(&GPS_LAT_DEG.to_bits().to_le_bytes());
    body.extend_from_slice(&GPS_LON_DEG.to_bits().to_le_bytes());
    // VAR section: satellite vector count, then one 16-byte element
    // (`cmb_fi_tcl_GpsBigVarData::oRead` 0x2f547b: u8,u8,u16,f32,f32,f32).
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&[1u8, 0u8]); // unknown flag bytes (obj +4, +5)
    body.extend_from_slice(&40u16.to_le_bytes()); // signal strength (obj +6)
    for v in [0.0f32, 45.0, 270.0] {
        // doppler, elevation, azimuth (obj +8, +0xc, +0x10)
        body.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    body
}

pub(crate) fn gps_status_message(seq: u16, msg_class: u8) -> Vec<u8> {
    let body = gps_wire_body();
    let len = 0x20 + body.len();
    let LEN = len.next_multiple_of(16);
    let mut content = vec![0u8; LEN];
    // Real captured wire (power-proxy, typebase 0x22): word0 = typebase ID,
    // word1 = body length, word2 = class/dest combo, word3 = destination - and
    // the null-message shape check (`0x2981c3`, run over the first six words
    // by `STAT_tclThread::vEntry`'s parse pipeline at `0x2981e7`) demands the
    // first four words NON-zero and words 4+5 == 0xffffffff, or the message is
    // declared "Unknown type of message" (RDE 0x43001c) and dropped.
    content[0x00..0x04].copy_from_slice(&0x0000000au32.to_le_bytes()); // typebase: sensorfi family (msgEntries svc 0xa)
    content[0x04..0x08].copy_from_slice(&(LEN as u32).to_le_bytes()); // snapshot/object length
    content[0x08] = 0x07; // real captures carry 0x07 here (class family byte)
    content[0x0b] = msg_class;
    content[0x0c..0x10].copy_from_slice(&0xffff_ffffu32.to_le_bytes()); // sub-ids 0xffff/0xffff (non-zero word3 + the 0xff sentinel word)
    // amt service envelope fields (the STAT CCA dispatch chain):
    content[0x14..0x16].copy_from_slice(&0x000bu16.to_le_bytes()); // serviceId: sensorfi
    // `bIsDataSet` (`0x24ea04`) compares the reference entry against getters at
    // wire `+0x00` (source app), `+0x14`, `+0x16` and `+0x0e`; `0x2c45b1` reads
    // `+0x16`, so this is the functionId slot (`GpsDataStatus` fn `5`).
    content[0x16..0x18].copy_from_slice(&0x0005u16.to_le_bytes()); // functionId: GPS data
    content[0x18..0x1a].copy_from_slice(&0x0005u16.to_le_bytes()); // propertyId: GPS data
    content[0x1a] = 0x06; // opCode: Status (server message)
    content[0x1c..0x1e].copy_from_slice(&seq.to_le_bytes()); // cmd counter
    content[0x20..0x20 + body.len()].copy_from_slice(&body); // CIR payload
    let mut blob = Vec::with_capacity(8 + LEN);
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&(LEN as u32).to_le_bytes());
    blob.extend_from_slice(&content);
    blob
}