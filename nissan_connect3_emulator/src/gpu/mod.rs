use crate::emulator::context::Context;
use sdl2::event::Event;
use sdl2::video::{GLContext, Window};
use sdl2::EventPump;
use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};
use unicorn_engine::{RegisterARM, Unicorn};

const GL_INVALID_ENUM: u32 = 0x0500;
const GL_INFO_LOG_LENGTH: u32 = 0x8b84;

const EGL_VENDOR: u32 = 0x3053;
const EGL_VERSION: u32 = 0x3054;
const EGL_EXTENSIONS: u32 = 0x3055;
const EGL_CLIENT_APIS: u32 = 0x308d;
const EGL_OPENGL_ES_API: u32 = 0x30a0;
const EGL_OPENVG_API: u32 = 0x30a1;
const EGL_OPENGL_API: u32 = 0x30a2;
const MAX_TEXTURE_DIM: i32 = 4096;
const MAX_TEXTURE_BYTES: usize = 32 * 1024 * 1024;

static NEXT_FALLBACK_ID: AtomicU32 = AtomicU32::new(0x1001);

thread_local! {
    static BACKEND: RefCell<Option<Backend>> = const { RefCell::new(None) };
    static PENDING_GL_ERROR: Cell<bool> = const { Cell::new(false) };
    static BACKEND_INIT_FAILED: Cell<bool> = const { Cell::new(false) };
    static API_LOG_COUNT: Cell<u32> = const { Cell::new(0) };
    static EGL_CURRENT_API: Cell<u32> = const { Cell::new(EGL_OPENGL_ES_API) };
}

struct Backend {
    _sdl: sdl2::Sdl,
    _video: sdl2::VideoSubsystem,
    _gl_context: GLContext,
    window: Window,
    events: EventPump,
}

fn init_sdl_backend() -> Result<Backend, String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;

    {
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::GLES);
        attrs.set_context_version(2, 0);
    }

    let window = video
        .window("Nissan Connect 3 HMI", 800, 480)
        .position_centered()
        .resizable()
        .opengl()
        .build()
        .map_err(|err| format!("{:?}", err))?;

    let gl_context = window.gl_create_context()?;
    gl::load_with(|name| video.gl_get_proc_address(name) as *const _);
    let _ = video.gl_set_swap_interval(1);
    let events = sdl.event_pump()?;

    unsafe {
        gl::Viewport(0, 0, 800, 480);
        gl::ClearColor(0.0, 0.0, 0.0, 1.0);
        gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
    }
    let _ = window.gl_swap_window();

    Ok(Backend {
        _sdl: sdl,
        _video: video,
        _gl_context: gl_context,
        window,
        events,
    })
}

pub fn tick(unicorn: &Unicorn<'_, Context>) {
    if !gpu_process_allowed(&unicorn.get_data().elf_path) {
        return;
    }

    BACKEND.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            if BACKEND_INIT_FAILED.with(|failed| failed.get()) {
                return;
            }
            match init_sdl_backend() {
                Ok(backend) => *slot = Some(backend),
                Err(err) => {
                    log::warn!(
                        "GPU: SDL/GL backend unavailable, using null backend: {}",
                        err
                    );
                    BACKEND_INIT_FAILED.with(|failed| failed.set(true));
                    return;
                }
            }
        }

        let backend = match slot.as_mut() {
            Some(backend) => backend,
            None => return,
        };

        while let Some(event) = backend.events.poll_event() {
            match event {
                Event::Quit { .. } => {
                    std::process::exit(0);
                }
                Event::Window { win_event, .. } => {
                    if let sdl2::event::WindowEvent::Resized(width, height) = win_event {
                        if width > 0 && height > 0 {
                            unsafe {
                                gl::Viewport(0, 0, width, height);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    });
}

fn gpu_process_allowed(elf_path: &str) -> bool {
    let Ok(filter) = std::env::var("EMU_GPU_PROCESS_FILTER") else {
        return true;
    };
    if filter.trim().is_empty() {
        return true;
    }

    filter
        .split(',')
        .map(|needle| needle.trim())
        .filter(|needle| !needle.is_empty())
        .any(|needle| elf_path.contains(needle))
}

fn ensure_backend(unicorn: &Unicorn<'_, Context>) -> bool {
    if !gpu_process_allowed(&unicorn.get_data().elf_path) {
        return false;
    }

    BACKEND.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return true;
        }
        if BACKEND_INIT_FAILED.with(|failed| failed.get()) {
            return false;
        }
        match init_sdl_backend() {
            Ok(backend) => {
                *slot = Some(backend);
                true
            }
            Err(err) => {
                log::warn!(
                    "GPU: SDL/GL backend unavailable, using null backend: {}",
                    err
                );
                BACKEND_INIT_FAILED.with(|failed| failed.set(true));
                false
            }
        }
    })
}

fn backend_ready(unicorn: &Unicorn<'_, Context>) -> bool {
    ensure_backend(unicorn)
}

fn with_backend<T>(
    unicorn: &Unicorn<'_, Context>,
    f: impl FnOnce(&mut Backend) -> T,
) -> Option<T> {
    if !ensure_backend(unicorn) {
        return None;
    }
    BACKEND.with(|slot| slot.borrow_mut().as_mut().map(|backend| f(backend)))
}

fn ureg(unicorn: &mut Unicorn<'_, Context>, register: RegisterARM) -> u32 {
    unicorn.reg_read(register).unwrap_or(0) as u32
}

fn freg(unicorn: &mut Unicorn<'_, Context>, register: RegisterARM) -> f32 {
    f32::from_bits(ureg(unicorn, register))
}

fn fstack(unicorn: &mut Unicorn<'_, Context>, index: usize) -> f32 {
    f32::from_bits(stack_arg(unicorn, index))
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

fn stack_arg(unicorn: &mut Unicorn<'_, Context>, index: usize) -> u32 {
    let sp = ureg(unicorn, RegisterARM::SP);
    read_u32(unicorn, sp + (index as u32 * 4))
}

fn read_bytes(unicorn: &mut Unicorn<'_, Context>, address: u32, len: usize) -> Option<Vec<u8>> {
    if address == 0 || len == 0 {
        return if len == 0 { Some(Vec::new()) } else { None };
    }
    if len > MAX_TEXTURE_BYTES {
        return None;
    }
    let mut data = vec![0u8; len];
    unicorn.mem_read(address as u64, &mut data).ok()?;
    Some(data)
}

fn read_best_effort_bytes(
    unicorn: &mut Unicorn<'_, Context>,
    address: u32,
    requested: usize,
) -> Option<(u32, Vec<u8>)> {
    if address == 0 {
        return None;
    }
    let mut len = requested.min(MAX_TEXTURE_BYTES).max(4);
    while len >= 4 {
        if let Some(data) = read_bytes(unicorn, address, len) {
            return Some((len as u32, data));
        }
        len /= 2;
    }
    None
}

fn read_f32_array(unicorn: &mut Unicorn<'_, Context>, address: u32, count: usize) -> Vec<f32> {
    read_bytes(unicorn, address, count * 4)
        .unwrap_or_default()
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap_or([0; 4])))
        .collect()
}

fn read_i32_array(unicorn: &mut Unicorn<'_, Context>, address: u32, count: usize) -> Vec<i32> {
    read_bytes(unicorn, address, count * 4)
        .unwrap_or_default()
        .chunks_exact(4)
        .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap_or([0; 4])))
        .collect()
}

fn read_u32_array(unicorn: &mut Unicorn<'_, Context>, address: u32, count: usize) -> Vec<u32> {
    read_bytes(unicorn, address, count * 4)
        .unwrap_or_default()
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap_or([0; 4])))
        .collect()
}

fn read_guest_cstr(unicorn: &mut Unicorn<'_, Context>, address: u32, max_len: usize) -> String {
    if address == 0 {
        return String::new();
    }

    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    for offset in 0..max_len {
        if unicorn
            .mem_read((address + offset as u32) as u64, &mut byte)
            .is_err()
        {
            break;
        }
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }

    String::from_utf8_lossy(&bytes).to_string()
}

fn next_fallback_id() -> u32 {
    let id = NEXT_FALLBACK_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        NEXT_FALLBACK_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

fn alloc_guest_bytes(unicorn: &mut Unicorn<'_, Context>, len: usize) -> u32 {
    let len = len.max(1);
    let mmu = unicorn.get_data().mmu.clone();
    let flags = {
        use unicorn_engine::unicorn_const::Prot;
        Prot::READ | Prot::WRITE
    };
    let addr = mmu
        .lock()
        .unwrap()
        .heap_alloc(unicorn, len as u32, flags, "[gpu-string]");
    addr
}

fn alloc_write_guest_bytes(unicorn: &mut Unicorn<'_, Context>, data: &[u8]) -> u32 {
    let mut bytes = data.to_vec();
    bytes.push(0);
    let addr = alloc_guest_bytes(unicorn, bytes.len());
    if addr != 0 {
        let _ = unicorn.mem_write(addr as u64, &bytes);
    }
    addr
}

fn alloc_write_guest_cstr(unicorn: &mut Unicorn<'_, Context>, value: &str) -> u32 {
    alloc_write_guest_bytes(unicorn, value.as_bytes())
}

fn host_cstr_to_string(ptr: *const std::os::raw::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(ptr).to_string_lossy().to_string() }
}

fn clear_host_gl_errors(unicorn: &Unicorn<'_, Context>) {
    if !backend_ready(unicorn) {
        return;
    }
    unsafe { while gl::GetError() != 0 {} }
}

fn force_gl_error(unicorn: &Unicorn<'_, Context>) {
    PENDING_GL_ERROR.with(|flag| flag.set(true));
    clear_host_gl_errors(unicorn);
}

fn log_api(unicorn: &mut Unicorn<'_, Context>, prefix: &str, name: &str) {
    let count = API_LOG_COUNT.with(|count| {
        let old = count.get();
        count.set(old.wrapping_add(1));
        old
    });
    if count < 3000 {
        log::info!(
            "GPU: {} {} r0={:x} r1={:x} r2={:x} r3={:x}",
            prefix,
            name,
            ureg(unicorn, RegisterARM::R0),
            ureg(unicorn, RegisterARM::R1),
            ureg(unicorn, RegisterARM::R2),
            ureg(unicorn, RegisterARM::R3)
        );
    }
}

pub fn egl_api(unicorn: &mut Unicorn<'_, Context>, name: &str) -> u32 {
    log_api(unicorn, "EGL", name);
    match name {
        "eglGetDisplay" | "eglGetCurrentDisplay" => return 1,
        "eglInitialize" => {
            let major = ureg(unicorn, RegisterARM::R1);
            let minor = ureg(unicorn, RegisterARM::R2);
            write_u32(unicorn, major, 1);
            write_u32(unicorn, minor, 4);
            return 1;
        }
        "eglBindAPI" => {
            let api = ureg(unicorn, RegisterARM::R0);
            if matches!(api, EGL_OPENGL_ES_API | EGL_OPENVG_API | EGL_OPENGL_API) {
                EGL_CURRENT_API.with(|slot| slot.set(api));
            }
            return 1;
        }
        "eglQueryAPI" => {
            return EGL_CURRENT_API.get();
        }
        "eglQuerySurface" => {
            let attr = ureg(unicorn, RegisterARM::R2);
            let out = ureg(unicorn, RegisterARM::R3);
            let value = match attr {
                0x3057 | 0x305d => 800,
                0x3056 | 0x305e => 480,
                0x305f => 0,
                _ => 1,
            };
            write_u32(unicorn, out, value);
            return 1;
        }
        "eglMakeCurrent"
        | "eglDestroyContext"
        | "eglDestroySurface"
        | "eglTerminate"
        | "eglReleaseThread"
        | "eglWaitClient"
        | "eglWaitGL"
        | "eglWaitNative"
        | "eglReleaseTexImage"
        | "eglBindTexImage"
        | "eglSurfaceAttrib"
        | "eglSwapInterval"
        | "eglCopyBuffers"
        | "eglQueryContext"
        | "eglCreatePbufferFromClientBuffer"
        | "eglCreatePixmapSurface" => {
            return 1;
        }
        "eglGetConfigs" => {
            let configs = ureg(unicorn, RegisterARM::R1);
            let max_count = ureg(unicorn, RegisterARM::R2);
            let num_config = ureg(unicorn, RegisterARM::R3);
            let count = if max_count == 0 { 0 } else { 1 };

            write_u32(unicorn, num_config, count);
            if configs != 0 && count != 0 {
                write_u32(unicorn, configs, 1);
            }
            return 1;
        }
        "eglGetConfigAttrib" => {
            let attribute = ureg(unicorn, RegisterARM::R2);
            let out = ureg(unicorn, RegisterARM::R3);
            let value = match attribute {
                0x3020 => 32,
                0x3021 => 8,
                0x3022 => 8,
                0x3023 => 8,
                0x3024 => 8,
                0x3025 => 0,
                0x3026 => 0,
                0x3027 => 0x3038,
                0x3028 => 1,
                0x3029 => 0,
                0x302a | 0x302c => 4096,
                0x302b => 4096 * 4096,
                0x302d => 1,
                0x302e | 0x302f => 1,
                0x3030 => 1,
                0x3031 => 1,
                0x3032 => 0,
                0x3033 => 1,
                0x3036 => 0x3022,
                0x3037 => 4,
                0x303d | 0x3040 => 4096,
                _ => 0,
            };

            write_u32(unicorn, out, value);
            return 1;
        }
        "eglSwapBuffers" => {
            if with_backend(unicorn, |backend| {
                unsafe {
                    gl::Finish();
                }
                let _ = backend.window.gl_swap_window();
                while let Some(event) = backend.events.poll_event() {
                    if let Event::Quit { .. } = event {
                        std::process::exit(0);
                    }
                }
            })
            .is_some()
            {
                return 1;
            }
            return 1;
        }
        "eglGetError" => return 0,
        "eglQueryString" => {
            let name = ureg(unicorn, RegisterARM::R1);
            let value = match name {
                EGL_VENDOR => "Emulator",
                EGL_VERSION => "1.4",
                EGL_EXTENSIONS => "",
                EGL_CLIENT_APIS => "OpenGL_ES",
                _ => "",
            };
            return alloc_write_guest_cstr(unicorn, value);
        }
        "eglGetProcAddress" => return 1,
        "eglChooseConfig" => {
            let configs = ureg(unicorn, RegisterARM::R2);
            let config_size = ureg(unicorn, RegisterARM::R3);
            let num_config = stack_arg(unicorn, 0);
            if configs != 0 && config_size != 0 {
                write_u32(unicorn, configs, 1);
            }
            write_u32(unicorn, num_config, 1);
            return 1;
        }
        "eglCreateWindowSurface" => {
            if backend_ready(unicorn) {
                clear_host_gl_errors(unicorn);
                return 2;
            }
            return next_fallback_id();
        }
        "eglCreateContext" | "eglCreatePbufferSurface" => {
            return next_fallback_id();
        }
        _ => {}
    }

    next_fallback_id()
}

pub fn gl_api(unicorn: &mut Unicorn<'_, Context>, name: &str) -> u32 {
    log_api(unicorn, "GL", name);
    let ready = backend_ready(unicorn);

    if name == "glShaderBinary" {
        force_gl_error(unicorn);
        return 0;
    }

    if name == "glGetError" {
        let pending = PENDING_GL_ERROR.with(|flag| flag.replace(false));
        if pending {
            clear_host_gl_errors(unicorn);
            return GL_INVALID_ENUM;
        }
        if ready {
            let err = unsafe { gl::GetError() } as u32;
            if err != 0 {
                return err;
            }
            while unsafe { gl::GetError() } != 0 {}
        }
        return 0;
    }

    if !ready {
        return gl_fallback(unicorn, name);
    }

    unsafe {
        match name {
            "glActiveTexture" => gl::ActiveTexture(ureg(unicorn, RegisterARM::R0)),
            "glAttachShader" => gl::AttachShader(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBindAttribLocation" => {
                let program = ureg(unicorn, RegisterARM::R0);
                let index = ureg(unicorn, RegisterARM::R1);
                let name_ptr = ureg(unicorn, RegisterARM::R2);
                let name = read_guest_cstr(unicorn, name_ptr, 128);
                let cname = CString::new(name).unwrap_or_else(|_| CString::new("").unwrap());
                gl::BindAttribLocation(program, index, cname.as_ptr() as *const _);
            }
            "glBindBuffer" => gl::BindBuffer(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBindFramebuffer" => gl::BindFramebuffer(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBindRenderbuffer" => gl::BindRenderbuffer(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBindTexture" => gl::BindTexture(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBlendColor" => {
                let values = [
                    freg(unicorn, RegisterARM::R0),
                    freg(unicorn, RegisterARM::R1),
                    freg(unicorn, RegisterARM::R2),
                    freg(unicorn, RegisterARM::R3),
                ];
                gl::BlendColor(values[0], values[1], values[2], values[3]);
            }
            "glBlendEquation" => gl::BlendEquation(ureg(unicorn, RegisterARM::R0)),
            "glBlendEquationSeparate" => gl::BlendEquationSeparate(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBlendFunc" => gl::BlendFunc(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glBlendFuncSeparate" => gl::BlendFuncSeparate(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2),
                ureg(unicorn, RegisterARM::R3),
            ),
            "glBufferData" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let size = (ureg(unicorn, RegisterARM::R1) as usize).min(MAX_TEXTURE_BYTES);
                let data_addr = ureg(unicorn, RegisterARM::R2);
                let data = if data_addr == 0 || size == 0 {
                    Vec::new()
                } else {
                    read_best_effort_bytes(unicorn, data_addr, size)
                        .map(|(_, data)| data)
                        .unwrap_or_default()
                };
                gl::BufferData(
                    target,
                    size as isize,
                    if data.is_empty() {
                        std::ptr::null()
                    } else {
                        data.as_ptr() as *const _
                    },
                    gl::STATIC_DRAW,
                );
            }
            "glBufferSubData" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let offset = ureg(unicorn, RegisterARM::R1) as isize;
                let size = (ureg(unicorn, RegisterARM::R2) as usize).min(MAX_TEXTURE_BYTES);
                let data_addr = ureg(unicorn, RegisterARM::R3);
                let data = read_best_effort_bytes(unicorn, data_addr, size)
                    .map(|(_, data)| data)
                    .unwrap_or_default();
                gl::BufferSubData(
                    target,
                    offset,
                    size as isize,
                    if data.is_empty() {
                        std::ptr::null()
                    } else {
                        data.as_ptr() as *const _
                    },
                );
            }
            "glCheckFramebufferStatus" => {
                return gl::CheckFramebufferStatus(ureg(unicorn, RegisterARM::R0)) as u32
            }
            "glClear" => gl::Clear(ureg(unicorn, RegisterARM::R0)),
            "glClearColor" => {
                let values = [
                    freg(unicorn, RegisterARM::R0),
                    freg(unicorn, RegisterARM::R1),
                    freg(unicorn, RegisterARM::R2),
                    freg(unicorn, RegisterARM::R3),
                ];
                gl::ClearColor(values[0], values[1], values[2], values[3]);
            }
            "glClearDepthf" => gl::ClearDepthf(freg(unicorn, RegisterARM::R0)),
            "glClearStencil" => gl::ClearStencil(ureg(unicorn, RegisterARM::R0) as i32),
            "glColorMask" => gl::ColorMask(
                ureg(unicorn, RegisterARM::R0) as u8,
                ureg(unicorn, RegisterARM::R1) as u8,
                ureg(unicorn, RegisterARM::R2) as u8,
                ureg(unicorn, RegisterARM::R3) as u8,
            ),
            "glCompileShader" => gl::CompileShader(ureg(unicorn, RegisterARM::R0)),
            "glCreateProgram" => return gl::CreateProgram(),
            "glCreateShader" => return gl::CreateShader(ureg(unicorn, RegisterARM::R0)),
            "glCullFace" => gl::CullFace(ureg(unicorn, RegisterARM::R0)),
            "glDeleteBuffers" => {
                let count = ureg(unicorn, RegisterARM::R0) as i32;
                let ptr = ureg(unicorn, RegisterARM::R1);
                let values = read_u32_array(unicorn, ptr, count as usize);
                gl::DeleteBuffers(count, values.as_ptr());
            }
            "glDeleteFramebuffers" => {
                let count = ureg(unicorn, RegisterARM::R0) as i32;
                let ptr = ureg(unicorn, RegisterARM::R1);
                let values = read_u32_array(unicorn, ptr, count as usize);
                gl::DeleteFramebuffers(count, values.as_ptr());
            }
            "glDeleteProgram" => gl::DeleteProgram(ureg(unicorn, RegisterARM::R0)),
            "glDeleteRenderbuffers" => {
                let count = ureg(unicorn, RegisterARM::R0) as i32;
                let ptr = ureg(unicorn, RegisterARM::R1);
                let values = read_u32_array(unicorn, ptr, count as usize);
                gl::DeleteRenderbuffers(count, values.as_ptr());
            }
            "glDeleteShader" => gl::DeleteShader(ureg(unicorn, RegisterARM::R0)),
            "glDeleteTextures" => {
                let count = ureg(unicorn, RegisterARM::R0) as i32;
                let ptr = ureg(unicorn, RegisterARM::R1);
                let values = read_u32_array(unicorn, ptr, count as usize);
                gl::DeleteTextures(count, values.as_ptr());
            }
            "glDepthFunc" => gl::DepthFunc(ureg(unicorn, RegisterARM::R0)),
            "glDepthMask" => gl::DepthMask(ureg(unicorn, RegisterARM::R0) as u8),
            "glDepthRangef" => gl::DepthRangef(
                freg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
            ),
            "glDetachShader" => gl::DetachShader(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glDisable" => gl::Disable(ureg(unicorn, RegisterARM::R0)),
            "glDisableVertexAttribArray" => {
                gl::DisableVertexAttribArray(ureg(unicorn, RegisterARM::R0))
            }
            "glDrawArrays" => gl::DrawArrays(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
            ),
            "glDrawElements" => gl::DrawElements(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2),
                ureg(unicorn, RegisterARM::R3) as *const _,
            ),
            "glEnable" => gl::Enable(ureg(unicorn, RegisterARM::R0)),
            "glEnableVertexAttribArray" => {
                gl::EnableVertexAttribArray(ureg(unicorn, RegisterARM::R0))
            }
            "glFinish" => gl::Finish(),
            "glFlush" => gl::Flush(),
            "glFramebufferRenderbuffer" => gl::FramebufferRenderbuffer(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2),
                ureg(unicorn, RegisterARM::R3),
            ),
            "glFramebufferTexture2D" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let attachment = ureg(unicorn, RegisterARM::R1);
                let textarget = ureg(unicorn, RegisterARM::R2);
                let texture = ureg(unicorn, RegisterARM::R3);
                let level = stack_arg(unicorn, 0) as i32;
                gl::FramebufferTexture2D(target, attachment, textarget, texture, level);
            }
            "glFrontFace" => gl::FrontFace(ureg(unicorn, RegisterARM::R0)),
            "glGenBuffers" => gen_ids(unicorn, GenKind::Buffer),
            "glGenerateMipmap" => gl::GenerateMipmap(ureg(unicorn, RegisterARM::R0)),
            "glGenFramebuffers" => gen_ids(unicorn, GenKind::Framebuffer),
            "glGenRenderbuffers" => gen_ids(unicorn, GenKind::Renderbuffer),
            "glGenTextures" => gen_ids(unicorn, GenKind::Texture),
            "glGetAttribLocation" => {
                let program = ureg(unicorn, RegisterARM::R0);
                let addr = ureg(unicorn, RegisterARM::R1);
                let name = read_guest_cstr(unicorn, addr, 128);
                let cname = CString::new(name).unwrap_or_else(|_| CString::new("").unwrap());
                return gl::GetAttribLocation(program, cname.as_ptr()) as u32;
            }
            "glGetError" => unreachable!(),
            "glGetIntegerv" => {
                let pname = ureg(unicorn, RegisterARM::R0);
                let out = ureg(unicorn, RegisterARM::R1);
                let mut value = 0i32;
                gl::GetIntegerv(pname, &mut value);
                write_u32(unicorn, out, value as u32);
            }
            "glGetProgramInfoLog" => {
                let program = ureg(unicorn, RegisterARM::R0);
                let buf_size = ureg(unicorn, RegisterARM::R1) as i32;
                let len_out = ureg(unicorn, RegisterARM::R2);
                let buf = ureg(unicorn, RegisterARM::R3);
                if buf_size > 0 && buf != 0 {
                    let mut log = vec![0u8; buf_size as usize];
                    let mut len = 0i32;
                    gl::GetProgramInfoLog(program, buf_size, &mut len, log.as_mut_ptr() as *mut _);
                    let copy_len = (len.max(0) as usize).min(buf_size as usize);
                    let _ = unicorn.mem_write(buf as u64, &log[..copy_len]);
                    write_u32(unicorn, len_out, len.max(0) as u32);
                }
            }
            "glGetProgramiv" => {
                let program = ureg(unicorn, RegisterARM::R0);
                let pname = ureg(unicorn, RegisterARM::R1);
                let out = ureg(unicorn, RegisterARM::R2);
                let mut value = 0i32;
                gl::GetProgramiv(program, pname, &mut value);
                if pname == GL_INFO_LOG_LENGTH && value <= 0 {
                    value = 1;
                }
                write_u32(unicorn, out, value as u32);
            }
            "glGetShaderInfoLog" => {
                let shader = ureg(unicorn, RegisterARM::R0);
                let buf_size = ureg(unicorn, RegisterARM::R1) as i32;
                let len_out = ureg(unicorn, RegisterARM::R2);
                let buf = ureg(unicorn, RegisterARM::R3);
                if buf_size > 0 && buf != 0 {
                    let mut log = vec![0u8; buf_size as usize];
                    let mut len = 0i32;
                    gl::GetShaderInfoLog(shader, buf_size, &mut len, log.as_mut_ptr() as *mut _);
                    let copy_len = (len.max(0) as usize).min(buf_size as usize);
                    let _ = unicorn.mem_write(buf as u64, &log[..copy_len]);
                    write_u32(unicorn, len_out, len.max(0) as u32);
                }
            }
            "glGetShaderiv" => {
                let shader = ureg(unicorn, RegisterARM::R0);
                let pname = ureg(unicorn, RegisterARM::R1);
                let out = ureg(unicorn, RegisterARM::R2);
                let mut value = 0i32;
                gl::GetShaderiv(shader, pname, &mut value);
                if pname == GL_INFO_LOG_LENGTH && value <= 0 {
                    value = 1;
                }
                write_u32(unicorn, out, value as u32);
            }
            "glGetString" => {
                let name = ureg(unicorn, RegisterARM::R0);
                let ptr = gl::GetString(name);
                let mut text = host_cstr_to_string(ptr as *const _);
                if name == gl::EXTENSIONS {
                    text = suppress_unstubbed_gl_extensions(&text);
                }
                return alloc_write_guest_cstr(unicorn, &text);
            }
            "glGetUniformLocation" => {
                let program = ureg(unicorn, RegisterARM::R0);
                let addr = ureg(unicorn, RegisterARM::R1);
                let name = read_guest_cstr(unicorn, addr, 128);
                let cname = CString::new(name).unwrap_or_else(|_| CString::new("").unwrap());
                return gl::GetUniformLocation(program, cname.as_ptr()) as u32;
            }
            "glHint" => gl::Hint(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            ),
            "glLinkProgram" => gl::LinkProgram(ureg(unicorn, RegisterARM::R0)),
            "glPixelStorei" => gl::PixelStorei(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1) as i32,
            ),
            "glPolygonOffset" => gl::PolygonOffset(
                freg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
            ),
            "glReadPixels" => {
                let x = ureg(unicorn, RegisterARM::R0) as i32;
                let y = ureg(unicorn, RegisterARM::R1) as i32;
                let width = ureg(unicorn, RegisterARM::R2) as i32;
                let height = ureg(unicorn, RegisterARM::R3) as i32;
                let format = stack_arg(unicorn, 0);
                let kind = stack_arg(unicorn, 1);
                let out = stack_arg(unicorn, 2);
                let size = (width.max(0) as usize)
                    .saturating_mul(height.max(0) as usize)
                    .saturating_mul(4);
                let mut data = vec![0u8; size];
                if !data.is_empty() {
                    gl::ReadPixels(
                        x,
                        y,
                        width,
                        height,
                        format,
                        kind,
                        data.as_mut_ptr() as *mut _,
                    );
                    let _ = unicorn.mem_write(out as u64, &data);
                }
            }
            "glRenderbufferStorage" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let internal = ureg(unicorn, RegisterARM::R1);
                let width = ureg(unicorn, RegisterARM::R2) as i32;
                let height = ureg(unicorn, RegisterARM::R3) as i32;
                gl::RenderbufferStorage(target, internal, width, height);
            }
            "glScissor" => gl::Scissor(
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            ),
            "glShaderSource" => {
                let shader = ureg(unicorn, RegisterARM::R0);
                let count = (ureg(unicorn, RegisterARM::R1) as usize).min(64);
                let strings_ptr = ureg(unicorn, RegisterARM::R2);
                let lengths_ptr = ureg(unicorn, RegisterARM::R3);
                let mut strings = Vec::new();
                for index in 0..count {
                    let src_ptr = read_u32(unicorn, strings_ptr + (index as u32 * 4));
                    let len = if lengths_ptr != 0 {
                        let len = read_u32(unicorn, lengths_ptr + (index as u32 * 4));
                        if len == 0 || len == u32::MAX {
                            None
                        } else {
                            Some(len as usize)
                        }
                    } else {
                        None
                    };
                    let s = if let Some(len) = len {
                        let bytes = read_bytes(unicorn, src_ptr, len).unwrap_or_default();
                        String::from_utf8_lossy(&bytes).to_string()
                    } else {
                        read_guest_cstr(unicorn, src_ptr, 64 * 1024)
                    };
                    strings.push(CString::new(s).unwrap_or_else(|_| CString::new("").unwrap()));
                }
                strings = strings
                    .into_iter()
                    .map(|source| {
                        let patched =
                            patch_vertex_shader_position_w(source.to_string_lossy().as_ref());
                        CString::new(patched).unwrap_or_else(|_| CString::new("").unwrap())
                    })
                    .collect();
                let ptrs: Vec<*const u8> =
                    strings.iter().map(|s| s.as_ptr() as *const u8).collect();
                gl::ShaderSource(shader, count as i32, ptrs.as_ptr(), std::ptr::null());
            }
            "glTexImage2D" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let level = ureg(unicorn, RegisterARM::R1) as i32;
                let internal = ureg(unicorn, RegisterARM::R2);
                let width = ureg(unicorn, RegisterARM::R3) as i32;
                let height = stack_arg(unicorn, 0) as i32;
                let border = stack_arg(unicorn, 1) as i32;
                let format = stack_arg(unicorn, 2);
                let kind = stack_arg(unicorn, 3);
                let data_addr = stack_arg(unicorn, 4);
                let width = clamp_texture_dim(width);
                let height = clamp_texture_dim(height);
                log::info!(
                    "GPU: glTexImage2D detail target={:#x} level={} internal={:#x} w={} h={} format={:#x} type={:#x} data={:#x}",
                    target,
                    level,
                    internal,
                    width,
                    height,
                    format,
                    kind,
                    data_addr
                );
                let data = texture_bytes(unicorn, format, kind, width, height, data_addr);
                gl::TexImage2D(
                    target,
                    level,
                    internal as i32,
                    width,
                    height,
                    border,
                    format,
                    kind,
                    if data_addr == 0 || data.is_empty() {
                        std::ptr::null()
                    } else {
                        data.as_ptr() as *const _
                    },
                );
            }
            "glTexSubImage2D" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let level = ureg(unicorn, RegisterARM::R1) as i32;
                let x = ureg(unicorn, RegisterARM::R2) as i32;
                let y = ureg(unicorn, RegisterARM::R3) as i32;
                let width = clamp_texture_dim(stack_arg(unicorn, 0) as i32);
                let height = clamp_texture_dim(stack_arg(unicorn, 1) as i32);
                let format = stack_arg(unicorn, 2);
                let kind = stack_arg(unicorn, 3);
                let data_addr = stack_arg(unicorn, 4);
                let data = texture_bytes(unicorn, format, kind, width, height, data_addr);
                gl::TexSubImage2D(
                    target,
                    level,
                    x,
                    y,
                    width,
                    height,
                    format,
                    kind,
                    if data_addr == 0 || data.is_empty() {
                        std::ptr::null()
                    } else {
                        data.as_ptr() as *const _
                    },
                );
            }
            "glTexParameterf" => gl::TexParameterf(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
            ),
            "glTexParameteri" => gl::TexParameteri(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2) as i32,
            ),
            "glTexParameteriv" => {
                let target = ureg(unicorn, RegisterARM::R0);
                let pname = ureg(unicorn, RegisterARM::R1);
                let value = ureg(unicorn, RegisterARM::R2);
                let values = read_i32_array(unicorn, value, 1);
                gl::TexParameteriv(target, pname, values.as_ptr());
            }
            "glUniform1f" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let value = freg(unicorn, RegisterARM::R1);
                gl::Uniform1f(loc, value);
            }
            "glUniform1fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_f32_array(unicorn, values_ptr, count as usize);
                gl::Uniform1fv(loc, count, values.as_ptr());
            }
            "glUniform1i" => gl::Uniform1i(
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
            ),
            "glUniform1iv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_i32_array(unicorn, values_ptr, count as usize);
                gl::Uniform1iv(loc, count, values.as_ptr());
            }
            "glUniform2f" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let values = [
                    freg(unicorn, RegisterARM::R1),
                    freg(unicorn, RegisterARM::R2),
                ];
                gl::Uniform2f(loc, values[0], values[1]);
            }
            "glUniform2fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 2);
                gl::Uniform2fv(loc, count, values.as_ptr());
            }
            "glUniform2i" => gl::Uniform2i(
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
            ),
            "glUniform2iv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_i32_array(unicorn, values_ptr, count as usize * 2);
                gl::Uniform2iv(loc, count, values.as_ptr());
            }
            "glUniform3f" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let values = [
                    freg(unicorn, RegisterARM::R1),
                    freg(unicorn, RegisterARM::R2),
                    freg(unicorn, RegisterARM::R3),
                ];
                gl::Uniform3f(loc, values[0], values[1], values[2]);
            }
            "glUniform3fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 3);
                gl::Uniform3fv(loc, count, values.as_ptr());
            }
            "glUniform3i" => gl::Uniform3i(
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            ),
            "glUniform3iv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_i32_array(unicorn, values_ptr, count as usize * 3);
                gl::Uniform3iv(loc, count, values.as_ptr());
            }
            "glUniform4f" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let values = [
                    freg(unicorn, RegisterARM::R1),
                    freg(unicorn, RegisterARM::R2),
                    freg(unicorn, RegisterARM::R3),
                    fstack(unicorn, 0),
                ];
                gl::Uniform4f(loc, values[0], values[1], values[2], values[3]);
            }
            "glUniform4fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 4);
                gl::Uniform4fv(loc, count, values.as_ptr());
            }
            "glUniform4i" => gl::Uniform4i(
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
                stack_arg(unicorn, 0) as i32,
            ),
            "glUniform4iv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let values_ptr = ureg(unicorn, RegisterARM::R2);
                let values = read_i32_array(unicorn, values_ptr, count as usize * 4);
                gl::Uniform4iv(loc, count, values.as_ptr());
            }
            "glUniformMatrix2fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let transpose = ureg(unicorn, RegisterARM::R2) as u8;
                let values_ptr = ureg(unicorn, RegisterARM::R3);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 4);
                gl::UniformMatrix2fv(loc, count, transpose, values.as_ptr());
            }
            "glUniformMatrix3fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let transpose = ureg(unicorn, RegisterARM::R2) as u8;
                let values_ptr = ureg(unicorn, RegisterARM::R3);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 9);
                gl::UniformMatrix3fv(loc, count, transpose, values.as_ptr());
            }
            "glUniformMatrix4fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let transpose = ureg(unicorn, RegisterARM::R2) as u8;
                let values_ptr = ureg(unicorn, RegisterARM::R3);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 16);
                gl::UniformMatrix4fv(loc, count, transpose, values.as_ptr());
            }
            "glUseProgram" => gl::UseProgram(ureg(unicorn, RegisterARM::R0)),
            "glValidateProgram" => gl::ValidateProgram(ureg(unicorn, RegisterARM::R0)),
            "glVertexAttrib1f" => gl::VertexAttrib1f(
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
            ),
            "glVertexAttrib2f" => gl::VertexAttrib2f(
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
            ),
            "glVertexAttrib3f" => gl::VertexAttrib3f(
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
            ),
            "glVertexAttrib4f" => gl::VertexAttrib4f(
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
                fstack(unicorn, 0),
            ),
            "glVertexAttribPointer" => vertex_attrib_pointer(unicorn),
            "glViewport" => gl::Viewport(
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            ),
            _ => {}
        }

        clear_host_gl_errors(unicorn);
    }

    0
}

fn gl_fallback(unicorn: &mut Unicorn<'_, Context>, name: &str) -> u32 {
    match name {
        "glGetError" | "glGetString" => 0,
        "glCreateShader" | "glCreateProgram" => next_fallback_id(),
        "glGenTextures" | "glGenFramebuffers" | "glGenRenderbuffers" | "glGenBuffers" => {
            let count = ureg(unicorn, RegisterARM::R0).min(1024);
            let out = ureg(unicorn, RegisterARM::R1);
            for i in 0..count {
                write_u32(unicorn, out + i * 4, next_fallback_id());
            }
            0
        }
        "glGetShaderiv" | "glGetProgramiv" => {
            let pname = ureg(unicorn, RegisterARM::R1);
            let out = ureg(unicorn, RegisterARM::R2);
            let value = if pname == GL_INFO_LOG_LENGTH { 0 } else { 1 };
            write_u32(unicorn, out, value);
            0
        }
        "glGetIntegerv" => {
            let pname = ureg(unicorn, RegisterARM::R0);
            let out = ureg(unicorn, RegisterARM::R1);
            let value = match pname {
                0x0d33 | 0x84e8 | 0x851c | 0x8ca6 => 4096,
                _ => 16,
            };
            write_u32(unicorn, out, value);
            0
        }
        "glCheckFramebufferStatus" => 0x8cd5,
        "glGetAttribLocation" => 0,
        "glGetUniformLocation" => 1,
        _ => 0,
    }
}

fn suppress_unstubbed_gl_extensions(extensions: &str) -> String {
    extensions
        .split_ascii_whitespace()
        .filter(|ext| !ext.contains("multi_draw_arrays"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn patch_vertex_shader_position_w(source: &str) -> String {
    if !source.contains("gl_Position") || source.contains("_hmi_fixw") {
        return source.to_string();
    }
    if !source.contains("attribute") || !source.contains("position") {
        return source.to_string();
    }

    let mut patched = source
        .replace(
            "gl_Position = mvp * position;",
            "gl_Position = mvp * _hmi_fixw(position);",
        )
        .replace(
            "gl_Position = position;",
            "gl_Position = _hmi_fixw(position);",
        );

    if patched == source {
        return patched;
    }

    if let Some(insert_at) = patched.find("void main") {
        patched.insert_str(
            insert_at,
            "highp vec4 _hmi_fixw(highp vec4 p) { return vec4(p.xyz, 1.0); }\n",
        );
    } else {
        patched.push_str("\nhighp vec4 _hmi_fixw(highp vec4 p) { return vec4(p.xyz, 1.0); }\n");
    }

    patched
}

#[derive(Clone, Copy)]
enum GenKind {
    Buffer,
    Framebuffer,
    Renderbuffer,
    Texture,
}

fn gen_ids(unicorn: &mut Unicorn<'_, Context>, kind: GenKind) {
    let count = ureg(unicorn, RegisterARM::R0).min(4096) as i32;
    let out = ureg(unicorn, RegisterARM::R1);
    if count <= 0 || out == 0 {
        return;
    }

    let mut ids = vec![0u32; count as usize];
    unsafe {
        match kind {
            GenKind::Buffer => gl::GenBuffers(count, ids.as_mut_ptr()),
            GenKind::Framebuffer => gl::GenFramebuffers(count, ids.as_mut_ptr()),
            GenKind::Renderbuffer => gl::GenRenderbuffers(count, ids.as_mut_ptr()),
            GenKind::Texture => gl::GenTextures(count, ids.as_mut_ptr()),
        }
    }

    for (index, id) in ids.iter().enumerate() {
        let id = if *id == 0 { next_fallback_id() } else { *id };
        write_u32(unicorn, out + (index as u32 * 4), id);
    }
}

fn clamp_texture_dim(value: i32) -> i32 {
    value.clamp(1, MAX_TEXTURE_DIM)
}

fn texture_bytes(
    unicorn: &mut Unicorn<'_, Context>,
    format: u32,
    kind: u32,
    width: i32,
    height: i32,
    data_addr: u32,
) -> Vec<u8> {
    if data_addr == 0 {
        return Vec::new();
    }

    let width = clamp_texture_dim(width) as usize;
    let height = clamp_texture_dim(height) as usize;
    let channels = match format {
        0x1903 => 1,
        0x1906 => 1,
        0x1907 => 3,
        0x1908 => 4,
        0x1909 => 1,
        0x190a => 2,
        0x80e1 => 4,
        0x8c40 => 3,
        0x8c42 => 4,
        0x8d48 => 4,
        _ => 4,
    };
    let per_pixel = match kind {
        0x1400 | 0x1401 => 1,
        0x1402 | 0x1403 => 2,
        0x1404 | 0x1405 | 0x1406 => 4,
        _ => 1,
    };
    let size = width
        .saturating_mul(height)
        .saturating_mul(channels)
        .saturating_mul(per_pixel);
    if size > MAX_TEXTURE_BYTES {
        log::warn!(
            "GPU: refusing oversized texture data {}x{} format={:#x} type={:#x} bytes={}",
            width,
            height,
            format,
            kind,
            size
        );
        return Vec::new();
    }

    read_best_effort_bytes(unicorn, data_addr, size)
        .map(|(_, data)| data)
        .unwrap_or_default()
}

fn vertex_attrib_pointer(unicorn: &mut Unicorn<'_, Context>) {
    let index = ureg(unicorn, RegisterARM::R0);
    let size = ureg(unicorn, RegisterARM::R1) as i32;
    let kind = ureg(unicorn, RegisterARM::R2);
    let normalized = ureg(unicorn, RegisterARM::R3) as u8;
    let stride = stack_arg(unicorn, 0) as i32;
    let pointer = stack_arg(unicorn, 1);
    log::info!(
        "GPU: glVertexAttribPointer detail index={} size={} type={:#x} normalized={} stride={} ptr={:#x}",
        index,
        size,
        kind,
        normalized,
        stride,
        pointer
    );

    if pointer == 0 {
        unsafe {
            gl::VertexAttribPointer(index, size, kind, normalized, stride, std::ptr::null());
        }
        return;
    }

    let per_vertex = (stride.max(0) as usize)
        .max(size.max(1) as usize)
        .max(4)
        .min(256);
    let max_verts = 4096usize;
    let requested = per_vertex.saturating_mul(max_verts);
    let Some((copied, data)) = read_best_effort_bytes(unicorn, pointer, requested) else {
        unsafe {
            gl::VertexAttribPointer(index, size, kind, normalized, stride, std::ptr::null());
        }
        return;
    };
    let _ = copied;

    let mut buffer = 0u32;
    unsafe {
        gl::GenBuffers(1, &mut buffer);
        gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
        gl::BufferData(
            gl::ARRAY_BUFFER,
            data.len() as isize,
            data.as_ptr() as *const _,
            gl::DYNAMIC_DRAW,
        );
        gl::VertexAttribPointer(index, size, kind, normalized, stride, std::ptr::null());
    }
}
