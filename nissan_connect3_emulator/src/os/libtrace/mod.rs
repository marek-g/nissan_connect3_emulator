mod trace;

use crate::emulator::context::Context;
use crate::os::libtrace::trace::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use unicorn_engine::{RegisterARM, Unicorn};

pub fn libtrace_add_code_hooks(unicorn: &mut Unicorn<Context>, base_address: u32) {
    let mut method_entries = HashMap::new();
    insert_libtrace_method_entries(&mut method_entries);

    unicorn
        .add_code_hook(
            (base_address + 0x00002f58) as u64,
            (base_address + 0x00002f58) as u64,
            |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [LIBTRACE HOOK] _init()",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                );
                let res = trace_init();
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            (base_address + 0x00004634) as u64,
            (base_address + 0x00004634) as u64,
            |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [LIBTRACE HOOK] TR_chan_acess_bRegChan()",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                );
                let res = trace_tr_chan_access();
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            (base_address + 0x000043a0) as u64,
            (base_address + 0x000043a0) as u64,
            |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [LIBTRACE HOOK] TR_core_uwTraceOut()",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                );
                let res = trace_tr_core_uw_trace_out();
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            (base_address + 0x00007864) as u64,
            (base_address + 0x00007864) as u64,
            |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [LIBTRACE HOOK] TRACE_sharedmem_create_dualOS()",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                );
                let res = trace_sharedmem_create_dual_os();
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            (base_address + 0x0000513c) as u64,
            (base_address + 0x0000513c) as u64,
            |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [LIBTRACE HOOK] TRACE_stop()",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                );
                let res = trace_stop();
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            },
        )
        .unwrap();

    unicorn
        .add_code_hook(
            (base_address + 0x000076e4) as u64,
            (base_address + 0x000076e4) as u64,
            |uc, addr, _| {
                log::trace!(
                    "{:#x}: [{}] [LIBTRACE HOOK] TR_core_bIsClassSelected()",
                    uc.reg_read(RegisterARM::PC).unwrap(),
                    uc.get_data().inner.thread_id,
                );
                let res = trace_tr_core_is_class_selected();
                uc.reg_write(RegisterARM::R0, res as u64).unwrap();
                uc.reg_write(RegisterARM::PC, uc.reg_read(RegisterARM::LR).unwrap())
                    .unwrap();
            },
        )
        .unwrap();

    for (mut address, method_name) in method_entries {
        address = base_address + address;
        unicorn
            .add_code_hook(address as u64, address as u64, move |uc, addr, _| {
                handle_hook(uc, addr, method_name)
            })
            .unwrap();
    }
}

fn handle_hook(uc: &mut Unicorn<Context>, addr: u64, method_name: &str) {
    log::trace!(
        "{:#x} [{}] [LIBTRACE] {}() [IN]",
        addr,
        uc.get_data().inner.thread_id,
        method_name
    );
}

// rabin2 -E ./libtrace.so
fn insert_libtrace_method_entries(method_entries: &mut HashMap<u32, &str>) {
    method_entries.insert(0x00006690, "TRACE_iosc_dl_setEvent");
    method_entries.insert(0x000088c8, "UTIL_swap");
    method_entries.insert(0x00007fe8, "TRACE_delete_flag_singleOS");
    method_entries.insert(0x00006708, "TRACE_iosc_dl_clearEvent");
    method_entries.insert(0x000043d0, "TR_proxy_uwTrace");
    method_entries.insert(0x00008130, "UTIL_IsClassSelected");
    method_entries.insert(0x000085dc, "itoa");
    method_entries.insert(0x000075a4, "TRACE_STATLIB_uninit");
    method_entries.insert(0x000072b8, "TRACE_socket_send");
    method_entries.insert(0x00008148, "UTIL_TraceOut");
    method_entries.insert(0x000089c4, "UTIL_trace_buf");
    method_entries.insert(0x000073d0, "TRACE_socket_open");
    method_entries.insert(0x00009e9c, "g_TRACE_fnPtr");
    method_entries.insert(0x000065e8, "TRACE_iosc_dl_releaseSem");
    method_entries.insert(0x00006620, "TRACE_iosc_dl_destroySem");
    method_entries.insert(0x00004558, "TRACE_create_task");
    //method_entries.insert(0x00007864, "TRACE_sharedmem_create_dualOS");
    method_entries.insert(0x00007d14, "TRACE_mutex_init_singleOS");
    method_entries.insert(0x00007d88, "TRACE_mutex_lock_singleOS");
    method_entries.insert(0x000068b8, "TRACE_iosc_dl_createSem");
    method_entries.insert(0x0000780c, "TRACE_sharedmem_init_done_dualOS");
    method_entries.insert(0x00007284, "TRACE_socket_receive");
    method_entries.insert(0x00008998, "UTIL_atraceb");
    method_entries.insert(0x00006514, "TRACE_snd_status");
    method_entries.insert(0x00006a70, "TRACE_q_fill_status");
    method_entries.insert(0x00007b54, "TRACE_mutex_lock_dualOS");
    method_entries.insert(0x00006810, "TRACE_iosc_dl_destroyMutex");
    method_entries.insert(0x00007bb8, "TRACE_clear_flag_dualOS");
    method_entries.insert(0x00003954, "TRACE_main");
    method_entries.insert(0x00008508, "UTIL_cre_mpf");
    method_entries.insert(0x00008c5c, "_fini");
    method_entries.insert(0x000077f4, "TRACE_sharedmem_isnew_dualOS");
    method_entries.insert(0x00006914, "TRACE_iosc_dl_close");
    method_entries.insert(0x00008974, "UTIL_traceb_mst");
    method_entries.insert(0x0000673c, "TRACE_iosc_dl_destroyEvent");
    method_entries.insert(0x00004e9c, "TRACE_commandLine_param");
    method_entries.insert(0x00007b6c, "TRACE_release_q_lock_dualOS");
    method_entries.insert(0x000036c0, "TRACE_media_detach");
    method_entries.insert(0x00006ec4, "TRACE_q_push");
    method_entries.insert(0x00006654, "TRACE_iosc_dl_createEvent");
    method_entries.insert(0x00003fc4, "TR_core_uwTraceBinOutput");
    method_entries.insert(0x00008128, "UTIL_UnregisterChannel");
    method_entries.insert(0x00007b94, "TRACE_q_lock_init_dualOS");
    method_entries.insert(0x00004400, "TR_chan_acess_bUnRegChan");
    method_entries.insert(0x00008490, "UTIL_get_mpf");
    method_entries.insert(0x000088d4, "UTIL_test_swap32");
    method_entries.insert(0x000084f0, "UTIL_del_mpf");
    method_entries.insert(0x00003950, "TRACE_frontend_init");
    method_entries.insert(0x00006770, "TRACE_iosc_dl_createMutex");
    method_entries.insert(0x0000797c, "TRACE_sharedmem_init_done_singleOS");
    method_entries.insert(0x000085b0, "getDigits");
    method_entries.insert(0x00007800, "TRACE_sharedmem_mem_dualOS");
    method_entries.insert(0x000077dc, "TRACE_sharedmem_isnew_singleOS");
    method_entries.insert(0x0000371c, "TRACE_media_attach");
    method_entries.insert(0x00009de8, "__data_start");
    method_entries.insert(0x00006b04, "TRACE_q_release_wait_tasks");
    method_entries.insert(0x00008310, "UTIL_del_hsh");
    method_entries.insert(0x0000513c, "TRACE_stop");
    method_entries.insert(0x000065ac, "TRACE_iosc_dl_obtainSem");
    method_entries.insert(0x00008638, "UTIL_axtoi");
    method_entries.insert(0x000084e8, "UTIL_blfsz_mpf");
    method_entries.insert(0x00008794, "UTIL_tokenizer");
    method_entries.insert(0x00007380, "TRACE_socket_close");
    method_entries.insert(0x00004bfc, "TRACE_get_sh_cnfg");
    method_entries.insert(0x00007998, "TRACE_sharedmem_destroy_singleOS");
    method_entries.insert(0x00008940, "UTIL_trace_chan_reg");
    method_entries.insert(0x000084e0, "UTIL_sizeof_cmpf");
    method_entries.insert(0x000066c4, "TRACE_iosc_dl_wait_for_event");
    method_entries.insert(0x00009874, "__exidx_end");
    method_entries.insert(0x0000812c, "UTIL_RegisterChannel");
    method_entries.insert(0x00007570, "TRACE_socket_open_sender");
    //method_entries.insert(0x000076e4, "TR_core_bIsClassSelected");
    method_entries.insert(0x00007cac, "TRACE_q_lock_init_singleOS");
    method_entries.insert(0x00009dec, "g_TRACE_ver");
    method_entries.insert(0x000067dc, "TRACE_iosc_dl_leaveMutex");
    method_entries.insert(0x00003988, "trace_drv_core_uninit");
    method_entries.insert(0x00007df8, "TRACE_wait_flag_singleOS");
    method_entries.insert(0x00007d7c, "TRACE_release_q_lock_singleOS");
    method_entries.insert(0x00007c60, "TRACE_delete_flag_dualOS");
    method_entries.insert(0x00007d84, "TRACE_obtain_q_lock_singleOS");
    method_entries.insert(0x0000890c, "UTIL_trace_chan_unreg");
    method_entries.insert(0x00006498, "TRACE_cre_hdr");
    method_entries.insert(0x00008710, "UTIL_stricmp");
    method_entries.insert(0x00007b64, "TRACE_mutex_unlock_dualOS");
    method_entries.insert(0x0000844c, "UTIL_init_mpf");
    method_entries.insert(0x00009e10, "runmode");
    method_entries.insert(0x000077e8, "TRACE_sharedmem_mem_singleOS");
    method_entries.insert(0x00007bc0, "TRACE_set_flag_dualOS");
    method_entries.insert(0x00008980, "UTIL_traceb");
    method_entries.insert(0x00004c44, "TRACE_get_sh_lock_cnfg");
    method_entries.insert(0x000067a4, "TRACE_iosc_dl_enterMutex");
    method_entries.insert(0x00008904, "UTIL_get_cpu_core");
    method_entries.insert(0x00008214, "UTIL_rem_hsh");
    method_entries.insert(0x00006378, "TRACE_init_task");
    method_entries.insert(0x00007828, "TRACE_sharedmem_destroy_dualOS");
    method_entries.insert(0x00007be4, "TRACE_wait_flag_dualOS");
    //method_entries.insert(0x000052b0, "TRACE_start");
    method_entries.insert(0x000089a8, "UTIL_trace_isActive");
    method_entries.insert(0x00006964, "TRACE_dl_init");
    method_entries.insert(0x00006930, "TRACE_iosc_dl_exit");
    method_entries.insert(0x00007b74, "TRACE_mutex_init_dualOS");
    method_entries.insert(0x00006a38, "TRACE_iosc_dl_init");
    method_entries.insert(0x000081b4, "UTIL_sea_hsh");
    method_entries.insert(0x00007dd4, "TRACE_clear_flag_singleOS");
    method_entries.insert(0x00008290, "UTIL_add_hsh");
    method_entries.insert(0x00006884, "TRACE_iosc_dl_shared_mem_free");
    method_entries.insert(0x00008044, "TRACE_create_flag_singleOS");
    method_entries.insert(0x00008378, "UTIL_cre_hsh");
    //method_entries.insert(0x00004634, "TR_chan_acess_bRegChan");
    method_entries.insert(0x00008334, "UTIL_clr_hsh");
    method_entries.insert(0x00004484, "TRACE_unreg_notify_evt");
    method_entries.insert(0x00003928, "TRACE_dll_uninit");
    method_entries.insert(0x0000397c, "TRACE_dll_init");
    method_entries.insert(0x00006844, "TRACE_iosc_dl_shared_malloc_with_id");
    method_entries.insert(0x00003780, "TRACE_lock_seed_status");
    method_entries.insert(0x00003994, "trace_drv_core_init");
    method_entries.insert(0x0000367c, "TRACE_get_sh_mem");
    method_entries.insert(0x00009e14, "ioscFn");
    method_entries.insert(0x00007614, "TR_proxy_bIsClassSelected");
    method_entries.insert(0x00003778, "TRACE_reg_backend");
    method_entries.insert(0x00007fb0, "TRACE_set_flag_singleOS");
    method_entries.insert(0x00007d80, "TRACE_mutex_unlock_singleOS");
    method_entries.insert(0x00007b5c, "TRACE_obtain_q_lock_dualOS");
    method_entries.insert(0x000074d8, "TRACE_socket_open_receiver");
    method_entries.insert(0x00007a24, "TRACE_sharedmem_create_singleOS");
    method_entries.insert(0x00004720, "TRACE_reg_notify_evt");
    method_entries.insert(0x000037fc, "TR_core_s32SendCmd");
    //method_entries.insert(0x000043a0, "TR_core_uwTraceOut");
    method_entries.insert(0x000084c0, "UTIL_rel_mpf");
    method_entries.insert(0x00009874, "__exidx_start");
    method_entries.insert(0x00007c78, "TRACE_create_flag_dualOS");
    method_entries.insert(0x00006b8c, "TRACE_q_pop");
    method_entries.insert(0x00008144, "UTIL_TraceBinOutput");
    //method_entries.insert(0x00002f58, "_init");
    method_entries.insert(0x00007270, "TRACE_socket_init");
}
