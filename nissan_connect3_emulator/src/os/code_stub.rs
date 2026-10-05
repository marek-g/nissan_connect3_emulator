use crate::emulator::context::Context;
use std::cell::Cell;
use unicorn_engine::Unicorn;

/// Host-side replacement for a stubbed guest function. Returns the value to place
/// in R0; the dispatcher then jumps back to the caller (PC = LR).
pub type CodeStubHandler = fn(&mut Unicorn<'_, Context>) -> u32;

thread_local! {
    static CURRENT_STUB_NAME: Cell<&'static str> = const { Cell::new("") };
}

pub fn current_stub_name() -> &'static str {
    CURRENT_STUB_NAME.with(|name| name.get())
}

pub fn set_current_stub_name(name: &'static str) {
    CURRENT_STUB_NAME.with(|cell| cell.set(name));
}

#[derive(Clone, Copy)]
pub struct CodeStub {
    pub lib: &'static str,
    pub name: &'static str,
    pub handler: CodeStubHandler,
}

/// `svc #0` (ARM, little-endian). Patched over a stubbed function's first
/// instruction so that calling it traps into the single intr hook instead of
/// running the original body.
pub const SVC0: [u8; 4] = [0x00, 0x00, 0x00, 0xef];

/// Replace the function at `address` with a `svc #0` trap and record how to
/// emulate it. Unlike `add_code_hook!` this registers no per-instruction Unicorn
/// code hook: the (already-present) intr hook dispatches on PC when the svc fires.
pub fn add_code_stub(
    unicorn: &mut Unicorn<'_, Context>,
    lib: &'static str,
    address: u32,
    name: &'static str,
    handler: CodeStubHandler,
) {
    unicorn.mem_write(address as u64, &SVC0).unwrap();
    unicorn
        .get_data()
        .code_stubs
        .lock()
        .unwrap()
        .insert(address, CodeStub { lib, name, handler });
}
