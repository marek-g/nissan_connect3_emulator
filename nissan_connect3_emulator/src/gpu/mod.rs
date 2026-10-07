use crate::emulator::context::Context;
use sdl2::event::Event;
use sdl2::video::{GLContext, Window};
use sdl2::EventPump;
use std::cell::Cell;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Once, OnceLock};
use std::time::Duration;
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
static REAL_GL_DRAW_COUNT: AtomicU32 = AtomicU32::new(0);
static REAL_EGL_SWAP_COUNT: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
struct GpuOutput {
    ret: u32,
    out_u32: Vec<u32>,
    out_bytes: Vec<u8>,
    out_string: Option<String>,
    swap_requested: bool,
}

type GpuFuture = Box<dyn FnOnce() -> GpuOutput + Send>;

struct GpuCommand {
    future: GpuFuture,
    reply: mpsc::Sender<GpuOutput>,
}

thread_local! {
    static PENDING_GL_ERROR: Cell<bool> = const { Cell::new(false) };
    static API_LOG_COUNT: Cell<u32> = const { Cell::new(0) };
    static EGL_CURRENT_API: Cell<u32> = const { Cell::new(EGL_OPENGL_ES_API) };
}

static GPU_SENDER: OnceLock<mpsc::Sender<GpuCommand>> = OnceLock::new();
static GPU_START: Once = Once::new();
static GPU_FAILED: AtomicBool = AtomicBool::new(false);
static GPU_MAKE_CURRENT_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);

struct Backend {
    _sdl: sdl2::Sdl,
    _video: sdl2::VideoSubsystem,
    gl_context: GLContext,
    window: Window,
    events: Option<EventPump>,
}

fn gpu_backend_visible() -> bool {
    std::env::var("EMU_GPU_VISIBLE")
        .map(|value| value != "0" && !value.is_empty())
        .unwrap_or(true)
}

fn init_backend() -> Result<Backend, String> {
    let visible = gpu_backend_visible();
    let sdl = sdl2::init()?;
    let video = sdl.video()?;

    {
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::GLES);
        attrs.set_context_version(2, 0);
    }

    let window = if visible {
        video
            .window("Nissan Connect 3 HMI", 800, 480)
            .position_centered()
            .resizable()
            .opengl()
            .build()
            .map_err(|err| format!("{:?}", err))?
    } else {
        video
            .window("Nissan Connect 3 HMI", 800, 480)
            .position_centered()
            .resizable()
            .hidden()
            .opengl()
            .build()
            .map_err(|err| format!("{:?}", err))?
    };

    let gl_context = window.gl_create_context()?;
    gl::load_with(|name| video.gl_get_proc_address(name) as *const _);
    let _ = video.gl_set_swap_interval(1);
    let events = if visible {
        Some(sdl.event_pump()?)
    } else {
        None
    };

    unsafe {
        gl::Viewport(0, 0, 800, 480);
        gl::ClearColor(0.0, 0.0, 0.0, 1.0);
        gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
    }
    if visible {
        let _ = window.gl_swap_window();
    }

    log::info!(
        "GPU: dedicated SDL/GL backend thread started visible={} thread={:?}",
        visible,
        std::thread::current().id()
    );

    Ok(Backend {
        _sdl: sdl,
        _video: video,
        gl_context,
        window,
        events,
    })
}

fn poll_events(backend: &mut Backend) -> bool {
    let Some(events) = backend.events.as_mut() else {
        return false;
    };

    while let Some(event) = events.poll_event() {
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

    false
}

fn backend_main(rx: mpsc::Receiver<GpuCommand>, ready_tx: mpsc::Sender<bool>) {
    let mut backend = match init_backend() {
        Ok(backend) => backend,
        Err(err) => {
            log::warn!("GPU: dedicated SDL/GL backend unavailable: {}", err);
            let _ = ready_tx.send(false);
            return;
        }
    };

    if ready_tx.send(true).is_err() {
        return;
    }

    loop {
        match rx.recv_timeout(Duration::from_millis(16)) {
            Ok(command) => {
                if let Err(err) = backend.window.gl_make_current(&backend.gl_context) {
                    if !GPU_MAKE_CURRENT_ERROR_LOGGED.swap(true, Ordering::Relaxed) {
                        log::warn!("GPU: failed to make backend GL context current: {}", err);
                    }
                }

                let mut output = (command.future)();
                if output.swap_requested {
                    unsafe {
                        gl::Finish();
                    }
                    let _ = backend.window.gl_swap_window();
                    if poll_events(&mut backend) {
                        return;
                    }
                }

                let _ = command.reply.send(output);

                if poll_events(&mut backend) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if poll_events(&mut backend) {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
}

fn ensure_gpu_thread() -> bool {
    GPU_START.call_once(|| {
        let (sender, receiver) = mpsc::channel();
        if GPU_SENDER.set(sender).is_err() {
            GPU_FAILED.store(true, Ordering::Release);
            return;
        }

        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || backend_main(receiver, ready_tx));

        match ready_rx.recv() {
            Ok(true) => {}
            Ok(false) | Err(_) => GPU_FAILED.store(true, Ordering::Release),
        }
    });

    !GPU_FAILED.load(Ordering::Acquire)
}

fn gpu_call<F>(future: F) -> Option<GpuOutput>
where
    F: FnOnce() -> GpuOutput + Send + 'static,
{
    if !ensure_gpu_thread() {
        return None;
    }

    let sender = GPU_SENDER.get()?;
    let (reply_tx, reply_rx) = mpsc::channel();
    sender
        .send(GpuCommand {
            future: Box::new(future),
            reply: reply_tx,
        })
        .ok()?;
    reply_rx.recv().ok()
}

fn gpu_void_clear<F>(f: F) -> bool
where
    F: FnOnce() + Send + 'static,
{
    gpu_call(move || {
        f();
        unsafe {
            while gl::GetError() != 0 {}
        }
        GpuOutput::default()
    })
    .is_some()
}

fn gpu_ret_clear<F>(f: F) -> Option<u32>
where
    F: FnOnce() -> u32 + Send + 'static,
{
    gpu_call(move || {
        let ret = f();
        unsafe {
            while gl::GetError() != 0 {}
        }
        GpuOutput {
            ret,
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_u32_clear<F>(f: F) -> Option<u32>
where
    F: FnOnce() -> u32 + Send + 'static,
{
    gpu_ret_clear(f)
}

fn gpu_bytes_clear<F>(f: F) -> Option<Vec<u8>>
where
    F: FnOnce() -> Vec<u8> + Send + 'static,
{
    gpu_call(move || {
        let out_bytes = f();
        unsafe {
            while gl::GetError() != 0 {}
        }
        GpuOutput {
            out_bytes,
            ..Default::default()
        }
    })
    .map(|output| output.out_bytes)
}

fn gpu_string_clear<F>(f: F) -> Option<String>
where
    F: FnOnce() -> String + Send + 'static,
{
    gpu_call(move || {
        let out_string = f();
        unsafe {
            while gl::GetError() != 0 {}
        }
        GpuOutput {
            out_string: Some(out_string),
            ..Default::default()
        }
    })
    .and_then(|output| output.out_string)
}

fn gpu_clear_errors() {
    gpu_void_clear(|| {});
}

pub fn tick(unicorn: &Unicorn<'_, Context>) {
    ensure_backend(unicorn);
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
    gpu_process_allowed(&unicorn.get_data().elf_path) && ensure_gpu_thread()
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

fn clear_host_gl_errors(_unicorn: &Unicorn<'_, Context>) {
    // Do not enqueue a command here. `force_gl_error()` sets the local pending
    // flag before clearing host-side GL errors, and enqueueing can change guest
    // scheduling. The worker drains real GL errors with explicit GL commands.
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
    let important = name.starts_with("glDraw")
        || matches!(
            name,
            "glClear" | "glClearColor" | "glViewport" | "eglSwapBuffers"
        );
    if important || count < 3000 {
        log::info!(
            "GPU: {} {} process={} r0={:x} r1={:x} r2={:x} r3={:x}",
            prefix,
            name,
            process_label(unicorn),
            ureg(unicorn, RegisterARM::R0),
            ureg(unicorn, RegisterARM::R1),
            ureg(unicorn, RegisterARM::R2),
            ureg(unicorn, RegisterARM::R3)
        );
    }
}

fn process_label(unicorn: &Unicorn<'_, Context>) -> String {
    let elf_path = &unicorn.get_data().elf_path;
    elf_path.rsplit('/').next().unwrap_or(elf_path).to_string()
}

fn note_real_gl_output(unicorn: &Unicorn<'_, Context>, kind: &str) {
    let count = match kind {
        "draw" => REAL_GL_DRAW_COUNT.fetch_add(1, Ordering::Relaxed),
        _ => REAL_EGL_SWAP_COUNT.fetch_add(1, Ordering::Relaxed),
    } + 1;
    if count == 1 || count % 60 == 0 {
        log::info!(
            "GPU: REAL {} process={} count={}",
            kind,
            process_label(unicorn),
            count
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
            let swapped = gpu_call(|| {
                GpuOutput {
                    swap_requested: true,
                    ..Default::default()
                }
            })
            .is_some();
            if swapped {
                note_real_gl_output(unicorn, "swap");
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
            if ensure_backend(unicorn) {
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

fn backend_ready(unicorn: &Unicorn<'_, Context>) -> bool {
    ensure_backend(unicorn)
}

pub fn gl_api(unicorn: &mut Unicorn<'_, Context>, name: &str) -> u32 {
    log_api(unicorn, "GL", name);

    if ensure_backend(unicorn) {
        match name {
            "glDrawArrays" => {
                let mode = ureg(unicorn, RegisterARM::R0);
                let first = ureg(unicorn, RegisterARM::R1) as i32;
                let count = ureg(unicorn, RegisterARM::R2) as i32;
                let ok = gpu_void_clear(move || unsafe {
                    gl::DrawArrays(mode, first, count);
                });
                if ok {
                    note_real_gl_output(unicorn, "draw");
                }
                return 0;
            }
            "glDrawElements" => {
                let mode = ureg(unicorn, RegisterARM::R0);
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let kind = ureg(unicorn, RegisterARM::R2);
                let offset = ureg(unicorn, RegisterARM::R3);
                let ok = gpu_void_clear(move || unsafe {
                    gl::DrawElements(mode, count, kind, offset as *const std::ffi::c_void);
                });
                if ok {
                    note_real_gl_output(unicorn, "draw");
                }
                return 0;
            }
            _ => {}
        }
    }

    if name == "glShaderBinary" {
        PENDING_GL_ERROR.with(|flag| flag.set(true));
        return 0;
    }

    if name == "glGetError" {
        let pending = PENDING_GL_ERROR.with(|flag| flag.replace(false));
        if pending {
            return GL_INVALID_ENUM;
        }
        if ensure_backend(unicorn) {
            return gpu_ret_clear(move || unsafe {
                let err = gl::GetError();
                while gl::GetError() != 0 {}
                err
            })
            .unwrap_or(0);
        }
        return 0;
    }

    gl_fallback(unicorn, name)
}

fn gl_fallback(unicorn: &mut Unicorn<'_, Context>, name: &str) -> u32 {
    match name {
        "glGetError" => 0,
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
        "glGetString" => {
            let name = ureg(unicorn, RegisterARM::R0);
            let text = match name {
                gl::VENDOR => "emulator",
                gl::RENDERER => "emulator",
                gl::VERSION => "OpenGL ES 1.1",
                0x8b8c => "OpenGL ES GLSL ES 1.00",
                gl::EXTENSIONS => "",
                _ => "emulator",
            };
            let addr = alloc_write_guest_cstr(unicorn, text);
            log::info!("GPU: fallback glGetString name={:#x} addr={:#x}", name, addr);
            addr
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
