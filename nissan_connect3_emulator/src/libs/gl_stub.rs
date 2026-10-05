use crate::emulator::context::Context;
use crate::os::code_stub::{add_code_stub, CodeStubHandler};
use std::sync::atomic::{AtomicU32, Ordering};
use unicorn_engine::{RegisterARM, Unicorn};

static NEXT_OBJECT_ID: AtomicU32 = AtomicU32::new(0x1001);
static NEXT_ATTR_LOCATION: AtomicU32 = AtomicU32::new(0);
static NEXT_UNIFORM_LOCATION: AtomicU32 = AtomicU32::new(1);

fn reg(unicorn: &mut Unicorn<'_, Context>, register: RegisterARM) -> u32 {
    unicorn.reg_read(register).unwrap_or(0) as u32
}

fn read_u32(unicorn: &mut Unicorn<'_, Context>, address: u32) -> u32 {
    if address == 0 {
        return 0;
    }
    let mut buf = [0u8; 4];
    match unicorn.mem_read(address as u64, &mut buf) {
        Ok(()) => u32::from_le_bytes(buf),
        Err(_) => 0,
    }
}

fn write_u32(unicorn: &mut Unicorn<'_, Context>, address: u32, value: u32) {
    if address != 0 {
        let _ = unicorn.mem_write(address as u64, &value.to_le_bytes());
    }
}

fn next_id() -> u32 {
    let id = NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        NEXT_OBJECT_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

fn stub_zero(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0
}

fn stub_one(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    1
}

fn stub_object(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    next_id()
}

fn stub_attr_location(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    NEXT_ATTR_LOCATION.fetch_add(1, Ordering::Relaxed) & 0x0f
}

fn stub_uniform_location(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    NEXT_UNIFORM_LOCATION.fetch_add(1, Ordering::Relaxed) & 0x0f
}

fn stub_gen_ids(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let count = reg(unicorn, RegisterARM::R0).min(1024);
    let out = reg(unicorn, RegisterARM::R1);
    for i in 0..count {
        write_u32(unicorn, out + i * 4, next_id());
    }
    0
}

fn stub_get_status(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let pname = reg(unicorn, RegisterARM::R1);
    let out = reg(unicorn, RegisterARM::R2);
    let value = match pname {
        0x8b84 => 0,
        _ => 1,
    };
    write_u32(unicorn, out, value);
    0
}

fn stub_get_integerv(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let pname = reg(unicorn, RegisterARM::R0);
    let out = reg(unicorn, RegisterARM::R1);
    let value = match pname {
        0x0d33 | 0x84e8 | 0x851c | 0x8ca6 => 4096,
        _ => 16,
    };
    write_u32(unicorn, out, value);
    0
}

fn stub_framebuffer_complete(_unicorn: &mut Unicorn<'_, Context>) -> u32 {
    0x8cd5
}

fn egl_initialize(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let major = reg(unicorn, RegisterARM::R1);
    let minor = reg(unicorn, RegisterARM::R2);
    write_u32(unicorn, major, 1);
    write_u32(unicorn, minor, 4);
    1
}

fn egl_choose_config(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let configs = reg(unicorn, RegisterARM::R2);
    let config_size = reg(unicorn, RegisterARM::R3);
    let sp = reg(unicorn, RegisterARM::SP);
    let num_config = read_u32(unicorn, sp);

    if configs != 0 && config_size != 0 {
        write_u32(unicorn, configs, 1);
    }
    write_u32(unicorn, num_config, 1);
    1
}

fn add_stub(
    unicorn: &mut Unicorn<'_, Context>,
    lib: &'static str,
    base_address: u32,
    offset: u32,
    name: &'static str,
    handler: CodeStubHandler,
) {
    add_code_stub(unicorn, lib, base_address + offset, name, handler);
}

pub fn libegl_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let lib = "/usr/lib/libEGL.so";
    let one: [(u32, &str); 21] = [
        (0x176c, "eglReleaseThread"),
        (0x1770, "eglWaitClient"),
        (0x1774, "eglQueryAPI"),
        (0x1778, "eglBindAPI"),
        (0x1780, "eglReleaseTexImage"),
        (0x1784, "eglBindTexImage"),
        (0x1788, "eglSurfaceAttrib"),
        (0x178c, "eglSwapInterval"),
        (0x1790, "eglCopyBuffers"),
        (0x1794, "eglSwapBuffers"),
        (0x1798, "eglWaitNative"),
        (0x179c, "eglWaitGL"),
        (0x17a0, "eglQueryContext"),
        (0x17b0, "eglMakeCurrent"),
        (0x17b4, "eglDestroyContext"),
        (0x17bc, "eglQuerySurface"),
        (0x17c0, "eglDestroySurface"),
        (0x17d0, "eglGetConfigAttrib"),
        (0x17d8, "eglGetConfigs"),
        (0x17e4, "eglTerminate"),
        (0x17f4, "EGLCloseWindow"),
    ];
    for (offset, name) in one {
        add_stub(unicorn, lib, base_address, offset, name, stub_one);
    }

    let object: [(u32, &str); 5] = [
        (
            0x177c,
            "eglCreatePbufferFromClientBuffer",
        ),
        (0x17ac, "eglGetCurrentContext"),
        (0x17b8, "eglCreateContext"),
        (0x17c4, "eglCreatePbufferSurface"),
        (0x17cc, "eglCreateWindowSurface"),
    ];
    for (offset, name) in object {
        add_stub(unicorn, lib, base_address, offset, name, stub_object);
    }

    let display: [(u32, &str); 2] = [
        (0x17a4, "eglGetCurrentDisplay"),
        (0x17ec, "eglGetDisplay"),
    ];
    for (offset, name) in display {
        add_stub(unicorn, lib, base_address, offset, name, stub_object);
    }

    add_stub(
        unicorn,
        lib,
        base_address,
        0x17a8,
        "eglGetCurrentSurface",
        stub_object,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x17dc,
        "eglGetProcAddress",
        stub_zero,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x17e0,
        "eglQueryString",
        stub_zero,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x17f0,
        "eglGetError",
        stub_zero,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x17e8,
        "eglInitialize",
        egl_initialize,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x17d4,
        "eglChooseConfig",
        egl_choose_config,
    );
}

pub fn libgles2_add_code_hooks(unicorn: &mut Unicorn<'_, Context>, base_address: u32) {
    let lib = "/usr/lib/libGLESv2.so";

    let zero: [(u32, &str); 2] = [
        (0x179a8, "glGetError"),
        (0x17fa0, "glGetString"),
    ];
    for (offset, name) in zero {
        add_stub(unicorn, lib, base_address, offset, name, stub_zero);
    }

    let ids: [(u32, &str); 4] = [
        (0x2d45c, "glGenTextures"),
        (0x14bb4, "glGenFramebuffers"),
        (0x14c44, "glGenRenderbuffers"),
        (0x0b6ac, "glGenBuffers"),
    ];
    for (offset, name) in ids {
        add_stub(unicorn, lib, base_address, offset, name, stub_gen_ids);
    }

    let object: [(u32, &str); 2] = [
        (0x24dac, "glCreateShader"),
        (0x24ed8, "glCreateProgram"),
    ];
    for (offset, name) in object {
        add_stub(unicorn, lib, base_address, offset, name, stub_object);
    }

    let status: [(u32, &str); 4] = [
        (0x18dbc, "glGetShaderiv"),
        (0x191c4, "glGetProgramiv"),
        (0x1a6a0, "glGetBooleanv"),
        (0x1a63c, "glGetFloatv"),
    ];
    for (offset, name) in status {
        add_stub(unicorn, lib, base_address, offset, name, stub_get_status);
    }

    add_stub(
        unicorn,
        lib,
        base_address,
        0x1a5d8,
        "glGetIntegerv",
        stub_get_integerv,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x17840,
        "glCheckFramebufferStatus",
        stub_framebuffer_complete,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x18aec,
        "glGetAttribLocation",
        stub_attr_location,
    );
    add_stub(
        unicorn,
        lib,
        base_address,
        0x36ea4,
        "glGetUniformLocation",
        stub_uniform_location,
    );

    let void: [(u32, &str); 38] = [
        (0x218dc, "glAttachShader"),
        (0x26f74, "glBlendEquation"),
        (0x26d78, "glBlendFunc"),
        (0x26de4, "glBlendFuncSeparate"),
        (0x21650, "glUseProgram"),
        (0x0c6f8, "glClear"),
        (0x0bcbc, "glClearColor"),
        (0x2173c, "glDeleteProgram"),
        (0x216a8, "glDeleteShader"),
        (0x2d6a0, "glDeleteTextures"),
        (0x1593c, "glDeleteFramebuffers"),
        (0x268bc, "glDisable"),
        (0x3db5c, "glDisableVertexAttribArray"),
        (0x10770, "glDrawArrays"),
        (0x26740, "glEnable"),
        (0x3dbf0, "glEnableVertexAttribArray"),
        (0x1c894, "glFinish"),
        (0x152d8, "glFramebufferTexture2D"),
        (0x15a64, "glBindFramebuffer"),
        (0x15eb4, "glBindRenderbuffer"),
        (0x16660, "glRenderbufferStorage"),
        (0x2d804, "glBindTexture"),
        (0x0b9bc, "glBindBuffer"),
        (0x0b420, "glBufferData"),
        (0x2dda0, "glTexParameteri"),
        (0x2cbc0, "glTexImage2D"),
        (0x23578, "glLinkProgram"),
        (0x23a90, "glShaderBinary"),
        (0x23c10, "glShaderSource"),
        (0x244f0, "glCompileShader"),
        (0x1e0cc, "glReadPixels"),
        (0x1e5e0, "glScissor"),
        (0x3dc84, "glVertexAttribPointer"),
        (0x36d4c, "glUniform1i"),
        (0x365c8, "glUniform4f"),
        (0x367fc, "glUniform2f"),
        (0x366e8, "glUniform3f"),
        (0x28f28, "glActiveTexture"),
    ];
    for (offset, name) in void {
        add_stub(unicorn, lib, base_address, offset, name, stub_zero);
    }
}