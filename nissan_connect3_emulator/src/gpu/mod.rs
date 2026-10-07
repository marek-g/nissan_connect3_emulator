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
    static REAL_GL_OUTPUT_LOG_COUNT: Cell<u32> = const { Cell::new(0) };
    static EGL_CURRENT_API: Cell<u32> = const { Cell::new(EGL_OPENGL_ES_API) };
}

static GPU_SENDER: OnceLock<mpsc::Sender<GpuCommand>> = OnceLock::new();
static GPU_START: Once = Once::new();
static GPU_FAILED: AtomicBool = AtomicBool::new(false);
static GPU_MAKE_CURRENT_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);
static GL_ERROR_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

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
    if !ensure_gpu_thread() {
        return false;
    }

    let Some(sender) = GPU_SENDER.get() else {
        return false;
    };
    let (reply_tx, _reply_rx) = mpsc::channel();
    sender
        .send(GpuCommand {
            future: Box::new(move || {
                f();
                unsafe {
                    drain_host_gl_errors();
                }
                GpuOutput::default()
            }),
            reply: reply_tx,
        })
        .is_ok()
}

fn gpu_ret_clear<F>(f: F) -> Option<u32>
where
    F: FnOnce() -> u32 + Send + 'static,
{
    gpu_call(move || {
        let ret = f();
        unsafe {
            drain_host_gl_errors();
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
            drain_host_gl_errors();
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
            drain_host_gl_errors();
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

fn drain_host_gl_errors() {
    let mut first = 0u32;
    unsafe {
        loop {
            let err = gl::GetError();
            if err == 0 {
                break;
            }
            if first == 0 {
                first = err;
            }
        }
    }
    if first != 0 && GL_ERROR_LOG_COUNT.fetch_add(1, Ordering::Relaxed) < 50 {
        log::warn!("GPU: backend GL error code={:#x}", first);
    }
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
    let logged = REAL_GL_OUTPUT_LOG_COUNT.with(|slot| {
        let value = slot.get();
        slot.set(value.wrapping_add(1));
        value
    });
    if count == 1 || count % 60 == 0 || logged < 10 {
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
            let swapped = if ensure_gpu_thread() {
                let sender = GPU_SENDER.get();
                let (reply_tx, _reply_rx) = mpsc::channel();
                sender
                    .map(|sender| {
                        sender
                            .send(GpuCommand {
                                future: Box::new(|| GpuOutput {
                                    swap_requested: true,
                                    ..Default::default()
                                }),
                                reply: reply_tx,
                            })
                            .is_ok()
                    })
                    .unwrap_or(false)
            } else {
                false
            };
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

fn gpu_void_v0(f: unsafe fn()) -> bool {
    gpu_void_clear(move || unsafe { f() })
}

fn gpu_void_v1<A: Copy + Send + 'static>(f: unsafe fn(A), a: A) -> bool {
    gpu_void_clear(move || unsafe { f(a) })
}

fn gpu_void_v2<A: Copy + Send + 'static, B: Copy + Send + 'static>(
    f: unsafe fn(A, B),
    a: A,
    b: B,
) -> bool {
    gpu_void_clear(move || unsafe { f(a, b) })
}

fn gpu_void_v3<A: Copy + Send + 'static, B: Copy + Send + 'static, C: Copy + Send + 'static>(
    f: unsafe fn(A, B, C),
    a: A,
    b: B,
    c: C,
) -> bool {
    gpu_void_clear(move || unsafe { f(a, b, c) })
}

fn gpu_void_v4<
    A: Copy + Send + 'static,
    B: Copy + Send + 'static,
    C: Copy + Send + 'static,
    D: Copy + Send + 'static,
>(
    f: unsafe fn(A, B, C, D),
    a: A,
    b: B,
    c: C,
    d: D,
) -> bool {
    gpu_void_clear(move || unsafe { f(a, b, c, d) })
}

fn gpu_void_v5<
    A: Copy + Send + 'static,
    B: Copy + Send + 'static,
    C: Copy + Send + 'static,
    D: Copy + Send + 'static,
    E: Copy + Send + 'static,
>(
    f: unsafe fn(A, B, C, D, E),
    a: A,
    b: B,
    c: C,
    d: D,
    e: E,
) -> bool {
    gpu_void_clear(move || unsafe { f(a, b, c, d, e) })
}

fn gpu_ret_u0(f: unsafe fn() -> u32) -> Option<u32> {
    gpu_ret_clear(move || unsafe { f() })
}

fn gpu_ret_u1<A: Copy + Send + 'static>(f: unsafe fn(A) -> u32, a: A) -> Option<u32> {
    gpu_ret_clear(move || unsafe { f(a) })
}

fn gpu_ret_i1<A: Copy + Send + 'static>(f: unsafe fn(A) -> i32, a: A) -> Option<u32> {
    gpu_ret_clear(move || unsafe { f(a) as u32 })
}

fn gpu_ret_i2<A: Copy + Send + 'static, B: Copy + Send + 'static>(
    f: unsafe fn(A, B) -> i32,
    a: A,
    b: B,
) -> Option<u32> {
    gpu_ret_clear(move || unsafe { f(a, b) as u32 })
}

fn gpu_get_integerv(pname: u32) -> Option<u32> {
    gpu_call(move || {
        let mut value = 0i32;
        unsafe {
            gl::GetIntegerv(pname, &mut value);
            drain_host_gl_errors();
        }
        GpuOutput {
            ret: value as u32,
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_get_floatv(pname: u32) -> Option<u32> {
    gpu_call(move || {
        let mut value = 0f32;
        unsafe {
            gl::GetFloatv(pname, &mut value);
            drain_host_gl_errors();
        }
        GpuOutput {
            ret: value.to_bits(),
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_get_shaderiv(shader: u32, pname: u32) -> Option<u32> {
    gpu_call(move || {
        let mut value = 0i32;
        unsafe {
            gl::GetShaderiv(shader, pname, &mut value);
            drain_host_gl_errors();
        }
        let value = if pname == GL_INFO_LOG_LENGTH && value <= 0 {
            1
        } else {
            value
        };
        GpuOutput {
            ret: value as u32,
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_get_programiv(program: u32, pname: u32) -> Option<u32> {
    gpu_call(move || {
        let mut value = 0i32;
        unsafe {
            gl::GetProgramiv(program, pname, &mut value);
            drain_host_gl_errors();
        }
        let value = if pname == GL_INFO_LOG_LENGTH && value <= 0 {
            1
        } else {
            value
        };
        GpuOutput {
            ret: value as u32,
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_get_uniform_location(program: u32, name: String) -> Option<u32> {
    let cname = CString::new(name).unwrap_or_else(|_| CString::new("").unwrap());
    gpu_call(move || {
        let value = unsafe {
            let value = gl::GetUniformLocation(program, cname.as_ptr());
            drain_host_gl_errors();
            value
        };
        GpuOutput {
            ret: value as u32,
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_get_attrib_location(program: u32, name: String) -> Option<u32> {
    let cname = CString::new(name).unwrap_or_else(|_| CString::new("").unwrap());
    gpu_call(move || {
        let value = unsafe {
            let value = gl::GetAttribLocation(program, cname.as_ptr());
            drain_host_gl_errors();
            value
        };
        GpuOutput {
            ret: value as u32,
            ..Default::default()
        }
    })
    .map(|output| output.ret)
}

fn gpu_get_string(pname: u32) -> Option<String> {
    gpu_string_clear(move || {
        let ptr = unsafe { gl::GetString(pname) };
        let mut text = host_cstr_to_string(ptr as *const std::os::raw::c_char);
        if pname == gl::EXTENSIONS {
            text = suppress_unstubbed_gl_extensions(&text);
        }
        text
    })
}

fn gpu_get_shader_info_log(shader: u32, buf_size: i32) -> Option<(u32, Vec<u8>)> {
    if buf_size <= 0 {
        return Some((0, Vec::new()));
    }
    gpu_call(move || {
        let mut log = vec![0u8; buf_size as usize];
        let mut len = 0i32;
        unsafe {
            gl::GetShaderInfoLog(shader, buf_size, &mut len, log.as_mut_ptr() as *mut _);
            drain_host_gl_errors();
        }
        let len = len.max(0).min(buf_size) as usize;
        GpuOutput {
            ret: len as u32,
            out_bytes: log[..len].to_vec(),
            ..Default::default()
        }
    })
    .map(|output| (output.ret, output.out_bytes))
}

fn gpu_get_program_info_log(program: u32, buf_size: i32) -> Option<(u32, Vec<u8>)> {
    if buf_size <= 0 {
        return Some((0, Vec::new()));
    }
    gpu_call(move || {
        let mut log = vec![0u8; buf_size as usize];
        let mut len = 0i32;
        unsafe {
            gl::GetProgramInfoLog(program, buf_size, &mut len, log.as_mut_ptr() as *mut _);
            drain_host_gl_errors();
        }
        let len = len.max(0).min(buf_size) as usize;
        GpuOutput {
            ret: len as u32,
            out_bytes: log[..len].to_vec(),
            ..Default::default()
        }
    })
    .map(|output| (output.ret, output.out_bytes))
}

#[derive(Clone, Copy)]
enum DeleteKind {
    Buffer,
    Framebuffer,
    Renderbuffer,
    Texture,
}

fn gpu_gen_ids(kind: GenKind, count: i32) -> Option<Vec<u32>> {
    if count <= 0 {
        return Some(Vec::new());
    }
    gpu_call(move || {
        let mut ids = vec![0u32; count as usize];
        unsafe {
            match kind {
                GenKind::Buffer => gl::GenBuffers(count, ids.as_mut_ptr()),
                GenKind::Framebuffer => gl::GenFramebuffers(count, ids.as_mut_ptr()),
                GenKind::Renderbuffer => gl::GenRenderbuffers(count, ids.as_mut_ptr()),
                GenKind::Texture => gl::GenTextures(count, ids.as_mut_ptr()),
            }
            drain_host_gl_errors();
        }
        GpuOutput {
            out_u32: ids,
            ..Default::default()
        }
    })
    .map(|output| output.out_u32)
}

fn gpu_delete_ids(kind: DeleteKind, count: i32, ids: Vec<u32>) -> bool {
    let count = count.max(0);
    if count == 0 || ids.is_empty() {
        return true;
    }
    gpu_void_clear(move || unsafe {
        match kind {
            DeleteKind::Buffer => gl::DeleteBuffers(count, ids.as_ptr()),
            DeleteKind::Framebuffer => gl::DeleteFramebuffers(count, ids.as_ptr()),
            DeleteKind::Renderbuffer => gl::DeleteRenderbuffers(count, ids.as_ptr()),
            DeleteKind::Texture => gl::DeleteTextures(count, ids.as_ptr()),
        }
    })
}

fn gpu_buffer_data(target: u32, size: usize, data: Vec<u8>) -> bool {
    gpu_void_clear(move || unsafe {
        let ptr = if data.is_empty() {
            std::ptr::null()
        } else {
            data.as_ptr() as *const _
        };
        gl::BufferData(target, size as isize, ptr, gl::STATIC_DRAW);
    })
}

fn gpu_buffer_sub_data(target: u32, offset: isize, size: usize, data: Vec<u8>) -> bool {
    gpu_void_clear(move || unsafe {
        let ptr = if data.is_empty() {
            std::ptr::null()
        } else {
            data.as_ptr() as *const _
        };
        gl::BufferSubData(target, offset, size as isize, ptr);
    })
}

fn gpu_tex_image2d(
    target: u32,
    level: i32,
    internal: i32,
    width: i32,
    height: i32,
    border: i32,
    format: u32,
    kind: u32,
    data: Vec<u8>,
) -> bool {
    gpu_void_clear(move || unsafe {
        let ptr = if data.is_empty() {
            std::ptr::null()
        } else {
            data.as_ptr() as *const _
        };
        gl::TexImage2D(target, level, internal, width, height, border, format, kind, ptr);
    })
}

fn gpu_tex_sub_image2d(
    target: u32,
    level: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    format: u32,
    kind: u32,
    data: Vec<u8>,
) -> bool {
    gpu_void_clear(move || unsafe {
        let ptr = if data.is_empty() {
            std::ptr::null()
        } else {
            data.as_ptr() as *const _
        };
        gl::TexSubImage2D(target, level, x, y, width, height, format, kind, ptr);
    })
}

fn gpu_vertex_attrib_pointer(
    index: u32,
    size: i32,
    kind: u32,
    normalized: u8,
    stride: i32,
    pointer: u32,
    data: Vec<u8>,
) -> bool {
    if data.is_empty() {
        return gpu_void_clear(move || unsafe {
            gl::VertexAttribPointer(
                index,
                size,
                kind,
                normalized,
                stride,
                std::ptr::null(),
            );
        });
    }

    gpu_void_clear(move || unsafe {
        let mut previous = 0i32;
        gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);

        let mut buffer = 0u32;
        gl::GenBuffers(1, &mut buffer);
        if buffer == 0 {
            buffer = next_fallback_id();
        }
        gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
        gl::BufferData(
            gl::ARRAY_BUFFER,
            data.len() as isize,
            data.as_ptr() as *const _,
            gl::DYNAMIC_DRAW,
        );
        gl::VertexAttribPointer(index, size, kind, normalized, stride, std::ptr::null());
        gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
    })
}

fn gpu_shader_source(shader: u32, count: i32, sources: Vec<CString>) -> bool {
    gpu_void_clear(move || unsafe {
        if sources.is_empty() {
            gl::ShaderSource(shader, 0, std::ptr::null(), std::ptr::null());
            return;
        }
        let ptrs: Vec<*const u8> = sources.iter().map(|source| source.as_ptr() as *const u8).collect();
        gl::ShaderSource(shader, count.min(ptrs.len() as i32), ptrs.as_ptr(), std::ptr::null());
    })
}

fn route_gen(unicorn: &mut Unicorn<'_, Context>, kind: GenKind) -> u32 {
    let count = ureg(unicorn, RegisterARM::R0).min(4096) as i32;
    let out = ureg(unicorn, RegisterARM::R1);
    if let Some(ids) = gpu_gen_ids(kind, count) {
        for (index, id) in ids.iter().enumerate() {
            let id = if *id == 0 { next_fallback_id() } else { *id };
            write_u32(unicorn, out + (index as u32 * 4), id);
        }
    }
    0
}

fn route_delete(unicorn: &mut Unicorn<'_, Context>, kind: DeleteKind) -> u32 {
    let count = ureg(unicorn, RegisterARM::R0).min(4096) as i32;
    let ptr = ureg(unicorn, RegisterARM::R1);
    if count > 0 && ptr != 0 {
        let values = read_u32_array(unicorn, ptr, count as usize);
        gpu_delete_ids(kind, count, values);
    }
    0
}

fn route_uniform_fv(unicorn: &mut Unicorn<'_, Context>, components: usize) -> u32 {
    let loc = ureg(unicorn, RegisterARM::R0) as i32;
    let count = ureg(unicorn, RegisterARM::R1).min(65536) as i32;
    let values_ptr = ureg(unicorn, RegisterARM::R2);
    let values = read_f32_array(unicorn, values_ptr, (count.max(0) as usize).saturating_mul(components));
    gpu_void_clear(move || unsafe {
        let ptr = if values.is_empty() {
            std::ptr::null()
        } else {
            values.as_ptr()
        };
        match components {
            1 => gl::Uniform1fv(loc, count, ptr),
            2 => gl::Uniform2fv(loc, count, ptr),
            3 => gl::Uniform3fv(loc, count, ptr),
            _ => gl::Uniform4fv(loc, count, ptr),
        }
    });
    0
}

fn route_uniform_iv(unicorn: &mut Unicorn<'_, Context>, components: usize) -> u32 {
    let loc = ureg(unicorn, RegisterARM::R0) as i32;
    let count = ureg(unicorn, RegisterARM::R1).min(65536) as i32;
    let values_ptr = ureg(unicorn, RegisterARM::R2);
    let values = read_i32_array(unicorn, values_ptr, (count.max(0) as usize).saturating_mul(components));
    gpu_void_clear(move || unsafe {
        let ptr = if values.is_empty() {
            std::ptr::null()
        } else {
            values.as_ptr()
        };
        match components {
            1 => gl::Uniform1iv(loc, count, ptr),
            2 => gl::Uniform2iv(loc, count, ptr),
            3 => gl::Uniform3iv(loc, count, ptr),
            _ => gl::Uniform4iv(loc, count, ptr),
        }
    });
    0
}

fn route_uniform_matrix(unicorn: &mut Unicorn<'_, Context>, components: usize) -> u32 {
    let loc = ureg(unicorn, RegisterARM::R0) as i32;
    let count = ureg(unicorn, RegisterARM::R1).min(65536) as i32;
    let transpose = ureg(unicorn, RegisterARM::R2) as u8;
    let values_ptr = ureg(unicorn, RegisterARM::R3);
    let values = read_f32_array(unicorn, values_ptr, (count.max(0) as usize).saturating_mul(components));
    gpu_void_clear(move || unsafe {
        let ptr = if values.is_empty() {
            std::ptr::null()
        } else {
            values.as_ptr()
        };
        match components {
            4 => gl::UniformMatrix2fv(loc, count, transpose, ptr),
            9 => gl::UniformMatrix3fv(loc, count, transpose, ptr),
            _ => gl::UniformMatrix4fv(loc, count, transpose, ptr),
        }
    });
    0
}

fn route_shader_source(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let shader = ureg(unicorn, RegisterARM::R0);
    let count = (ureg(unicorn, RegisterARM::R1) as usize).min(64);
    let strings_ptr = ureg(unicorn, RegisterARM::R2);
    let lengths_ptr = ureg(unicorn, RegisterARM::R3);
    let mut sources = Vec::new();
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
        let source = if let Some(len) = len {
            let bytes = read_bytes(unicorn, src_ptr, len).unwrap_or_default();
            String::from_utf8_lossy(&bytes).to_string()
        } else {
            read_guest_cstr(unicorn, src_ptr, 64 * 1024)
        };
        let patched = patch_vertex_shader_position_w(&source);
        sources.push(CString::new(patched).unwrap_or_else(|_| CString::new("").unwrap()));
    }
    gpu_shader_source(shader, count as i32, sources);
    0
}

fn route_vertex_attrib_pointer(unicorn: &mut Unicorn<'_, Context>) -> u32 {
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
        gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, Vec::new());
        return 0;
    }

    let per_vertex = (stride.max(0) as usize)
        .max(size.max(1) as usize)
        .max(4)
        .min(256);
    let max_verts = 4096usize;
    let requested = per_vertex.saturating_mul(max_verts);
    let Some((_, data)) = read_best_effort_bytes(unicorn, pointer, requested) else {
        gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, Vec::new());
        return 0;
    };

    gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, data);
    0
}

fn route_tex_image2d(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let target = ureg(unicorn, RegisterARM::R0);
    let level = ureg(unicorn, RegisterARM::R1) as i32;
    let internal = ureg(unicorn, RegisterARM::R2) as i32;
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
    gpu_tex_image2d(
        target, level, internal, width, height, border, format, kind, data,
    );
    0
}

fn route_tex_sub_image2d(unicorn: &mut Unicorn<'_, Context>) -> u32 {
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
    gpu_tex_sub_image2d(target, level, x, y, width, height, format, kind, data);
    0
}

fn gl_backend_api(unicorn: &mut Unicorn<'_, Context>, name: &str) -> Option<u32> {
    match name {
        "glShaderBinary" => {
            PENDING_GL_ERROR.with(|flag| flag.set(true));
            return Some(0);
        }
        "glDrawArrays" => {
            let mode = ureg(unicorn, RegisterARM::R0);
            let first = ureg(unicorn, RegisterARM::R1) as i32;
            let count = ureg(unicorn, RegisterARM::R2) as i32;
            if gpu_void_v3(gl::DrawArrays, mode, first, count) {
                note_real_gl_output(unicorn, "draw");
            }
            return Some(0);
        }
        "glDrawElements" => {
            let mode = ureg(unicorn, RegisterARM::R0);
            let count = ureg(unicorn, RegisterARM::R1) as i32;
            let kind = ureg(unicorn, RegisterARM::R2);
            let offset = ureg(unicorn, RegisterARM::R3);
            if gpu_void_clear(move || unsafe {
                gl::DrawElements(mode, count, kind, offset as *const std::ffi::c_void);
            }) {
                note_real_gl_output(unicorn, "draw");
            }
            return Some(0);
        }
        "glActiveTexture" => {
            gpu_void_v1(gl::ActiveTexture, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glAttachShader" => {
            gpu_void_v2(
                gl::AttachShader,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBindBuffer" => {
            gpu_void_v2(
                gl::BindBuffer,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBindFramebuffer" => {
            gpu_void_v2(
                gl::BindFramebuffer,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBindRenderbuffer" => {
            gpu_void_v2(
                gl::BindRenderbuffer,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBindTexture" => {
            gpu_void_v2(
                gl::BindTexture,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBlendEquation" => {
            gpu_void_v1(gl::BlendEquation, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glBlendEquationSeparate" => {
            gpu_void_v2(
                gl::BlendEquationSeparate,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBlendFunc" => {
            gpu_void_v2(
                gl::BlendFunc,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glBlendFuncSeparate" => {
            gpu_void_v4(
                gl::BlendFuncSeparate,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2),
                ureg(unicorn, RegisterARM::R3),
            );
            return Some(0);
        }
        "glClear" => {
            gpu_void_v1(gl::Clear, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glClearColor" => {
            gpu_void_v4(
                gl::ClearColor,
                freg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
            );
            return Some(0);
        }
        "glClearDepthf" => {
            gpu_void_v1(gl::ClearDepthf, freg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glClearStencil" => {
            gpu_void_v1(gl::ClearStencil, ureg(unicorn, RegisterARM::R0) as i32);
            return Some(0);
        }
        "glColorMask" => {
            gpu_void_v4(
                gl::ColorMask,
                ureg(unicorn, RegisterARM::R0) as u8,
                ureg(unicorn, RegisterARM::R1) as u8,
                ureg(unicorn, RegisterARM::R2) as u8,
                ureg(unicorn, RegisterARM::R3) as u8,
            );
            return Some(0);
        }
        "glCullFace" => {
            gpu_void_v1(gl::CullFace, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDepthFunc" => {
            gpu_void_v1(gl::DepthFunc, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDepthMask" => {
            gpu_void_v1(gl::DepthMask, ureg(unicorn, RegisterARM::R0) as u8);
            return Some(0);
        }
        "glDepthRangef" => {
            gpu_void_v2(
                gl::DepthRangef,
                freg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glDisable" => {
            gpu_void_v1(gl::Disable, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDisableVertexAttribArray" => {
            gpu_void_v1(gl::DisableVertexAttribArray, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glEnable" => {
            gpu_void_v1(gl::Enable, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glEnableVertexAttribArray" => {
            gpu_void_v1(gl::EnableVertexAttribArray, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glFinish" => {
            gpu_void_v0(gl::Finish);
            return Some(0);
        }
        "glFlush" => {
            gpu_void_v0(gl::Flush);
            return Some(0);
        }
        "glFrontFace" => {
            gpu_void_v1(gl::FrontFace, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glGenerateMipmap" => {
            gpu_void_v1(gl::GenerateMipmap, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glHint" => {
            gpu_void_v2(
                gl::Hint,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glLinkProgram" => {
            gpu_void_v1(gl::LinkProgram, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDetachShader" => {
            gpu_void_v2(
                gl::DetachShader,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glPixelStorei" => {
            gpu_void_v2(
                gl::PixelStorei,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1) as i32,
            );
            return Some(0);
        }
        "glPolygonOffset" => {
            gpu_void_v2(
                gl::PolygonOffset,
                freg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glScissor" => {
            gpu_void_v4(
                gl::Scissor,
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            );
            return Some(0);
        }
        "glStencilMask" => {
            gpu_void_v1(gl::StencilMask, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glUseProgram" => {
            gpu_void_v1(gl::UseProgram, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glValidateProgram" => {
            gpu_void_v1(gl::ValidateProgram, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glViewport" => {
            gpu_void_v4(
                gl::Viewport,
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            );
            return Some(0);
        }
        "glCreateShader" => {
            return gpu_ret_u1(gl::CreateShader, ureg(unicorn, RegisterARM::R0));
        }
        "glCreateProgram" => {
            return gpu_ret_u0(gl::CreateProgram);
        }
        "glCompileShader" => {
            gpu_void_v1(gl::CompileShader, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDeleteProgram" => {
            gpu_void_v1(gl::DeleteProgram, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDeleteShader" => {
            gpu_void_v1(gl::DeleteShader, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glCheckFramebufferStatus" => {
            return gpu_ret_u1(gl::CheckFramebufferStatus, ureg(unicorn, RegisterARM::R0));
        }
        "glGenBuffers" => return Some(route_gen(unicorn, GenKind::Buffer)),
        "glGenFramebuffers" => return Some(route_gen(unicorn, GenKind::Framebuffer)),
        "glGenRenderbuffers" => return Some(route_gen(unicorn, GenKind::Renderbuffer)),
        "glGenTextures" => return Some(route_gen(unicorn, GenKind::Texture)),
        "glDeleteBuffers" => return Some(route_delete(unicorn, DeleteKind::Buffer)),
        "glDeleteFramebuffers" => return Some(route_delete(unicorn, DeleteKind::Framebuffer)),
        "glDeleteRenderbuffers" => return Some(route_delete(unicorn, DeleteKind::Renderbuffer)),
        "glDeleteTextures" => return Some(route_delete(unicorn, DeleteKind::Texture)),
        "glGetAttribLocation" => {
            let program = ureg(unicorn, RegisterARM::R0);
            let name_ptr = ureg(unicorn, RegisterARM::R1);
            let name = read_guest_cstr(unicorn, name_ptr, 128);
            return gpu_get_attrib_location(program, name);
        }
        "glGetUniformLocation" => {
            let program = ureg(unicorn, RegisterARM::R0);
            let name_ptr = ureg(unicorn, RegisterARM::R1);
            let name = read_guest_cstr(unicorn, name_ptr, 128);
            return gpu_get_uniform_location(program, name);
        }
        "glGetError" => {
            if PENDING_GL_ERROR.with(|flag| flag.replace(false)) {
                return Some(GL_INVALID_ENUM);
            }
            return gpu_ret_clear(move || unsafe {
                let err = gl::GetError();
                drain_host_gl_errors();
                err
            });
        }
        "glGetIntegerv" => {
            let pname = ureg(unicorn, RegisterARM::R0);
            let out = ureg(unicorn, RegisterARM::R1);
            if let Some(value) = gpu_get_integerv(pname) {
                write_u32(unicorn, out, value);
            }
            return Some(0);
        }
        "glGetFloatv" => {
            let pname = ureg(unicorn, RegisterARM::R0);
            let out = ureg(unicorn, RegisterARM::R1);
            if let Some(value) = gpu_get_floatv(pname) {
                write_u32(unicorn, out, value);
            }
            return Some(0);
        }
        "glGetShaderiv" => {
            let shader = ureg(unicorn, RegisterARM::R0);
            let pname = ureg(unicorn, RegisterARM::R1);
            let out = ureg(unicorn, RegisterARM::R2);
            if let Some(value) = gpu_get_shaderiv(shader, pname) {
                write_u32(unicorn, out, value);
            }
            return Some(0);
        }
        "glGetProgramiv" => {
            let program = ureg(unicorn, RegisterARM::R0);
            let pname = ureg(unicorn, RegisterARM::R1);
            let out = ureg(unicorn, RegisterARM::R2);
            if let Some(value) = gpu_get_programiv(program, pname) {
                write_u32(unicorn, out, value);
            }
            return Some(0);
        }
        "glGetString" => {
            let pname = ureg(unicorn, RegisterARM::R0);
            if let Some(text) = gpu_get_string(pname) {
                return Some(alloc_write_guest_cstr(unicorn, &text));
            }
            return Some(gl_fallback(unicorn, name));
        }
        "glGetShaderInfoLog" => {
            let shader = ureg(unicorn, RegisterARM::R0);
            let buf_size = ureg(unicorn, RegisterARM::R1).min(1024 * 1024) as i32;
            let len_out = ureg(unicorn, RegisterARM::R2);
            let buf = ureg(unicorn, RegisterARM::R3);
            if buf_size > 0 && buf != 0 {
                if let Some((len, data)) = gpu_get_shader_info_log(shader, buf_size) {
                    let _ = unicorn.mem_write(buf as u64, &data);
                    write_u32(unicorn, len_out, len);
                }
            }
            return Some(0);
        }
        "glGetProgramInfoLog" => {
            let program = ureg(unicorn, RegisterARM::R0);
            let buf_size = ureg(unicorn, RegisterARM::R1).min(1024 * 1024) as i32;
            let len_out = ureg(unicorn, RegisterARM::R2);
            let buf = ureg(unicorn, RegisterARM::R3);
            if buf_size > 0 && buf != 0 {
                if let Some((len, data)) = gpu_get_program_info_log(program, buf_size) {
                    let _ = unicorn.mem_write(buf as u64, &data);
                    write_u32(unicorn, len_out, len);
                }
            }
            return Some(0);
        }
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
            gpu_buffer_data(target, size, data);
            return Some(0);
        }
        "glBufferSubData" => {
            let target = ureg(unicorn, RegisterARM::R0);
            let offset = ureg(unicorn, RegisterARM::R1) as isize;
            let size = (ureg(unicorn, RegisterARM::R2) as usize).min(MAX_TEXTURE_BYTES);
            let data_addr = ureg(unicorn, RegisterARM::R3);
            let data = read_best_effort_bytes(unicorn, data_addr, size)
                .map(|(_, data)| data)
                .unwrap_or_default();
            gpu_buffer_sub_data(target, offset, size, data);
            return Some(0);
        }
        "glFramebufferRenderbuffer" => {
            gpu_void_v4(
                gl::FramebufferRenderbuffer,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2),
                ureg(unicorn, RegisterARM::R3),
            );
            return Some(0);
        }
        "glFramebufferTexture2D" => {
            let target = ureg(unicorn, RegisterARM::R0);
            let attachment = ureg(unicorn, RegisterARM::R1);
            let textarget = ureg(unicorn, RegisterARM::R2);
            let texture = ureg(unicorn, RegisterARM::R3);
            let level = stack_arg(unicorn, 0) as i32;
            gpu_void_v5(
                gl::FramebufferTexture2D,
                target,
                attachment,
                textarget,
                texture,
                level,
            );
            return Some(0);
        }
        "glRenderbufferStorage" => {
            gpu_void_v4(
                gl::RenderbufferStorage,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            );
            return Some(0);
        }
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
            if size > MAX_TEXTURE_BYTES || out == 0 {
                return Some(0);
            }
            if let Some(data) = gpu_bytes_clear(move || unsafe {
                let mut data = vec![0u8; size];
                gl::ReadPixels(
                    x,
                    y,
                    width,
                    height,
                    format,
                    kind,
                    data.as_mut_ptr() as *mut _,
                );
                drain_host_gl_errors();
                data
            }) {
                let _ = unicorn.mem_write(out as u64, &data);
            }
            return Some(0);
        }
        "glTexParameterf" => {
            gpu_void_v3(
                gl::TexParameterf,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
            );
            return Some(0);
        }
        "glTexParameteri" => {
            gpu_void_v3(
                gl::TexParameteri,
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1),
                ureg(unicorn, RegisterARM::R2) as i32,
            );
            return Some(0);
        }
        "glTexParameteriv" => {
            let target = ureg(unicorn, RegisterARM::R0);
            let pname = ureg(unicorn, RegisterARM::R1);
            let values_ptr = ureg(unicorn, RegisterARM::R2);
            let values = read_i32_array(unicorn, values_ptr, 1);
            gpu_void_clear(move || unsafe {
                let ptr = if values.is_empty() {
                    std::ptr::null()
                } else {
                    values.as_ptr()
                };
                gl::TexParameteriv(target, pname, ptr);
            });
            return Some(0);
        }
        "glUniform1f" => {
            gpu_void_v2(
                gl::Uniform1f,
                ureg(unicorn, RegisterARM::R0) as i32,
                freg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glUniform1fv" => return Some(route_uniform_fv(unicorn, 1)),
        "glUniform1i" => {
            gpu_void_v2(
                gl::Uniform1i,
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
            );
            return Some(0);
        }
        "glUniform1iv" => return Some(route_uniform_iv(unicorn, 1)),
        "glUniform2f" => {
            gpu_void_v3(
                gl::Uniform2f,
                ureg(unicorn, RegisterARM::R0) as i32,
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
            );
            return Some(0);
        }
        "glUniform2fv" => return Some(route_uniform_fv(unicorn, 2)),
        "glUniform2i" => {
            gpu_void_v3(
                gl::Uniform2i,
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
            );
            return Some(0);
        }
        "glUniform2iv" => return Some(route_uniform_iv(unicorn, 2)),
        "glUniform3f" => {
            gpu_void_v4(
                gl::Uniform3f,
                ureg(unicorn, RegisterARM::R0) as i32,
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
            );
            return Some(0);
        }
        "glUniform3fv" => return Some(route_uniform_fv(unicorn, 3)),
        "glUniform3i" => {
            gpu_void_v4(
                gl::Uniform3i,
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
            );
            return Some(0);
        }
        "glUniform3iv" => return Some(route_uniform_iv(unicorn, 3)),
        "glUniform4f" => {
            gpu_void_v5(
                gl::Uniform4f,
                ureg(unicorn, RegisterARM::R0) as i32,
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
                fstack(unicorn, 0),
            );
            return Some(0);
        }
        "glUniform4fv" => return Some(route_uniform_fv(unicorn, 4)),
        "glUniform4i" => {
            gpu_void_v5(
                gl::Uniform4i,
                ureg(unicorn, RegisterARM::R0) as i32,
                ureg(unicorn, RegisterARM::R1) as i32,
                ureg(unicorn, RegisterARM::R2) as i32,
                ureg(unicorn, RegisterARM::R3) as i32,
                stack_arg(unicorn, 0) as i32,
            );
            return Some(0);
        }
        "glUniform4iv" => return Some(route_uniform_iv(unicorn, 4)),
        "glUniformMatrix2fv" => return Some(route_uniform_matrix(unicorn, 4)),
        "glUniformMatrix3fv" => return Some(route_uniform_matrix(unicorn, 9)),
        "glUniformMatrix4fv" => return Some(route_uniform_matrix(unicorn, 16)),
        "glShaderSource" => return Some(route_shader_source(unicorn)),
        "glVertexAttribPointer" => return Some(route_vertex_attrib_pointer(unicorn)),
        "glTexImage2D" => return Some(route_tex_image2d(unicorn)),
        "glTexSubImage2D" => return Some(route_tex_sub_image2d(unicorn)),
        "glVertexAttrib1f" => {
            gpu_void_v2(
                gl::VertexAttrib1f,
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
            );
            return Some(0);
        }
        "glVertexAttrib2f" => {
            gpu_void_v3(
                gl::VertexAttrib2f,
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
            );
            return Some(0);
        }
        "glVertexAttrib3f" => {
            gpu_void_v4(
                gl::VertexAttrib3f,
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
            );
            return Some(0);
        }
        "glVertexAttrib4f" => {
            gpu_void_v5(
                gl::VertexAttrib4f,
                ureg(unicorn, RegisterARM::R0),
                freg(unicorn, RegisterARM::R1),
                freg(unicorn, RegisterARM::R2),
                freg(unicorn, RegisterARM::R3),
                fstack(unicorn, 0),
            );
            return Some(0);
        }
        "glVertexAttrib4fv" => {
            let index = ureg(unicorn, RegisterARM::R0);
            let values_ptr = ureg(unicorn, RegisterARM::R1);
            let values = read_f32_array(unicorn, values_ptr, 4);
            gpu_void_clear(move || unsafe {
                let ptr = if values.is_empty() {
                    std::ptr::null()
                } else {
                    values.as_ptr()
                };
                gl::VertexAttrib4fv(index, ptr);
            });
            return Some(0);
        }
        _ => {}
    }

    None
}

pub fn gl_api(unicorn: &mut Unicorn<'_, Context>, name: &str) -> u32 {
    log_api(unicorn, "GL", name);

    if ensure_backend(unicorn) {
        return gl_backend_api(unicorn, name).unwrap_or_else(|| gl_fallback(unicorn, name));
    }

    if name == "glShaderBinary" {
        PENDING_GL_ERROR.with(|flag| flag.set(true));
        return 0;
    }

    if name == "glGetError" {
        if PENDING_GL_ERROR.with(|flag| flag.replace(false)) {
            return GL_INVALID_ENUM;
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
