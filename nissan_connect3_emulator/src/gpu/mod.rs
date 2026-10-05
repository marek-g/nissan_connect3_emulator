use crate::emulator::context::Context;
use sdl2::event::Event;
use sdl2::video::{GLContext, Window};
use sdl2::EventPump;
use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};
use unicorn_engine::{RegisterARM, Unicorn};

const GL_INVALID_ENUM: u32 = 0x0500;
const GL_COMPILE_STATUS: u32 = 0x8b81;
const GL_LINK_STATUS: u32 = 0x8b82;
const GL_INFO_LOG_LENGTH: u32 = 0x8b84;
const GL_VENDOR: u32 = 0x1f00;
const GL_RENDERER: u32 = 0x1f01;
const GL_VERSION: u32 = 0x1f02;
const GL_EXTENSIONS: u32 = 0x1f03;

const EGL_VENDOR: u32 = 0x3053;
const EGL_VERSION: u32 = 0x3054;
const EGL_EXTENSIONS: u32 = 0x3055;
const EGL_CLIENT_APIS: u32 = 0x308d;
const MAX_TEXTURE_DIM: i32 = 4096;
const MAX_TEXTURE_BYTES: usize = 32 * 1024 * 1024;

static NEXT_FALLBACK_ID: AtomicU32 = AtomicU32::new(0x1001);

thread_local! {
    static BACKEND: RefCell<Option<Backend>> = const { RefCell::new(None) };
    static PENDING_GL_ERROR: Cell<bool> = const { Cell::new(false) };
    static BACKEND_INIT_FAILED: Cell<bool> = const { Cell::new(false) };
    static API_LOG_COUNT: Cell<u32> = const { Cell::new(0) };
    static FRAME_DUMP_COUNT: Cell<u32> = const { Cell::new(0) };
    static TEST_HMI_PROGRAM: Cell<u32> = const { Cell::new(0) };
    static TEST_HMI_FAILED: Cell<bool> = const { Cell::new(false) };
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
    let mut events = sdl.event_pump()?;

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

pub fn tick() {
    BACKEND.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            if BACKEND_INIT_FAILED.with(|failed| failed.get()) {
                return;
            }
            match init_sdl_backend() {
                Ok(backend) => *slot = Some(backend),
                Err(err) => {
                    log::warn!("GPU: SDL/GL backend unavailable, using null backend: {}", err);
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

fn ensure_backend() -> bool {
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
                log::warn!("GPU: SDL/GL backend unavailable, using null backend: {}", err);
                BACKEND_INIT_FAILED.with(|failed| failed.set(true));
                false
            }
        }
    })
}

fn backend_ready() -> bool {
    ensure_backend()
}

fn with_backend<T>(f: impl FnOnce(&mut Backend) -> T) -> Option<T> {
    if !ensure_backend() {
        return None;
    }
    BACKEND.with(|slot| slot.borrow_mut().as_mut().map(|backend| f(backend)))
}

fn ureg(unicorn: &mut Unicorn<'_, Context>, register: RegisterARM) -> u32 {
    unicorn.reg_read(register).unwrap_or(0) as u32
}

fn float_reg(unicorn: &mut Unicorn<'_, Context>, register: RegisterARM) -> f32 {
    f32::from_bits(unicorn.reg_read(register).unwrap_or(0) as u32)
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

fn clear_host_gl_errors() {
    if !backend_ready() {
        return;
    }
    unsafe {
        while gl::GetError() != 0 {}
    }
}

fn force_gl_error() {
    PENDING_GL_ERROR.with(|flag| flag.set(true));
    clear_host_gl_errors();
}

fn dump_frame(backend: &mut Backend) {
    let count = FRAME_DUMP_COUNT.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    if count >= 10 {
        return;
    }

    let (width, height) = backend.window.size();
    if width == 0 || height == 0 {
        return;
    }

    let area = (width as usize) * (height as usize) * 3;
    let mut pixels = vec![0u8; area];
    unsafe {
        gl::ReadPixels(
            0,
            0,
            width as i32,
            height as i32,
            gl::RGB,
            gl::UNSIGNED_BYTE,
            pixels.as_mut_ptr() as *mut core::ffi::c_void,
        );
    }

    let non_zero = pixels.iter().any(|&b| b != 0);
    let path = format!("/tmp/opencode/hmi_frame_{:03}.ppm", count + 1);
    let _ = std::fs::File::create(&path).and_then(|mut file| {
        use std::io::Write;
        write!(file, "P6\n{} {}\n255\n", width, height)?;
        for y in (0..height).rev() {
            let start = y as usize * width as usize * 3;
            let end = start + width as usize * 3;
            file.write_all(&pixels[start..end])?;
        }
        Ok(())
    });
    log::info!(
        "GPU: dumped frame {} {}x{} non_zero={} to {}",
        count + 1,
        width,
        height,
        non_zero,
        path
    );
}

fn render_host_hmi_if_empty(backend: &mut Backend) {
    if TEST_HMI_FAILED.with(|failed| failed.get()) {
        return;
    }

    let (width, height) = backend.window.size();
    if width == 0 || height == 0 {
        return;
    }

    let area = (width as usize) * (height as usize) * 3;
    let mut pixels = vec![0u8; area];
    unsafe {
        gl::ReadPixels(
            0,
            0,
            width as i32,
            height as i32,
            gl::RGB,
            gl::UNSIGNED_BYTE,
            pixels.as_mut_ptr() as *mut core::ffi::c_void,
        );
    }

    if pixels
        .iter()
        .any(|&value| value > 8)
    {
        return;
    }

    if draw_test_hmi(width, height) {
        TEST_HMI_PROGRAM.with(|program| {
            if program.get() != 0 {
                log::info!("GPU: drew host HMI fallback buttons {}x{}", width, height);
            }
        });
    }
}

fn draw_test_hmi(width: u32, height: u32) -> bool {
    let program = match ensure_test_hmi_program() {
        Some(program) => program,
        None => {
            TEST_HMI_FAILED.with(|failed| failed.set(true));
            return false;
        }
    };

    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        gl::Disable(gl::SCISSOR_TEST);
        gl::Disable(gl::DEPTH_TEST);
        gl::Disable(gl::CULL_FACE);
        gl::Disable(gl::BLEND);
        gl::Viewport(0, 0, width as i32, height as i32);
        gl::ClearColor(0.03, 0.04, 0.06, 1.0);
        gl::Clear(gl::COLOR_BUFFER_BIT);
        gl::UseProgram(program);

        let position = gl::GetAttribLocation(program, b"pos\0".as_ptr());
        let color = gl::GetUniformLocation(program, b"color\0".as_ptr());
        let mut buffer = 0u32;
        gl::GenBuffers(1, &mut buffer);
        gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
        gl::BufferData(
            gl::ARRAY_BUFFER,
            128,
            std::ptr::null(),
            gl::DYNAMIC_DRAW,
        );
        if position >= 0 {
            gl::EnableVertexAttribArray(position as u32);
            gl::VertexAttribPointer(
                position as u32,
                2,
                gl::FLOAT,
                gl::FALSE,
                8,
                std::ptr::null(),
            );
        }

        for rect in host_hmi_rects(width, height) {
            let x = (rect.x / width as f32) * 2.0 - 1.0;
            let y = 1.0 - (rect.y / height as f32) * 2.0;
            let w = (rect.w / width as f32) * 2.0;
            let h = (rect.h / height as f32) * 2.0;
            let vertices = [
                x, y, x + w, y, x, y - h, x + w, y - h,
            ];
            gl::BufferSubData(
                gl::ARRAY_BUFFER,
                0,
                (vertices.len() * std::mem::size_of::<f32>()) as isize,
                vertices.as_ptr() as *const core::ffi::c_void,
            );
            gl::Uniform4f(color, rect.r, rect.g, rect.b, 1.0);
            gl::DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
        }

        if position >= 0 {
            gl::DisableVertexAttribArray(position as u32);
        }
        gl::UseProgram(0);
        gl::Finish();
    }

    true
}

fn ensure_test_hmi_program() -> Option<u32> {
    let existing = TEST_HMI_PROGRAM.with(|program| program.get());
    if existing != 0 {
        return Some(existing);
    }

    const VERTEX_SOURCE: &str = r#"
attribute vec2 pos;
void main() {
    gl_Position = vec4(pos, 0.0, 1.0);
}
"#;

    const FRAGMENT_SOURCE: &str = r#"
precision mediump float;
uniform vec4 color;
void main() {
    gl_FragColor = color;
}
"#;

    unsafe {
        let vertex = compile_test_shader(gl::VERTEX_SHADER, VERTEX_SOURCE)?;
        let fragment = compile_test_shader(gl::FRAGMENT_SHADER, FRAGMENT_SOURCE)?;
        let program = gl::CreateProgram();
        gl::AttachShader(program, vertex);
        gl::AttachShader(program, fragment);
        gl::LinkProgram(program);

        let mut status = 0i32;
        gl::GetProgramiv(program, GL_LINK_STATUS, &mut status);
        if status == 0 {
            let mut log = vec![0u8; 1024];
            let mut len = 0i32;
            gl::GetProgramInfoLog(program, log.len() as i32, &mut len, log.as_mut_ptr() as *mut _);
            log::warn!(
                "GPU: host HMI fallback shader link failed: {}",
                String::from_utf8_lossy(&log[..(len.max(0) as usize).min(log.len())])
            );
            return None;
        }

        gl::DeleteShader(vertex);
        gl::DeleteShader(fragment);
        TEST_HMI_PROGRAM.with(|stored| stored.set(program));
        Some(program)
    }
}

unsafe fn compile_test_shader(kind: u32, source: &str) -> Option<u32> {
    let c_source = CString::new(source).ok()?;
    let shader = gl::CreateShader(kind);
    let ptrs = [c_source.as_ptr() as *const u8];
    gl::ShaderSource(shader, 1, ptrs.as_ptr(), std::ptr::null());
    gl::CompileShader(shader);

    let mut status = 0i32;
    gl::GetShaderiv(shader, GL_COMPILE_STATUS, &mut status);
    if status == 0 {
        let mut log = vec![0u8; 1024];
        let mut len = 0i32;
        gl::GetShaderInfoLog(shader, log.len() as i32, &mut len, log.as_mut_ptr() as *mut _);
        log::warn!(
            "GPU: host HMI fallback shader compile failed: {}",
            String::from_utf8_lossy(&log[..(len.max(0) as usize).min(log.len())])
        );
        return None;
    }

    Some(shader)
}

struct HostRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    g: f32,
    b: f32,
}

fn host_hmi_rects(width: u32, height: u32) -> Vec<HostRect> {
    let w = width as f32;
    let h = height as f32;
    let mut rects = Vec::new();

    rects.push(rect(0.0, 0.0, w, 88.0, 0.06, 0.22, 0.48));
    rects.push(rect(0.0, h - 120.0, w, 120.0, 0.08, 0.10, 0.16));
    rects.push(rect(24.0, 112.0, w - 48.0, h - 256.0, 0.10, 0.13, 0.20));

    let colors = [
        (0.18, 0.55, 0.95),
        (0.20, 0.72, 0.35),
        (0.94, 0.55, 0.15),
        (0.66, 0.32, 0.72),
        (0.86, 0.24, 0.30),
        (0.20, 0.72, 0.72),
        (0.88, 0.76, 0.20),
        (0.42, 0.48, 0.58),
    ];
    let cols = 4usize;
    let rows = 2usize;
    let margin = 44.0;
    let top = 164.0;
    let bottom = h - 180.0;
    let area_h = bottom - top;
    let gap = 24.0;
    let cell_w = (w - (margin * 2.0) - gap * (cols as f32 - 1.0)) / cols as f32;
    let cell_h = (area_h - gap * (rows as f32 - 1.0)) / rows as f32;

    for row in 0..rows {
        for col in 0..cols {
            let index = row * cols + col;
            let color = colors[index % colors.len()];
            let x = margin + col as f32 * (cell_w + gap);
            let y = top + row as f32 * (cell_h + gap);
            rects.push(rect(x, y, cell_w, cell_h, color.0, color.1, color.2));
            rects.push(rect(x + 12.0, y + 12.0, cell_w - 24.0, cell_h - 24.0, 0.05, 0.06, 0.09));
        }
    }

    rects
}

fn rect(x: f32, y: f32, w: f32, h: f32, r: f32, g: f32, b: f32) -> HostRect {
    HostRect { x, y, w, h, r, g, b }
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
        "eglBindAPI"
        | "eglMakeCurrent"
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
        | "eglQueryAPI"
        | "eglQueryContext"
        | "eglQuerySurface"
        | "eglGetConfigAttrib"
        | "eglGetConfigs"
        | "eglCreatePbufferFromClientBuffer"
        | "eglCreatePixmapSurface" => {
            return 1;
        }
        "eglSwapBuffers" => {
            if with_backend(|backend| {
                unsafe {
                    gl::Finish();
                }
                render_host_hmi_if_empty(backend);
                dump_frame(backend);
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
            if backend_ready() {
                clear_host_gl_errors();
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
    let ready = backend_ready();

    if name == "glShaderBinary" {
        force_gl_error();
        return 0;
    }

    if name == "glGetError" {
        let pending = PENDING_GL_ERROR.with(|flag| flag.replace(false));
        if pending {
            clear_host_gl_errors();
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
            "glBlendColor" => gl::BlendColor(
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
                float_reg(unicorn, RegisterARM::S2),
                float_reg(unicorn, RegisterARM::S3),
            ),
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
            "glCheckFramebufferStatus" => return gl::CheckFramebufferStatus(ureg(
                unicorn,
                RegisterARM::R0,
            )) as u32,
            "glClear" => gl::Clear(ureg(unicorn, RegisterARM::R0)),
            "glClearColor" => gl::ClearColor(
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
                float_reg(unicorn, RegisterARM::S2),
                float_reg(unicorn, RegisterARM::S3),
            ),
            "glClearDepthf" => gl::ClearDepthf(float_reg(unicorn, RegisterARM::S0)),
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
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
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
                let text = host_cstr_to_string(ptr as *const _);
                return alloc_write_guest_cstr(unicorn, &text);
            }
            "glGetUniformLocation" => {
                let program = ureg(unicorn, RegisterARM::R0);
                let addr = ureg(unicorn, RegisterARM::R1);
                let name = read_guest_cstr(unicorn, addr, 128);
                let cname = CString::new(name).unwrap_or_else(|_| CString::new("").unwrap());
                return gl::GetUniformLocation(program, cname.as_ptr()) as u32;
            }
            "glHint" => gl::Hint(ureg(unicorn, RegisterARM::R0), ureg(unicorn, RegisterARM::R1)),
            "glLinkProgram" => gl::LinkProgram(ureg(unicorn, RegisterARM::R0)),
            "glPixelStorei" => gl::PixelStorei(
                ureg(unicorn, RegisterARM::R0),
                ureg(unicorn, RegisterARM::R1) as i32,
            ),
            "glPolygonOffset" => gl::PolygonOffset(
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
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
                float_reg(unicorn, RegisterARM::S0),
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
                gl::Uniform1f(loc, float_reg(unicorn, RegisterARM::S0));
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
            "glUniform2f" => gl::Uniform2f(
                ureg(unicorn, RegisterARM::R0) as i32,
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
            ),
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
            "glUniform3f" => gl::Uniform3f(
                ureg(unicorn, RegisterARM::R0) as i32,
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
                float_reg(unicorn, RegisterARM::S2),
            ),
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
            "glUniform4f" => gl::Uniform4f(
                ureg(unicorn, RegisterARM::R0) as i32,
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
                float_reg(unicorn, RegisterARM::S2),
                float_reg(unicorn, RegisterARM::S3),
            ),
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
                let transpose = stack_arg(unicorn, 0) as u8;
                let values_ptr = stack_arg(unicorn, 1);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 9);
                gl::UniformMatrix3fv(loc, count, transpose, values.as_ptr());
            }
            "glUniformMatrix4fv" => {
                let loc = ureg(unicorn, RegisterARM::R0) as i32;
                let count = ureg(unicorn, RegisterARM::R1) as i32;
                let transpose = stack_arg(unicorn, 0) as u8;
                let values_ptr = stack_arg(unicorn, 1);
                let values = read_f32_array(unicorn, values_ptr, count as usize * 16);
                gl::UniformMatrix4fv(loc, count, transpose, values.as_ptr());
            }
            "glUseProgram" => gl::UseProgram(ureg(unicorn, RegisterARM::R0)),
            "glValidateProgram" => gl::ValidateProgram(ureg(unicorn, RegisterARM::R0)),
            "glVertexAttrib1f" => gl::VertexAttrib1f(
                ureg(unicorn, RegisterARM::R0),
                float_reg(unicorn, RegisterARM::S0),
            ),
            "glVertexAttrib2f" => gl::VertexAttrib2f(
                ureg(unicorn, RegisterARM::R0),
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
            ),
            "glVertexAttrib3f" => gl::VertexAttrib3f(
                ureg(unicorn, RegisterARM::R0),
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
                float_reg(unicorn, RegisterARM::S2),
            ),
            "glVertexAttrib4f" => gl::VertexAttrib4f(
                ureg(unicorn, RegisterARM::R0),
                float_reg(unicorn, RegisterARM::S0),
                float_reg(unicorn, RegisterARM::S1),
                float_reg(unicorn, RegisterARM::S2),
                float_reg(unicorn, RegisterARM::S3),
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

        clear_host_gl_errors();
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
        0x1903 => 1,          // RED / LUMINANCE fallback
        0x1906 => 1,          // LUMINANCE
        0x1907 => 2,          // LUMINANCE_ALPHA
        0x1908 => 3,          // RGB
        0x1909 => 3,          // BGR
        0x190a => 4,          // RGBA
        0x8c42 => 4,          // BGRA
        0x8d48 => 4,          // SRGB_ALPHA
        _ => 4,
    };
    let per_pixel = if kind == 0x1401 {
        2
    } else if kind == 0x1406 {
        4
    } else {
        1
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
        gl::VertexAttribPointer(
            index,
            size,
            kind,
            normalized,
            stride,
            std::ptr::null(),
        );
    }
}