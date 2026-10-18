use crate::emulator::context::Context;
use sdl2::event::Event;
use sdl2::video::Window;
use sdl2::{sys, EventPump};
use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, Once, OnceLock};
use std::time::Duration;
use unicorn_engine::unicorn_const::Prot;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpuTarget {
    Hmi,
    Map,
}

pub(crate) static GL_LIVE_TEXTURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static GL_LIVE_FRAMEBUFFERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static GL_LIVE_RENDERBUFFERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static GL_LIVE_BUFFERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static GL_LIVE_SHADERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static GL_LIVE_PROGRAMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct GpuCommand {
    target: GpuTarget,
    future: GpuFuture,
    reply: mpsc::Sender<GpuOutput>,
}

/// r768: one-shot request to run a full HMI present + window dump on the
/// GPU thread without a guest eglSwapBuffers (the app idles GL-free once
/// its startup cascades end; the popup widget-hide channel renders the
/// main view but nothing presents it).  Presentation only - the surface
/// content is exactly what the guest rendered.
static FORCE_PRESENT_REQ: AtomicBool = AtomicBool::new(false);

/// Ask the GPU backend thread to run one HMI present (+ EMU_MAP_DUMP_DIR
/// window dump) at its next opportunity.  Safe before the backend thread
/// exists (the flag simply waits for it).
pub fn request_forced_present() {
    FORCE_PRESENT_REQ.store(true, Ordering::Relaxed);
}

/// GLSL ES sources substituted for the map engine's precompiled (Vivante
/// `glShaderBinary`) shader blobs, which no other GPU can load.  The map
/// engine's `rl_tclOpenGLShader` resolves *all* of its interface by name
/// (`vFindUniformLocations`/`vFindAttribLocations` via
/// `glGetActiveUniform`/`glGetUniformLocation`), so a synthesized program
/// only has to declare the engine's own attribute/uniform vocabulary:
/// `a_position`/`a_color` (see the table at 0x68b960) and `u_matModelView`/
/// `u_matViewProj` (uploaded by `rl_tclOpenGLShader::vUpdateShaderUniforms`
/// into program-slot offsets +0xec/+0xf0).  r659: street elements draw with
/// an empty slot 0x19 (`shd_42100000.glsl` is a `[BinaryShader]` manifest
/// only), binding 526 VBOs but reaching no `glDraw*`.
const SYNTH_VERT_SHADER: &str = "attribute vec2 a_VertexXY;
attribute vec2 a_v2DirVecs;
attribute float a_VertexZ;
attribute vec4 a_color;
uniform mat4 u_matViewProj;
uniform mat4 u_matModelView;
uniform mat4 u_m4MetersModelViewWOTransVec;
uniform mat4 u_matNormal;
uniform mat4 u_m4ProjMat;
uniform mat4 u_m4ProjMatStreets;
uniform float u_flLineWidth;
uniform vec2 u_v2Pan;
varying vec4 v_color;
varying vec4 v_keep;
void main() {
    // Reconstructed from `shd_42100000v.bin`'s embedded symbol table
    // (a_VertexXY, a_v2DirVecs, a_VertexZ, a_color, u_flLineWidth,
    // u_matModelView, u_m4ProjMatStreets).  r700 readback of the road VBO
    // settles the operand roles: the section behind the `a_VertexXY` leg
    // (offset 0) holds near-unit extrusion vectors (+-1) while the section
    // behind the `a_v2DirVecs` leg (offset count*8) holds the block-local
    // line positions (25..230) - the names are swapped relative to the
    // r682 guess, and the extrusion length is already baked into the
    // vectors, so a plain sum reproduces the thick line for every element
    // family (families that only feed `a_VertexXY` see a zero constant in
    // the other attrib).  `u_flLineWidth` must still resolve as a uniform
    // for the engine's location table (slot +0x1c), so it stays alive via
    // the same `clamp(+-1e-38)` sink NVIDIA cannot fold.  The streets
    // projection is a right-handed perspective (`w = -z_mv`) while the
    // engine's modelview carries the camera at +z and - with DTM off - the
    // vertices at z=0, which would put every road behind the camera; the
    // emulator has no terrain, so fold the modelview z to the negative
    // half-space (a no-op for the families that already sit there).
    vec2 extruded = a_VertexXY + a_v2DirVecs;
    vec4 mvpos = u_matModelView * vec4(extruded, a_VertexZ, 1.0);
    // r725: view-centring pan.  The engine derives the pass modelview
    // translation from `poTransPos3D`, but several element families share
    // the pass and only the street element's DAPI buffer carries the
    // car-centred pan (-4472 = -block_centre * scale, real Krakow SD
    // data).  Rather than let the overlay families win that global race,
    // the emulator - acting as the display engine - centres the view on
    // the geometry itself: `u_v2Pan` is the modelview-space centre of the
    // sampled road vertices of this very draw (all coordinates are the
    // engine's real ones), subtracted here so the block sits mid-screen.
    mvpos.x -= u_v2Pan.x;
    mvpos.y -= u_v2Pan.y;
    mvpos.z = -abs(mvpos.z);
    gl_Position = u_m4ProjMatStreets * mvpos;
    // r711: the r710 probes show the streets projection lands roads at
    // z/w = -4.9..-12.9, so GL clip-space culls every fragment even with
    // real geometry.  The emulator has no terrain or 3D layer to depth-sort
    // against, so pin depth mid-range instead of fighting the RH fold.
    gl_Position.z = 0.5 * gl_Position.w;
    v_color = a_color;
    v_keep = clamp(u_matViewProj[0] + u_m4MetersModelViewWOTransVec[0]
        + u_matNormal[0] + u_m4ProjMat[0] + u_flLineWidth * 1e-38,
        -1e-38, 1e-38);
}
";
const SYNTH_FRAG_SHADER: &str = "precision mediump float;
varying vec4 v_color;
varying vec4 v_keep;
void main() {
    gl_FragColor = v_color + v_keep;
}
";

/// r711: last engine-fed `glVertexAttrib4fv` constant on the map target
/// (attribute index, RGBA); consulted by the `glMultiDrawArraysEXT` arm to
/// decide whether a visible fallback stroke color is needed (the road RGBA
/// rides on `a_color`, which the emulator may never have been given).
static LAST_MAP_ATTRIB4: std::sync::LazyLock<Mutex<Option<(u32, [f32; 4])>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// r725: view-space pan that centres the latest road draw's block,
/// computed by `probe_map_draw_vertices` from the engine's own vertex
/// data and uploaded as `u_v2Pan` before the `glMultiDrawArraysEXT` arm
/// rasterises it.
static LAST_MAP_PAN: std::sync::LazyLock<Mutex<Option<(f32, f32)>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Guest arenas backing `glMapBufferOES` (target -> (address, capacity, last
/// requested size)); shared by the map and unmap arms of `gl_api`.
static MAP_BUFFER_ARENAS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<u32, (u32, usize, usize)>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Set once the engine's `vUpdateVBO` gate passes (street-coord probe in
/// `procmapengine.rs`): from that instant the road VBOs carry real geometry,
/// and the map draw probe starts observing big strips (clocks of the engine
/// and GPU backend threads are not aligned, so the draw probe keys on this
/// fact instead of elapsed time).
static ROAD_GEOMETRY_UPLOADED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn note_road_geometry_uploaded() {
    ROAD_GEOMETRY_UPLOADED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn road_geometry_uploaded() -> bool {
    ROAD_GEOMETRY_UPLOADED.load(std::sync::atomic::Ordering::Relaxed)
}

const GL_VERTEX_SHADER_TYPE: u32 = 0x8b31;

/// `glCreateShader` type per shader object, so `glShaderBinary` can tell
/// vertex from fragment.  Global (not thread-local): guest GL calls can be
/// routed from different host workers than the ones that queried the state.
static GL_SHADER_STAGES: std::sync::LazyLock<Mutex<HashMap<u32, u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
/// Synthesized source waiting for `glCompileShader` after a
/// `glShaderBinary` substitution.
static GL_SYNTH_SHADER_SOURCE: std::sync::LazyLock<Mutex<HashMap<u32, String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

thread_local! {
    static PENDING_GL_ERROR: Cell<bool> = const { Cell::new(false) };
    /// Caps the map-target matrix-upload trace.
    static MATRIX_LOG_SEEN: Cell<u32> = const { Cell::new(0) };
    static API_LOG_COUNT: Cell<u32> = const { Cell::new(0) };
    static REAL_GL_OUTPUT_LOG_COUNT: Cell<u32> = const { Cell::new(0) };
    static EGL_CURRENT_API: Cell<u32> = const { Cell::new(EGL_OPENGL_ES_API) };
    static CURRENT_GPU_TARGET: Cell<Option<GpuTarget>> = const { Cell::new(None) };
}

const MAP_SURFACE_WIDTH: usize = 800;
const MAP_SURFACE_HEIGHT: usize = 480;
const MAP_SURFACE_SIZE: usize = MAP_SURFACE_WIDTH * MAP_SURFACE_HEIGHT * 4;
const MAP_SURFACE_HANDLE: u32 = 0x5f4d4150;

static MAP_SURFACE_DIRTY: AtomicBool = AtomicBool::new(false);
static MAP_SURFACE_BYTES: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Newest matrix per uniform location uploaded by the map engine, used by
/// the draw-time clip-coordinate probe.
static LAST_UPLOADED_MATRICES: std::sync::LazyLock<Mutex<HashMap<i32, [f32; 16]>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

// Display-engine state of the map layer, as requested by the HMI through the
// svg layer API (see `libs::prochmi`'s layer control hooks): `*_ENABLED` is
// `svgEnableLayer`/`svgDisableLayer` for the map layer handle and the window is
// what `LayerSync::setLayersVisible` hands over.  The compositor substitutes map
// pixels only inside that window, the way the hardware layer compositor shows a
// layer that the GUI asked to be visible.
static MAP_LAYER_ENABLED: AtomicBool = AtomicBool::new(false);
static MAP_LAYER_X: AtomicU32 = AtomicU32::new(0);
static MAP_LAYER_Y: AtomicU32 = AtomicU32::new(0);
static MAP_LAYER_W: AtomicU32 = AtomicU32::new(0);
static MAP_LAYER_H: AtomicU32 = AtomicU32::new(0);
static MAP_LAYER_SIGNATURE: AtomicU64 = AtomicU64::new(u64::MAX);
static MAP_LAYER_PUBLISHES: AtomicU32 = AtomicU32::new(0);
static MAP_LAYER_HOLE_LOGS: AtomicU32 = AtomicU32::new(0);

/// Publish the map layer's visibility and screen window (top-left origin, in
/// screen pixels) so the frame compositor knows where the layer below the HMI
/// layer is actually on.
pub fn set_map_layer_window(enabled: bool, x: u32, y: u32, w: u32, h: u32) {
    let pack = |enabled: bool, x: u32, y: u32, w: u32, h: u32| -> u64 {
        ((enabled as u64) << 63)
            | ((x as u64 & 0xffff) << 40)
            | ((y as u64 & 0xffff) << 24)
            | ((w as u64 & 0xffffff) << 12)
            | (h as u64 & 0xfff)
    };
    let signature = pack(enabled, x, y, w, h);
    MAP_LAYER_ENABLED.store(enabled, Ordering::Relaxed);
    MAP_LAYER_X.store(x, Ordering::Relaxed);
    MAP_LAYER_Y.store(y, Ordering::Relaxed);
    MAP_LAYER_W.store(w, Ordering::Relaxed);
    MAP_LAYER_H.store(h, Ordering::Relaxed);
    if MAP_LAYER_SIGNATURE.swap(signature, Ordering::Relaxed) != signature {
        let hits = MAP_LAYER_PUBLISHES.fetch_add(1, Ordering::Relaxed);
        if hits < 40 {
            log::info!(
                "GPU: map layer window published enabled={} rect=({},{},{},{})",
                enabled,
                x,
                y,
                w,
                h
            );
        }
    }
}

// Client-array emulation for glVertexAttribPointer: guest vertices live in
// guest RAM and are re-specified every frame with the SAME pointers. One
// scratch read buffer + one host VBO per distinct pointer (with a content
// hash so unchanged geometry skips BufferData entirely) keeps this steady
// state allocation-free. The old path malloc'd a 64 KB Vec and minted a
// throwaway VBO per call (~15x/swap), leaking driver buffers and growing the
// process RSS watermark by ~200 MB per 10 minutes (the "GPU memory leak").
static VAP_SCRATCH: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static VAP_CACHE: std::sync::LazyLock<Mutex<std::collections::HashMap<u32, (u32, u64)>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
/// Map-target client-array registrations: attrib index -> (guest pointer,
/// copied length, last-copied content hash, backing VBO).  Consulted by
/// `refresh_map_client_arrays` right before every engine `glDrawArrays`.
static VAP_ATTRIB_REGS: std::sync::LazyLock<
    Mutex<std::collections::HashMap<u32, (u32, usize, u64, u32)>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
static MAP_SURFACE_GUEST_BASES: OnceLock<Mutex<HashMap<u32, u32>>> = OnceLock::new();

static GPU_SENDER: OnceLock<mpsc::Sender<GpuCommand>> = OnceLock::new();
static GPU_START: Once = Once::new();
static GPU_FAILED: AtomicBool = AtomicBool::new(false);
static GPU_MAKE_CURRENT_ERROR_LOGGED: AtomicBool = AtomicBool::new(false);
static GPU_TARGET_SWITCH_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
static GL_ERROR_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_CAPTURE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_RAW_CAPTURE_COUNT: AtomicU32 = AtomicU32::new(0);
static HMI_COMPOSITE_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

// IDs of the per-target private "default framebuffer" substitutes, published
// by the backend thread once so the guest-side dispatch (running on guest
// threads) can rewrite glBindFramebuffer(_*, 0) and shadow
// glGetIntegerv(GL_FRAMEBUFFER_BINDING) without round-tripping a command.
static HMI_DEFAULT_FBO: AtomicU32 = AtomicU32::new(0);
static MAP_DEFAULT_FBO: AtomicU32 = AtomicU32::new(0);

fn private_default_fbo(target: GpuTarget) -> u32 {
    match target {
        GpuTarget::Hmi => HMI_DEFAULT_FBO.load(Ordering::Acquire),
        GpuTarget::Map => MAP_DEFAULT_FBO.load(Ordering::Acquire),
    }
}

const GL_FRAMEBUFFER_BINDING: u32 = 0x8ca6;
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8caa;

// Guest processes occasionally bind framebuffer names they never obtained
// from glGenFramebuffers (prochmi binds fb 2, which is the same numeric name
// as our HMI private default FBO). With unshared contexts the host would
// happily hand that name to whichever guest binds it first, aliasing the two
// tenants' objects. This table gives such a colliding guest name its own
// fresh host object per target, so each process keeps a private ID space.
fn fbo_aliases() -> &'static Mutex<HashMap<(u8, u32), u32>> {
    static ALIASES: OnceLock<Mutex<HashMap<(u8, u32), u32>>> = OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn target_key(target: GpuTarget) -> u8 {
    match target {
        GpuTarget::Hmi => 0,
        GpuTarget::Map => 1,
    }
}

// Translate a guest framebuffer name to its host name. Runs on the backend
// thread with the owning target's context current.
unsafe fn resolve_guest_fbo(target: GpuTarget, guest_id: u32) -> u32 {
    if guest_id == 0 {
        let private = private_default_fbo(target);
        return if private != 0 { private } else { 0 };
    }
    let key = (target_key(target), guest_id);
    let private = private_default_fbo(target);
    let mut aliases = match fbo_aliases().lock() {
        Ok(map) => map,
        Err(_) => return guest_id,
    };
    if let Some(host) = aliases.get(&key) {
        return *host;
    }
    if guest_id == private {
        let mut host = 0u32;
        gl::GenFramebuffers(1, &mut host);
        if host != 0 {
            aliases.insert(key, host);
            log::info!(
                "GPU: fbo alias target={:?} guest={} host={}",
                target,
                guest_id,
                host
            );
        }
        return host;
    }
    guest_id
}

// Reverse map a host framebuffer binding back to what the owning guest
// believes is bound (0 for the private default, the alias name when the bound
// object stands in for a guessed guest name).
fn guest_framebinding(target: GpuTarget, host_id: u32) -> u32 {
    if host_id == 0 {
        return 0;
    }
    let private = private_default_fbo(target);
    if private != 0 && host_id == private {
        return 0;
    }
    if let Ok(aliases) = fbo_aliases().lock() {
        for ((key_target, guest_id), host) in aliases.iter() {
            if *host == host_id && *key_target == target_key(target) {
                return *guest_id;
            }
        }
    }
    host_id
}


struct Backend {
    _sdl: sdl2::Sdl,
    _video: sdl2::VideoSubsystem,
    window: Window,
    window_raw: *mut sys::SDL_Window,
    hmi_context: sys::SDL_GLContext,
    map_context: sys::SDL_GLContext,
    // Private host-compositor context: the ONLY context that ever renders to
    // (and swaps) the SDL window. Guests rely on GL state (bound TEXTURE_2D,
    // current program, enabled attrib arrays) persisting between their own GL
    // calls across swaps - mixLayers draws without binding textures/programs.
    // Running our window blit on the HMI context trashed exactly that state
    // and made the whole HMI layer render black.
    comp_context: sys::SDL_GLContext,
    // Per-target private "default framebuffer" substitutes. Guests issue
    // glBindFramebuffer(_*, 0) expecting their own window surface; we bind
    // these once as each context's initial framebuffer and rewrite guest
    // binds of 0 to them (see `resolve_guest_fbo`), so nothing ever touches
    // the SDL window except our final composite blit. Each is allocated in
    // its own unshared context, so the IDs live in that guest's namespace.
    hmi_framebuffer: u32,
    map_framebuffer: u32,
    hmi_pixels: Vec<u8>,
    map_pixels: Vec<u8>,
    composite_program: u32,
    composite_texture: u32,
    composite_vbo: u32,
    composite_pos_loc: i32,
    current_target: Option<GpuTarget>,
    events: Option<EventPump>,
}

fn gpu_backend_visible() -> bool {
    std::env::var("EMU_GPU_VISIBLE")
        .map(|value| value != "0" && !value.is_empty())
        .unwrap_or(true)
}

fn gpu_target_for_elf(elf_path: &str) -> GpuTarget {
    if elf_path.contains("procmapengine") {
        GpuTarget::Map
    } else {
        GpuTarget::Hmi
    }
}

fn set_gpu_target_for_elf(elf_path: &str) -> GpuTarget {
    let target = gpu_target_for_elf(elf_path);
    CURRENT_GPU_TARGET.with(|slot| slot.set(Some(target)));
    target
}

fn current_gpu_target() -> GpuTarget {
    CURRENT_GPU_TARGET.with(|slot| slot.get()).unwrap_or(GpuTarget::Hmi)
}

pub fn map_surface_is_dirty() -> bool {
    MAP_SURFACE_DIRTY.load(Ordering::Acquire)
}

pub fn take_map_surface() -> Option<Vec<u8>> {
    if MAP_SURFACE_DIRTY.swap(false, Ordering::AcqRel) {
        MAP_SURFACE_BYTES.lock().ok().map(|bytes| bytes.clone())
    } else {
        None
    }
}

fn map_surface_bases() -> &'static Mutex<HashMap<u32, u32>> {
    MAP_SURFACE_GUEST_BASES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn write_map_surface_to_guest(unicorn: &mut Unicorn<'_, Context>) -> Option<u32> {
    let process_id = unicorn.get_data().process_id;
    let existing = map_surface_bases()
        .lock()
        .ok()
        .and_then(|bases| bases.get(&process_id).copied());

    let bytes = latest_map_surface_bytes();

    if let Some(base) = existing {
        unicorn
            .mem_write(base as u64, &bytes)
            .ok()
            .map(|_| base)
            .or_else(|| Some(base))
    } else {
        let base = {
            let mmu = unicorn.get_data().mmu.clone();
            let allocated = mmu.lock().unwrap().heap_alloc(
                unicorn,
                MAP_SURFACE_SIZE as u32,
                Prot::READ | Prot::WRITE,
                "[svg-map-surface]",
            );
            allocated
        };
        if base == 0 {
            return None;
        }
        if let Ok(mut bases) = map_surface_bases().lock() {
            bases.insert(process_id, base);
        }
        log::info!(
            "GPU: allocated SVG map surface copy process={} guest={:#x} size={:#x}",
            process_id,
            base,
            MAP_SURFACE_SIZE
        );
        unicorn
            .mem_write(base as u64, &bytes)
            .ok()
            .map(|_| base)
            .or_else(|| Some(base))
    }
}

fn latest_map_surface_bytes() -> Vec<u8> {
    let bytes = MAP_SURFACE_BYTES
        .lock()
        .ok()
        .map(|bytes| bytes.clone())
        .unwrap_or_default();
    if bytes.len() == MAP_SURFACE_SIZE {
        bytes
    } else {
        vec![0u8; MAP_SURFACE_SIZE]
    }
}

pub fn map_surface_guest_base(unicorn: &Unicorn<'_, Context>) -> Option<u32> {
    let process_id = unicorn.get_data().process_id;
    map_surface_bases()
        .lock()
        .ok()
        .and_then(|bases| bases.get(&process_id).copied())
}

fn publish_map_surface_pixels(pixels: &[u8]) {
    static PUBLISH_COUNT: AtomicU32 = AtomicU32::new(0);
    let n = PUBLISH_COUNT.fetch_add(1, Ordering::Relaxed);
    if n < 60 || n % 60 == 0 {
        let mut nonblack = 0usize;
        let mut nonblack_visible = 0usize;
        let mut saturated = 0usize;
        let mut distinct = std::collections::HashSet::new();
        for px in pixels.chunks(4) {
            if px[0] | px[1] | px[2] != 0 {
                nonblack += 1;
                if px[3] != 0 {
                    nonblack_visible += 1;
                }
            }
            if (px[0] as i32 - px[1] as i32).abs().max((px[1] as i32 - px[2] as i32).abs()) > 40 {
                saturated += 1;
            }
            if distinct.len() < 4096 {
                distinct.insert((px[0], px[1], px[2]));
            }
        }
        log::info!(
            "GPU: map surface publish n={} nonblack={} visible={} saturated={} distinct={} sample={:02x?}",
            n,
            nonblack,
            nonblack_visible,
            saturated,
            distinct.len(),
            &pixels[(240 * 800 + 400) * 4..(240 * 800 + 400) * 4 + 4]
        );
        // Visual check of the map-target readback: dump the raw RGBA as PPM
        // (`EMU_MAP_DUMP_DIR`), bottom-up rows flipped so the image matches
        // the screen orientation.  Used to tell an empty engine FBO readback
        // apart from pixels lost later in the HMI compositing chain.
        if let Ok(dir) = std::env::var("EMU_MAP_DUMP_DIR") {
            let mut out: Vec<u8> = Vec::with_capacity(32 + MAP_SURFACE_WIDTH * MAP_SURFACE_HEIGHT * 3);
            out.extend_from_slice(format!(
                "P6\n{} {}\n255\n",
                MAP_SURFACE_WIDTH, MAP_SURFACE_HEIGHT
            ).as_bytes());
            for row in pixels
                .chunks(MAP_SURFACE_WIDTH * 4)
                .rev()
                .take(MAP_SURFACE_HEIGHT)
            {
                for px in row.chunks(4) {
                    out.push(px[0]);
                    out.push(px[1]);
                    out.push(px[2]);
                }
            }
            let path = format!("{}/map_pub_{:04}.ppm", dir, n);
            let _ = std::fs::write(&path, out);
        }
    }
    if let Ok(mut host) = MAP_SURFACE_BYTES.lock() {
        *host = pixels.to_vec();
        MAP_SURFACE_DIRTY.store(true, Ordering::Release);
    }
}

/// r728: read the SDL window's default framebuffer on the compositor
/// context (right after `blit_hmi_pixels_to_window` painted the composed
/// HMI-over-map frame) and dump it as PPM to `EMU_MAP_DUMP_DIR`, every
/// 20th frame for the first 400 swaps.
fn dump_window_pixels(backend: &mut Backend) {
    let Ok(dir) = std::env::var("EMU_MAP_DUMP_DIR") else {
        return;
    };
    static WINDOW_NEXT: std::sync::LazyLock<std::sync::Mutex<std::time::Instant>> =
        std::sync::LazyLock::new(|| Mutex::new(std::time::Instant::now()));
    static WINDOW_DUMPS: AtomicU32 = AtomicU32::new(0);
    let due = {
        let Ok(next) = WINDOW_NEXT.lock() else {
            return;
        };
        std::time::Instant::now() >= *next
    };
    static DUMP_CALLS: AtomicU32 = AtomicU32::new(0);
    let c = DUMP_CALLS.fetch_add(1, Ordering::Relaxed);
    let _ = c;
    if !due {
        return;
    }
    let n = WINDOW_DUMPS.fetch_add(1, Ordering::Relaxed);
    if n >= 40 {
        return;
    }
    if let Ok(mut next) = WINDOW_NEXT.lock() {
        // r760: interval knob so short validation runs (<30 s) can capture a
        // post-milestone frame too (EMU_MAP_DUMP_SECS, default 30).
        let secs: u64 = std::env::var("EMU_MAP_DUMP_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        *next = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    }
    let (mut draw_w, mut draw_h) = (800i32, 480i32);
    let mut win_w = 800i32;
    let mut win_h = 480i32;
    unsafe {
        sys::SDL_GetWindowSize(backend.window_raw, &mut win_w, &mut win_h);
        sys::SDL_GL_GetDrawableSize(backend.window_raw, &mut draw_w, &mut draw_h);
    }
    if draw_w <= 0 || draw_h <= 0 || draw_w > 4096 || draw_h > 4096 {
        return;
    }
    let mut pixels = vec![0u8; (draw_w as usize) * (draw_h as usize) * 4];
    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        gl::Finish();
        gl::ReadPixels(
            0,
            0,
            draw_w,
            draw_h,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            pixels.as_mut_ptr() as *mut _,
        );
        drain_host_gl_errors();
    }
    let mut out: Vec<u8> =
        Vec::with_capacity(32 + (draw_w as usize) * (draw_h as usize) * 3);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", draw_w, draw_h).as_bytes());
    for row in pixels.chunks(draw_w as usize * 4).rev() {
        for px in row.chunks(4) {
            out.push(px[0]);
            out.push(px[1]);
            out.push(px[2]);
        }
    }
    let path = format!("{}/window_{:04}.ppm", dir, n);
    let _ = std::fs::write(&path, out);
    log::info!(
        "GPU: window dump {} ({}x{}, drawable {}x{}) -> {}",
        n,
        win_w,
        win_h,
        draw_w,
        draw_h,
        path
    );
}

unsafe fn compile_helper_shader(kind: u32, src: *const u8) -> u32 {
    let shader = gl::CreateShader(kind);
    gl::ShaderSource(shader, 1, &src, core::ptr::null());
    gl::CompileShader(shader);
    let mut ok = 0i32;
    gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut ok);
    if ok != gl::TRUE as i32 {
        let mut info = [0u8; 512];
        gl::GetShaderInfoLog(shader, info.len() as i32, core::ptr::null_mut(), info.as_mut_ptr());
        let msg = std::ffi::CStr::from_ptr(info.as_ptr() as *const core::ffi::c_char)
            .to_string_lossy()
            .into_owned();
        log::warn!(
            "GPU: composite shader compile failed kind={:#x}: {}",
            kind,
            msg.trim()
        );
        gl::DeleteShader(shader);
        return 0;
    }
    shader
}

// Build an off-screen (RGBA8 colour texture + DEPTH_COMPONENT16 renderbuffer)
// framebuffer. Used for both the HMI and Map private "default framebuffer"
// substitutes; each lives in the owning target's context so IDs stay in that
// guest's private namespace. Must be called with the target context current.
unsafe fn create_default_fbo(width: i32, height: i32) -> (u32, u32, u32) {
    let mut fbo = 0u32;
    let mut color = 0u32;
    let mut depth_rb = 0u32;
    gl::GenFramebuffers(1, &mut fbo);
    gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);

    gl::GenTextures(1, &mut color);
    gl::BindTexture(gl::TEXTURE_2D, color);
    gl::TexImage2D(
        gl::TEXTURE_2D,
        0,
        gl::RGBA as i32,
        width,
        height,
        0,
        gl::RGBA,
        gl::UNSIGNED_BYTE,
        core::ptr::null(),
    );
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
    gl::FramebufferTexture2D(
        gl::FRAMEBUFFER,
        gl::COLOR_ATTACHMENT0,
        gl::TEXTURE_2D,
        color,
        0,
    );

    gl::GenRenderbuffers(1, &mut depth_rb);
    gl::BindRenderbuffer(gl::RENDERBUFFER, depth_rb);
    gl::RenderbufferStorage(gl::RENDERBUFFER, gl::DEPTH_COMPONENT16, width, height);
    gl::FramebufferRenderbuffer(
        gl::FRAMEBUFFER,
        gl::DEPTH_ATTACHMENT,
        gl::RENDERBUFFER,
        depth_rb,
    );

    gl::BindTexture(gl::TEXTURE_2D, 0);
    gl::BindRenderbuffer(gl::RENDERBUFFER, 0);
    if gl::CheckFramebufferStatus(gl::FRAMEBUFFER) != gl::FRAMEBUFFER_COMPLETE {
        gl::DeleteFramebuffers(1, &fbo);
        gl::DeleteTextures(1, &color);
        gl::DeleteRenderbuffers(1, &depth_rb);
        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        return (0, 0, 0);
    }
    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
    (fbo, color, depth_rb)
}

fn init_backend() -> Result<Backend, String> {
    let visible = gpu_backend_visible();
    let sdl = sdl2::init()?;
    let video = sdl.video()?;

    {
        let attrs = video.gl_attr();
        // Guests (procmap, prochmi) compile GLSL ES 1.00 shaders against
        // GLES 2 semantics. NVIDIA's ES profile honours that; the desktop
        // Compatibility profile accepts the source but changes precision and
        // built-in behaviour, which visibly breaks prochmi's HMI (missing
        // blue buttons, popups). Keep the ES profile and do the SVG-layer
        // merge with a GLES-2 fullscreen-quad shader instead of the fixed-
        // function glDrawPixels path (ES has no DrawPixels at all).
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

    let window_raw = window.raw();
    let hmi_context = unsafe { sys::SDL_GL_CreateContext(window_raw) };
    if hmi_context.is_null() {
        return Err("SDL_GL_CreateContext(HMI) returned null".to_string());
    }
    gl::load_with(|name| video.gl_get_proc_address(name) as *const _);
    let _ = video.gl_set_swap_interval(1);
    unsafe {
        let version = gl::GetString(gl::VERSION);
        let renderer = gl::GetString(gl::RENDERER);
        let v = if version.is_null() {
            "<null>".to_string()
        } else {
            std::ffi::CStr::from_ptr(version as *const core::ffi::c_char)
                .to_string_lossy()
                .into_owned()
        };
        let r = if renderer.is_null() {
            "<null>".to_string()
        } else {
            std::ffi::CStr::from_ptr(renderer as *const core::ffi::c_char)
                .to_string_lossy()
                .into_owned()
        };
        log::info!("GPU: hmi_context GL version='{}' renderer='{}'", v, r);
    }

    unsafe {
        // Real target has two isolated EGLDisplay+context pairs (procmap owns
        // one, prochmi owns another). Their GL object ID spaces do NOT
        // overlap: procmap's FBO 3 and prochmi's FBO 3 are different objects.
        // We must not share resources between our contexts either, otherwise
        // glGen* returns IDs from a common pool and the two guests collide.
        sys::SDL_GL_SetAttribute(
            sys::SDL_GLattr::SDL_GL_SHARE_WITH_CURRENT_CONTEXT,
            0,
        );
    }
    let map_context = unsafe { sys::SDL_GL_CreateContext(window_raw) };
    if map_context.is_null() {
        return Err("SDL_GL_CreateContext(map) returned null".to_string());
    }

    // Host-only compositor context (unshared like the others; its private
    // texture/program/VBO IDs never touch guest namespaces).
    let comp_context = unsafe { sys::SDL_GL_CreateContext(window_raw) };
    if comp_context.is_null() {
        return Err("SDL_GL_CreateContext(compositor) returned null".to_string());
    }

    // Per-target private default framebuffers. Guests issue
    // glBindFramebuffer(_*, 0) intending "my window surface"; we redirect that
    // to these (see `gl_backend_api`). Rendering stays off-screen; we present a
    // merged image to the visible SDL window ourselves. Each is also bound ONCE
    // here as the initial framebuffer of its context - procmap never calls
    // glBindFramebuffer at all and expects to draw into its window from the
    // start. After this the backend never touches a guest's framebuffer
    // binding on context switches; the unshared contexts keep each guest's
    // binding state private across MakeCurrent, like real EGLDisplay's.
    let (map_framebuffer, _, _) = unsafe {
        sys::SDL_GL_MakeCurrent(window_raw, map_context);
        let default_fbo = create_default_fbo(800, 480);
        if default_fbo.0 != 0 {
            gl::BindFramebuffer(gl::FRAMEBUFFER, default_fbo.0);
        }
        default_fbo
    };
    if map_framebuffer == 0 {
        log::warn!("GPU: map default FBO incomplete, map compositing disabled");
    }
    MAP_DEFAULT_FBO.store(map_framebuffer, Ordering::Release);

    let map_pixels = vec![0u8; MAP_SURFACE_SIZE];
    let hmi_pixels = vec![0u8; MAP_SURFACE_SIZE];

    let (hmi_framebuffer, _, _) = unsafe {
        sys::SDL_GL_MakeCurrent(window_raw, hmi_context);
        create_default_fbo(800, 480)
    };
    if hmi_framebuffer == 0 {
        log::warn!("GPU: hmi default FBO incomplete, HMI compositing disabled");
    }
    HMI_DEFAULT_FBO.store(hmi_framebuffer, Ordering::Release);

    // GLES 2 composite pipeline: on every prochmi swap we upload the CPU
    // merged (hmi-over-map) RGBA buffer as a texture and draw a fullscreen
    // quad to the window. GLES 2 has no glDrawPixels and no fixed-function
    // pipeline, so a minimal shader is required.
    let mut composite_program = 0u32;
    let mut composite_texture = 0u32;
    let mut composite_vbo = 0u32;
    let mut composite_pos_loc = -1i32;
    unsafe {
        sys::SDL_GL_MakeCurrent(window_raw, comp_context);
        let vs_src = b"attribute vec2 a_pos;\
            varying vec2 v_uv;\
            void main() { v_uv = a_pos * 0.5 + 0.5; \
            gl_Position = vec4(a_pos, 0.0, 1.0); }\0";
        let fs_src = b"precision mediump float;\
            varying vec2 v_uv;\
            uniform sampler2D u_tex;\
            void main() { gl_FragColor = texture2D(u_tex, v_uv); }\0";
        let vs = compile_helper_shader(gl::VERTEX_SHADER, vs_src.as_ptr());
        let fs = compile_helper_shader(gl::FRAGMENT_SHADER, fs_src.as_ptr());
        if vs != 0 && fs != 0 {
            let prog = gl::CreateProgram();
            gl::AttachShader(prog, vs);
            gl::AttachShader(prog, fs);
            gl::BindAttribLocation(prog, 0, b"a_pos\0".as_ptr());
            gl::LinkProgram(prog);
            let mut ok = 0i32;
            gl::GetProgramiv(prog, gl::LINK_STATUS, &mut ok);
            if ok == gl::TRUE as i32 {
                composite_program = prog;
                composite_pos_loc = 0;
            } else {
                log::warn!("GPU: composite shader link failed; HMI blit disabled");
            }
            gl::DeleteShader(vs);
            gl::DeleteShader(fs);
        } else {
            log::warn!("GPU: composite shader compile failed; HMI blit disabled");
        }
        if composite_program != 0 {
            gl::GenTextures(1, &mut composite_texture);
            gl::BindTexture(gl::TEXTURE_2D, composite_texture);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA as i32,
                MAP_SURFACE_WIDTH as i32,
                MAP_SURFACE_HEIGHT as i32,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                core::ptr::null(),
            );
            let filter = if scale_filter_is_nearest() {
                gl::NEAREST
            } else {
                gl::LINEAR
            };
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, filter as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, filter as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
            let quad: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            gl::GenBuffers(1, &mut composite_vbo);
            gl::BindBuffer(gl::ARRAY_BUFFER, composite_vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (quad.len() * core::mem::size_of::<f32>()) as isize,
                quad.as_ptr() as *const core::ffi::c_void,
                gl::STATIC_DRAW,
            );
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
        }
        gl::BindTexture(gl::TEXTURE_2D, 0);
        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
    }

    let events = if visible {
        Some(sdl.event_pump()?)
    } else {
        None
    };

    unsafe {
        sys::SDL_GL_MakeCurrent(window_raw, comp_context);
        gl::Viewport(0, 0, 800, 480);
        gl::ClearColor(0.0, 0.0, 0.0, 1.0);
        gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
    }
    if visible {
        let _ = window.gl_swap_window();
    }
    // Hand the HMI context back to its guest with its private window-surface
    // stand-in bound (create_default_fbo unbinds to 0 on purpose).
    unsafe {
        sys::SDL_GL_MakeCurrent(window_raw, hmi_context);
    }
    if hmi_framebuffer != 0 {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, hmi_framebuffer);
        }
    }

    log::info!(
        "GPU: dedicated SDL/GL backend thread started visible={} map_fbo={} hmi_fbo={} thread={:?}",
        visible,
        map_framebuffer,
        hmi_framebuffer,
        std::thread::current().id()
    );

    Ok(Backend {
        _sdl: sdl,
        _video: video,
        window,
        window_raw,
        hmi_context,
        map_context,
        comp_context,
        hmi_framebuffer,
        map_framebuffer,
        hmi_pixels,
        map_pixels,
        composite_program,
        composite_texture,
        composite_vbo,
        composite_pos_loc,
        current_target: Some(GpuTarget::Hmi),
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
                    // Only a log: `glViewport` here would apply the *logical*
                    // window size to whichever context happens to be current,
                    // while the frame blit sets the viewport from
                    // `SDL_GL_GetDrawableSize` (the drawable's real pixel size)
                    // on every swap anyway.
                    let hits = WINDOW_RESIZE_EVENT_LOGS.fetch_add(1, Ordering::Relaxed);
                    if hits < 40 {
                        log::info!("GPU: window resized event to {}x{}", width, height);
                    }
                }
            }
            _ => {}
        }
    }

    false
}

fn make_target_current(backend: &mut Backend, target: GpuTarget) {
    if backend.current_target == Some(target) {
        return;
    }

    let context = match target {
        GpuTarget::Hmi => backend.hmi_context,
        GpuTarget::Map => backend.map_context,
    };

    let result = unsafe { sys::SDL_GL_MakeCurrent(backend.window_raw, context) };
    let count = GPU_TARGET_SWITCH_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 20 {
        log::info!(
            "GPU: target switch from {:?} to {:?} make_current={} map_fbo={}",
            backend.current_target,
            target,
            result,
            backend.map_framebuffer
        );
    }
    if result != 0 {
        if !GPU_MAKE_CURRENT_ERROR_LOGGED.swap(true, Ordering::Relaxed) {
            log::warn!("GPU: failed to make target {:?} GL context current", target);
        }
        return;
    }

    // Do NOT re-bind any framebuffer here. Each context is unshared and
    // persists its own GL_*_FRAMEBUFFER_BINDING across MakeCurrent, exactly
    // like the guests' real EGLDisplay contexts. The private default FBO was
    // bound once at init as the context's initial "window surface"; re-binding
    // it on every switch clobbered whatever offscreen FBO layer the guest had
    // bound mid-frame whenever the other process's commands interleaved,
    // which made the two layers bleed into each other nondeterministically.
    backend.current_target = Some(target);
}

fn vap_legacy() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EMU_VAP_LEGACY").is_ok())
}

/// Window upscale filter for the 800x480 panel image: `nearest` (the default)
/// shows panel pixels as blocks with no smoothing, `EMU_GPU_SCALE_FILTER=linear`
/// restores bilinear smoothing.
fn scale_filter_is_nearest() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        let wanted = std::env::var("EMU_GPU_SCALE_FILTER")
            .unwrap_or_else(|_| "nearest".to_string())
            .trim()
            .to_ascii_lowercase();
        let nearest = wanted != "linear" && wanted != "bilinear" && wanted != "smooth";
        log::info!(
            "GPU: window scale filter = {} (EMU_GPU_SCALE_FILTER=linear for smoothing)",
            if nearest { "nearest" } else { "linear" }
        );
        nearest
    })
}

fn probe_clears() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EMU_GPU_PROBE_CLEARS").is_ok())
}

// True (the default) when the HMI's own window surface is cleared as a
// compositor layer would be, i.e. transparently, so the map layer underneath
// can show through.  Set EMU_GPU_HMI_CLEAR_OPAQUE to compare against the

fn hmi_capture_prefix() -> Option<String> {
    std::env::var("EMU_GPU_HMI_CAPTURE_PREFIX")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

// Host-side re-upload of one HMI-context texture from the shared map surface
// (used to keep the LayerSync map snapshot in sync with procmap's live map
// surface, since the snapshot is taken once when the map layer hides). The
// closure reads the global pixels on the GPU thread itself: copying 1.5 MB
// per refresh through a channel-captured Vec churned the allocator and grew
// the RSS watermark indefinitely (GPU memory "leak").
pub fn refresh_texture_from_map_surface(name: u32, width: i32, height: i32) {
    if name == 0 {
        return;
    }
    if !ensure_gpu_thread() {
        return;
    }
    let Some(sender) = GPU_SENDER.get() else {
        return;
    };
    let (reply_tx, _reply_rx) = mpsc::channel();
    let _ = sender.send(GpuCommand {
        target: GpuTarget::Hmi,
        future: Box::new(move || {
            let Ok(bytes) = MAP_SURFACE_BYTES.lock() else {
                return GpuOutput::default();
            };
            if bytes.len() == (width * height * 4) as usize {
                unsafe {
                    let mut previous_tex = 0i32;
                    gl::GetIntegerv(gl::TEXTURE_BINDING_2D, &mut previous_tex);
                    gl::BindTexture(gl::TEXTURE_2D, name);
                    gl::TexSubImage2D(
                        gl::TEXTURE_2D,
                        0,
                        0,
                        0,
                        width,
                        height,
                        gl::RGBA,
                        gl::UNSIGNED_BYTE,
                        bytes.as_ptr() as *const _,
                    );
                    gl::BindTexture(gl::TEXTURE_2D, previous_tex as u32);
                    drain_host_gl_errors();
                }
            }
            if probe_layer_alphas_enabled() {
                static PROBE_FBO: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                unsafe {
                    let mut fbo = PROBE_FBO.load(Ordering::Relaxed);
                    if fbo == 0 {
                        gl::GenFramebuffers(1, &mut fbo);
                        PROBE_FBO.store(fbo, Ordering::Relaxed);
                    }
                    let mut previous = 0i32;
                    let mut previous_tex = 0i32;
                    gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut previous);
                    gl::GetIntegerv(gl::TEXTURE_BINDING_2D, &mut previous_tex);
                    gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
                    gl::BindTexture(gl::TEXTURE_2D, name);
                    gl::FramebufferTexture2D(
                        gl::FRAMEBUFFER,
                        gl::COLOR_ATTACHMENT0,
                        gl::TEXTURE_2D,
                        name,
                        0,
                    );
                    let mut out = [0u8; 3 * 4];
                    for (i, (x, y)) in [(400, 240), (20, 240), (700, 100)].iter().enumerate() {
                        gl::ReadPixels(
                            *x,
                            *y,
                            1,
                            1,
                            gl::RGBA,
                            gl::UNSIGNED_BYTE,
                            out.as_mut_ptr().add(i * 4) as *mut _,
                        );
                    }
                    gl::BindFramebuffer(gl::FRAMEBUFFER, previous as u32);
                    gl::BindTexture(gl::TEXTURE_2D, previous_tex as u32);
                    log::info!(
                        "GPU: snapshot-readback tex={} center={:02x?} left={:02x?} right={:02x?}",
                        name,
                        &out[0..4],
                        &out[4..8],
                        &out[8..12]
                    );
                }
            }
            GpuOutput::default()
        }),
        reply: reply_tx,
    });
}
fn hmi_frame_is_non_black(pixels: &[u8]) -> bool {
    pixels
        .chunks_exact(4)
        .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0)
}

fn should_capture_hmi_frame(frame: u32) -> bool {
    frame < 5 || matches!(frame, 50 | 100 | 200 | 400 | 800 | 2000 | 4000 | 8000)
}

fn save_hmi_capture(prefix: &str, frame: u32, pixels: &[u8], width: i32, height: i32) {
    let path = format!("{}{:04}.ppm", prefix, frame);
    let mut file = match std::fs::File::create(&path) {
        Ok(file) => file,
        Err(err) => {
            log::warn!("GPU: failed to create HMI capture {}: {}", path, err);
            return;
        }
    };

    if write!(file, "P6\n{} {}\n255\n", width, height).is_err() {
        log::warn!("GPU: failed to write HMI capture header {}", path);
        return;
    }

    let row_bytes = (width as usize) * 4;
    for row in (0..height as usize).rev() {
        for rgba in pixels[row * row_bytes..][..row_bytes].chunks_exact(4) {
            if file.write_all(&rgba[..3]).is_err() {
                log::warn!("GPU: failed to write HMI capture pixels {}", path);
                return;
            }
        }
    }
    if file.flush().is_err() {
        log::warn!("GPU: failed to flush HMI capture {}", path);
        return;
    }

    log::info!(
        "GPU: saved HMI capture {} non_black={}",
        path,
        hmi_frame_is_non_black(pixels)
    );
}

fn capture_hmi_framebuffer(_backend: &mut Backend) {
    let Some(prefix) = hmi_capture_prefix() else {
        return;
    };

    let frame = HMI_CAPTURE_COUNT.fetch_add(1, Ordering::Relaxed);
    if !should_capture_hmi_frame(frame) {
        return;
    }

    let width = MAP_SURFACE_WIDTH as i32;
    let height = MAP_SURFACE_HEIGHT as i32;
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        gl::ReadPixels(
            0,
            0,
            width,
            height,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            pixels.as_mut_ptr() as *mut _,
        );
    }
    save_hmi_capture(&prefix, frame, &pixels, width, height);
}

// Diagnostic: read the HMI offscreen framebuffer (prochmi's raw draw) to a
// `<prefix>_raw_<n>.ppm` before compositing, so we can see what prochmi
// actually drew (including its alpha channel behaviour) versus what the
// final composited window frame shows.
fn capture_raw_hmi_before_composite(backend: &mut Backend) {
    let Some(prefix) = hmi_capture_prefix() else {
        return;
    };
    let frame = HMI_RAW_CAPTURE_COUNT.fetch_add(1, Ordering::Relaxed);
    if !should_capture_hmi_frame(frame) {
        return;
    }
    unsafe {
        let mut previous = 0i32;
        gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut previous);
        gl::BindFramebuffer(gl::FRAMEBUFFER, backend.hmi_framebuffer);
        gl::ReadPixels(
            0,
            0,
            MAP_SURFACE_WIDTH as i32,
            MAP_SURFACE_HEIGHT as i32,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            backend.hmi_pixels.as_mut_ptr() as *mut _,
        );
        gl::BindFramebuffer(gl::FRAMEBUFFER, previous as u32);
    }
    let path = format!("{}_raw_{:04}.ppm", prefix, frame);
    let count = frame;
    if count < 5 {
        let mut alpha_hist = [0u32; 8];
        for rgba in backend.hmi_pixels.chunks_exact(4) {
            let a = rgba[3];
            let bucket = if a == 0 {
                0
            } else if a == 255 {
                7
            } else {
                1 + ((a as usize - 1) * 6) / 254
            };
            alpha_hist[bucket] += 1;
        }
        log::info!(
            "GPU: hmi_raw frame={} a0={} a1_63={} a64_127={} a128_191={} a192_254={} a255={}",
            count,
            alpha_hist[0],
            alpha_hist[1],
            alpha_hist[2],
            alpha_hist[3],
            alpha_hist[4] + alpha_hist[5],
            alpha_hist[6] + alpha_hist[7],
        );
    }
    let mut file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(_) => return,
    };
    if write!(
        file,
        "P6\n{} {}\n255\n",
        MAP_SURFACE_WIDTH, MAP_SURFACE_HEIGHT
    )
    .is_err()
    {
        return;
    }
    let row_bytes = (MAP_SURFACE_WIDTH as usize) * 4;
    for row in (0..MAP_SURFACE_HEIGHT as usize).rev() {
        for rgba in backend.hmi_pixels[row * row_bytes..][..row_bytes].chunks_exact(4) {
            if file.write_all(&rgba[..3]).is_err() {
                return;
            }
        }
    }
}

// Host GL object leak watchdog: counts guest-side Gen/Delete and logs the
// live totals plus process RSS periodically.
fn gpu_resource_telemetry() {
    static SWAPS: AtomicU32 = AtomicU32::new(0);
    let n = SWAPS.fetch_add(1, Ordering::Relaxed);
    if n % 120 != 0 {
        return;
    }
    let rss_kb = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .map(|pages| pages * 4)
        .unwrap_or(0);
    log::info!(
        "GPU: resource telemetry swap={} textures={} fbos={} rbos={} buffers={} shaders={} programs={} rss_kb={}",
        n,
        GL_LIVE_TEXTURES.load(Ordering::Relaxed),
        GL_LIVE_FRAMEBUFFERS.load(Ordering::Relaxed),
        GL_LIVE_RENDERBUFFERS.load(Ordering::Relaxed),
        GL_LIVE_BUFFERS.load(Ordering::Relaxed),
        GL_LIVE_SHADERS.load(Ordering::Relaxed),
        GL_LIVE_PROGRAMS.load(Ordering::Relaxed),
        rss_kb,
    );
}

fn probe_layer_alphas_enabled() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EMU_GPU_PROBE_ALPHAS").is_ok())
}

fn probe_layer_alphas(backend: &Backend) {
    if std::env::var("EMU_GPU_PROBE_ALPHAS").map(|v| v != "1").unwrap_or(true) {
        return;
    }
    static PROBE_COUNT: AtomicU32 = AtomicU32::new(0);
    let n = PROBE_COUNT.fetch_add(1, Ordering::Relaxed);
    if n > 600 || n % 60 != 0 {
        return;
    }
    let mut prev = 0i32;
    unsafe {
        gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut prev);
        for guest in [2u32, 3] {
            let host = resolve_guest_fbo(GpuTarget::Hmi, guest);
            gl::BindFramebuffer(gl::FRAMEBUFFER, host);
            for x in [20i32, 400] {
                let mut px = [0u8; 4];
                gl::ReadPixels(x, 240, 1, 1, gl::RGBA, gl::UNSIGNED_BYTE, px.as_mut_ptr() as *mut _);
                log::info!(
                    "GPU: alpha probe n={} hmi_fb={} host={} px({},{})={:02x?}",
                    n,
                    guest,
                    host,
                    x,
                    240,
                    px
                );
            }
        }
        gl::BindFramebuffer(gl::FRAMEBUFFER, backend.hmi_framebuffer);
        let mut px = [0u8; 4];
        gl::ReadPixels(20, 240, 1, 1, gl::RGBA, gl::UNSIGNED_BYTE, px.as_mut_ptr() as *mut _);
        log::info!("GPU: alpha probe n={} composed default fb px(20,240)={:02x?}", n, px);
        gl::BindFramebuffer(gl::FRAMEBUFFER, prev as u32);
    }
}

fn handle_surface_swap(backend: &mut Backend, target: GpuTarget) {
    // The swap bookkeeping below reads/renders via framebuffer objects the
    // guest never bound (host window FBO 0, the other target's pixels). Save
    // whatever the guest had bound and hand it back untouched so its own
    // GL_FRAMEBUFFER_BINDING state is unaffected by presenting a frame.
    let mut guest_fbo_raw = 0i32;
    unsafe {
        gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut guest_fbo_raw);
    }
    let guest_fbo = guest_fbo_raw as u32;
    match target {
        GpuTarget::Hmi => {
            unsafe {
                gl::Finish();
            }
            probe_layer_alphas(backend);
            gpu_resource_telemetry();
            if backend.hmi_framebuffer != 0 {
                if hmi_capture_prefix().is_some() {
                    capture_raw_hmi_before_composite(backend);
                }
                composite_hmi_over_map(backend, true);
                // Present on the private compositor context so guests never
                // see our program/texture/attrib state.
                unsafe {
                    sys::SDL_GL_MakeCurrent(backend.window_raw, backend.comp_context);
                }
                backend.current_target = None;
                blit_hmi_pixels_to_window(backend);
                // r728: emulator-side capture of the composed window (the
                // desktop screenshot tools on this host keep failing
                // silently), written as PPM next to the map dumps.
                dump_window_pixels(backend);
                let _ = backend.window.gl_swap_window();
                unsafe {
                    sys::SDL_GL_MakeCurrent(backend.window_raw, backend.hmi_context);
                }
                backend.current_target = Some(GpuTarget::Hmi);
            }
            capture_hmi_framebuffer(backend);
        }
        GpuTarget::Map => unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, backend.map_framebuffer);
            let mut fresh = vec![0u8; MAP_SURFACE_SIZE];
            gl::ReadPixels(
                0,
                0,
                MAP_SURFACE_WIDTH as i32,
                MAP_SURFACE_HEIGHT as i32,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                fresh.as_mut_ptr() as *mut _,
            );
            // r730: the engine re-runs the street multi-draw only when its
            // data updates; later repaints just clear the FBO and read back
            // as a single flat colour (r729/r730 window dumps: one road
            // frame published, then every composite saw flat/transparent
            // pixels and the window lost the roads).  The real unit's map
            // layer is fully redrawn every frame and its LCD always shows
            // the latest drawn map, so keep the most recent content-bearing
            // readback for compositing and let flat repaints pass through
            // as no-ops.
            let flat = {
                let mut reference = 0u32;
                let mut seen_reference = false;
                let mut flat = true;
                for chunk in fresh.chunks_exact(4).skip(701).step_by(997) {
                    let value =
                        u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if !seen_reference {
                        reference = value;
                        seen_reference = true;
                    } else if value != reference {
                        flat = false;
                        break;
                    }
                }
                flat
            };
            if !flat {
                backend.map_pixels.copy_from_slice(&fresh);
            }
            publish_map_surface_pixels(&backend.map_pixels);
            // r750: DPU-style rescan. The guest HMI compositor can freeze
            // (its AIL tick threads park at t~56 s while the map engine
            // keeps publishing frames); on the unit the LCD still shows the
            // live map because the display engine scans out both layers
            // independently of the HMI application redraws. Re-present the
            // composed window on every map publish - flat repaints included,
            // the scanout does not care whether the content changed - so the
            // window keeps tracking the published map even without a fresh
            // prochmi swap (and the throttled window dump below rides on the
            // same call, catching up whenever it becomes due).
            if backend.hmi_framebuffer != 0 {
                // r750: do NOT re-read the HMI FBO from here - this runs on
                // the map publishing thread with the MAP context current,
                // and `hmi_framebuffer` is an id from prochmi's context, so
                // binding it here resolves to the map's own FBO (id 1) and
                // the map layer would masquerade as the HMI layer.  Reuse
                // the `hmi_pixels` cached by the last real HMI swap: on the
                // unit a frozen HMI layer keeps showing its last painted
                // frame while the map layer underneath stays live.
                composite_hmi_over_map(backend, false);
                sys::SDL_GL_MakeCurrent(backend.window_raw, backend.comp_context);
                backend.current_target = None;
                blit_hmi_pixels_to_window(backend);
                dump_window_pixels(backend);
                let _ = backend.window.gl_swap_window();
                sys::SDL_GL_MakeCurrent(backend.window_raw, backend.map_context);
                backend.current_target = Some(GpuTarget::Map);
            }
        },
    }
    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER, guest_fbo);
    }
}

// Merge SVG layers into the HMI frame the way libsvg-layer's
// svgMergeAllLayersFB does on real hardware: procmap's map layer sits below,
// prochmi's HMI layer is blended on top using its own per-pixel alpha only.
// Fully transparent HMI pixels let the map through, partial alpha blends,
// opaque pixels (including intentionally black widgets) occlude the map.
fn composite_hmi_over_map(backend: &mut Backend, reload_hmi: bool) {
    // r750: the raw (pre-blend) HMI layer as of the last real HMI swap.  The
    // map-publish re-present cannot re-read the HMI FBO (foreign-context id)
    // and must not reuse the post-blend `hmi_pixels`, so the pristine layer
    // pixels are cached here on every swap that can read them.
    static HMI_RAW_CACHE: std::sync::LazyLock<Mutex<Vec<u8>>> =
        std::sync::LazyLock::new(|| Mutex::new(Vec::new()));
    if reload_hmi {
        unsafe {
            let mut previous = 0i32;
            gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut previous);
            gl::BindFramebuffer(gl::FRAMEBUFFER, backend.hmi_framebuffer);
            gl::ReadPixels(
                0,
                0,
                MAP_SURFACE_WIDTH as i32,
                MAP_SURFACE_HEIGHT as i32,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                backend.hmi_pixels.as_mut_ptr() as *mut _,
            );
            gl::BindFramebuffer(gl::FRAMEBUFFER, previous as u32);
        }
        if let Ok(mut cache) = HMI_RAW_CACHE.lock() {
            cache.clear();
            cache.extend_from_slice(&backend.hmi_pixels);
        }
    } else if let Ok(cache) = HMI_RAW_CACHE.lock() {
        if cache.len() == backend.hmi_pixels.len() && !cache.is_empty() {
            backend.hmi_pixels.copy_from_slice(&cache);
        }
    }
    if backend.map_pixels.len() != backend.hmi_pixels.len() {
        return;
    }
    // The display engine only shows the map layer where the HMI told it to:
    // `svgEnableLayer` for the map layer handle plus the window from
    // `LayerSync::setLayersVisible` (see `libs::prochmi`).  A window that was
    // never published means nothing was requested, so the HMI frame stands on its
    // own - exactly like a disabled layer on the unit.
    let enabled = MAP_LAYER_ENABLED.load(Ordering::Relaxed);
    let lx = (MAP_LAYER_X.load(Ordering::Relaxed) as usize).min(MAP_SURFACE_WIDTH);
    let ly = (MAP_LAYER_Y.load(Ordering::Relaxed) as usize).min(MAP_SURFACE_HEIGHT);
    let lw = match MAP_LAYER_W.load(Ordering::Relaxed) as usize {
        0 => MAP_SURFACE_WIDTH,
        w => w.min(MAP_SURFACE_WIDTH - lx),
    };
    let lh = match MAP_LAYER_H.load(Ordering::Relaxed) as usize {
        0 => MAP_SURFACE_HEIGHT,
        h => h.min(MAP_SURFACE_HEIGHT - ly),
    };
    // The readback runs bottom-up while a layer window is top-left origin.
    let row_first = MAP_SURFACE_HEIGHT.saturating_sub(ly + lh);
    let row_last = MAP_SURFACE_HEIGHT.saturating_sub(ly);
    let col_last = (lx + lw).min(MAP_SURFACE_WIDTH);
    let mut opaque_count = 0u32;
    let mut transparent_count = 0u32;
    let mut mixed_count = 0u32;
    let mut outside_count = 0u32;
    for (index, (hmi, map)) in backend
        .hmi_pixels
        .chunks_exact_mut(4)
        .zip(backend.map_pixels.chunks_exact(4))
        .enumerate()
    {
        let row = index / MAP_SURFACE_WIDTH;
        let col = index % MAP_SURFACE_WIDTH;
        if !enabled || row < row_first || row >= row_last || col < lx || col >= col_last {
            outside_count += 1;
            continue;
        }
        let a = hmi[3] as u16;
        if a == 255 {
            opaque_count += 1;
            continue;
        }
        if a == 0 {
            transparent_count += 1;
            hmi[0] = map[0];
            hmi[1] = map[1];
            hmi[2] = map[2];
            hmi[3] = 255;
            continue;
        }
        mixed_count += 1;
        let inv = 255 - a;
        for c in 0..3 {
            hmi[c] = ((a * hmi[c] as u16 + inv * map[c] as u16) / 255) as u8;
        }
        hmi[3] = 255;
    }
    if !enabled && transparent_count > 0 {
        // The HMI layer does have a hole (a transparent clear) but no layer is
        // enabled underneath it, so nothing can show through.  This is the state
        // to compare against the unit's map page.
        let hits = MAP_LAYER_HOLE_LOGS.fetch_add(1, Ordering::Relaxed);
        if hits < 5 {
            log::info!(
                "GPU: HMI layer has {} transparent pixels but no map layer is enabled (window {:#x})",
                transparent_count,
                MAP_LAYER_SIGNATURE.load(Ordering::Relaxed)
            );
        }
    }
    let count = HMI_COMPOSITE_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 20 {
        log::info!(
            "GPU: HMI composite frame={} enabled={} opaque={} transparent={} mixed={} outside={} map_first_px={}",
            count,
            enabled,
            opaque_count,
            transparent_count,
            mixed_count,
            outside_count,
            if backend.map_pixels.len() >= 4 {
                format!(
                    "({},{},{},{})",
                    backend.map_pixels[0],
                    backend.map_pixels[1],
                    backend.map_pixels[2],
                    backend.map_pixels[3]
                )
            } else {
                "<empty>".to_string()
            },
        );
    }
}

// Present the CPU-composited 800x480 RGBA `backend.hmi_pixels` to the SDL
// window. GLES 2 has no glDrawPixels / fixed-function pipeline, so we upload
// the pixels to a texture and draw a fullscreen quad with our helper shader.
static WINDOW_SIZE_SIGNATURE: AtomicU64 = AtomicU64::new(u64::MAX);
static WINDOW_SIZE_LOGS: AtomicU32 = AtomicU32::new(0);
static WINDOW_RESIZE_EVENT_LOGS: AtomicU32 = AtomicU32::new(0);
static WINDOW_BLITS: AtomicU32 = AtomicU32::new(0);

// Reproduce a user resize without anyone at the keyboard: `EMU_GPU_TEST_RESIZE`
// set to `WIDTHxHEIGHT` resizes the window once, shortly after the first frames,
// so the logical/drawable size behaviour can be read from the log.
fn test_window_resize(backend: &mut Backend) {
    static DONE: AtomicBool = AtomicBool::new(false);
    let blits = WINDOW_BLITS.fetch_add(1, Ordering::Relaxed);
    if DONE.load(Ordering::Relaxed) || blits < 60 {
        return;
    }
    let Ok(spec) = std::env::var("EMU_GPU_TEST_RESIZE") else {
        return;
    };
    let Some((w, h)) = spec.split_once('x') else {
        return;
    };
    let (Ok(w), Ok(h)) = (w.trim().parse::<i32>(), h.trim().parse::<i32>()) else {
        return;
    };
    DONE.store(true, Ordering::Relaxed);
    unsafe {
        sys::SDL_SetWindowSize(backend.window_raw, w, h);
    }
    log::info!("GPU: test resize requested to {}x{}", w, h);
}

fn blit_hmi_pixels_to_window(backend: &mut Backend) {
    if backend.composite_program == 0 {
        return;
    }
    let mut draw_w = 800i32;
    let mut draw_h = 480i32;
    let mut win_w = 800i32;
    let mut win_h = 480i32;
    unsafe {
        sys::SDL_GetWindowSize(backend.window_raw, &mut win_w, &mut win_h);
        sys::SDL_GL_GetDrawableSize(backend.window_raw, &mut draw_w, &mut draw_h);
    }
    // The window geometry the compositor gave us and the size of the GL drawable
    // behind it are two different numbers whenever SDL applies a HiDPI/scaling
    // factor, and the viewport has to follow the drawable.  Log both (and the
    // ratio) so a wrong-looking window scale can be attributed instead of guessed.
    let signature = win_w as u64
        | (win_h as u64) << 16
        | (draw_w as u64) << 32
        | ((draw_h as u64) << 48);
    if WINDOW_SIZE_SIGNATURE.swap(signature, Ordering::Relaxed) != signature {
        let hits = WINDOW_SIZE_LOGS.fetch_add(1, Ordering::Relaxed);
        if hits < 40 {
            log::info!(
                "GPU: window size logical={}x{} drawable={}x{} ratio={:.3}:{:.3} viewport={}x{}",
                win_w,
                win_h,
                draw_w,
                draw_h,
                if win_w == 0 { 0.0 } else { draw_w as f32 / win_w as f32 },
                if win_h == 0 { 0.0 } else { draw_h as f32 / win_h as f32 },
                draw_w.max(1),
                draw_h.max(1)
            );
        }
    }
    test_window_resize(backend);
    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
        gl::Viewport(0, 0, draw_w.max(1), draw_h.max(1));
        gl::Disable(gl::BLEND);
        gl::Disable(gl::DEPTH_TEST);
        gl::Disable(gl::CULL_FACE);
        gl::UseProgram(backend.composite_program);
        gl::ActiveTexture(gl::TEXTURE0);
        gl::BindTexture(gl::TEXTURE_2D, backend.composite_texture);
        gl::TexImage2D(
            gl::TEXTURE_2D,
            0,
            gl::RGBA as i32,
            MAP_SURFACE_WIDTH as i32,
            MAP_SURFACE_HEIGHT as i32,
            0,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            backend.hmi_pixels.as_ptr() as *const core::ffi::c_void,
        );
        let loc = gl::GetUniformLocation(backend.composite_program, b"u_tex\0".as_ptr());
        if loc >= 0 {
            gl::Uniform1i(loc, 0);
        }
        gl::BindBuffer(gl::ARRAY_BUFFER, backend.composite_vbo);
        gl::EnableVertexAttribArray(backend.composite_pos_loc as u32);
        gl::VertexAttribPointer(
            backend.composite_pos_loc as u32,
            2,
            gl::FLOAT,
            gl::FALSE,
            0,
            core::ptr::null(),
        );
        gl::DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
        gl::DisableVertexAttribArray(backend.composite_pos_loc as u32);
        gl::BindBuffer(gl::ARRAY_BUFFER, 0);
        gl::BindTexture(gl::TEXTURE_2D, 0);
        gl::UseProgram(0);
    }
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
                make_target_current(&mut backend, command.target);
                let output = (command.future)();
                if output.swap_requested {
                    handle_surface_swap(&mut backend, command.target);
                    if poll_events(&mut backend) {
                        return;
                    }
                }
                // r768: on-demand present (see request_forced_present).
                if FORCE_PRESENT_REQ.swap(false, Ordering::Relaxed) {
                    handle_surface_swap(&mut backend, GpuTarget::Hmi);
                }

                let _ = command.reply.send(output);

                if poll_events(&mut backend) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // r768: the guest idles without swapping once its startup
                // cascades end (see docs r768 repaint gap), so the timeout
                // tick must serve forced presents too - prochmi posts one
                // after the widget-hide channel re-renders the main view.
                if FORCE_PRESENT_REQ.swap(false, Ordering::Relaxed) {
                    make_target_current(&mut backend, GpuTarget::Hmi);
                    handle_surface_swap(&mut backend, GpuTarget::Hmi);
                }
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
            target: current_gpu_target(),
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
            target: current_gpu_target(),
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
    let elf_path = &unicorn.get_data().elf_path;
    set_gpu_target_for_elf(elf_path);
    gpu_process_allowed(elf_path) && ensure_gpu_thread()
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

fn gpu_trace_all() -> bool {
    std::env::var("EMU_GPU_TRACE_ALL")
        .map(|value| value != "0")
        .unwrap_or(false)
}

fn log_api(unicorn: &mut Unicorn<'_, Context>, prefix: &str, name: &str) {
    set_gpu_target_for_elf(&unicorn.get_data().elf_path);
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
    if important || count < 3000 || gpu_trace_all() {
        log::info!(
            "GPU: {} {} process={} thread [{}] r0={:x} r1={:x} r2={:x} r3={:x}",
            prefix,
            name,
            process_label(unicorn),
            unicorn.get_data().inner.thread_id(),
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
            // Block until the backend has actually presented this frame. The
            // private default FBO is a single buffer (no double buffering
            // behind it), so letting the guest run ahead would let its next
            // frame's glClear land before our ReadPixels captured this one -
            // a per-run nondeterministic tear. Blocking also paces swaps like
            // the vsynced window on real hardware.
            let swapped = if ensure_gpu_thread() {
                let sender = GPU_SENDER.get();
                let (reply_tx, reply_rx) = mpsc::channel();
                sender
                    .map(|sender| {
                        sender
                            .send(GpuCommand {
                                target: current_gpu_target(),
                                future: Box::new(|| GpuOutput {
                                    swap_requested: true,
                                    ..Default::default()
                                }),
                                reply: reply_tx,
                            })
                            .is_ok()
                            && reply_rx.recv().is_ok()
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
        "eglGetProcAddress" => {
            // Bosch GL wrappers (e.g. procmapengine's `glMapBufferOES`) resolve
            // OES extension entries through `eglGetProcAddress` and call the
            // returned pointer directly.  Hand out the address of our own
            // code stub registered for that name (libGLESv2 table), so the
            // indirect call traps back into `gl_api` like any import.
            // r710: the name is the single argument - in r0 (the `glMulti-
            // DrawArraysEXT` wrapper at 0x552edc proves it: `ldr r0,[0x553014];
            // bl eglGetProcAddress`), and its result is cached WITHOUT
            // null-checks (`str r0,[r10]; blx [r10]`), so a failed lookup must
            // answer 0 (the wrapper then takes its documented `glDrawArrays`
            // fallback loop) - answering 1 turns the cached value into a jump
            // target and kills the render thread.
            let mut name_ptr = ureg(unicorn, RegisterARM::R0);
            let mut name = read_guest_cstr(unicorn, name_ptr, 64);
            if name.is_empty() {
                name_ptr = ureg(unicorn, RegisterARM::R1);
                name = read_guest_cstr(unicorn, name_ptr, 64);
            }
            let address = unicorn
                .get_data()
                .code_stubs
                .lock()
                .ok()
                .and_then(|stubs| {
                    stubs
                        .iter()
                        .find(|(_, stub)| stub.name == name)
                        .map(|(address, _)| *address)
                })
                .unwrap_or(0);
            static PROC_LOGS: AtomicU32 = AtomicU32::new(0);
            if PROC_LOGS.fetch_add(1, Ordering::Relaxed) < 16 {
                log::info!("GPU: eglGetProcAddress({name:?}) -> {address:#x}");
            }
            return address;
        }
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
                if current_gpu_target() == GpuTarget::Map {
                    return MAP_SURFACE_HANDLE;
                }
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
    let target = current_gpu_target();
    match kind {
        GenKind::Buffer => GL_LIVE_BUFFERS.fetch_add(count as u64, Ordering::Relaxed),
        GenKind::Framebuffer => GL_LIVE_FRAMEBUFFERS.fetch_add(count as u64, Ordering::Relaxed),
        GenKind::Renderbuffer => GL_LIVE_RENDERBUFFERS.fetch_add(count as u64, Ordering::Relaxed),
        GenKind::Texture => GL_LIVE_TEXTURES.fetch_add(count as u64, Ordering::Relaxed),
    };
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
        // r698: host ids live in one namespace per context and the internal
        // scratch/ZERO gens share it with the engine, so log every id handed
        // to a guest to tell whose buffer 9 really is.
        if matches!(kind, GenKind::Buffer) {
            static GEN_LOGS: AtomicU32 = AtomicU32::new(0);
            if GEN_LOGS.fetch_add(1, Ordering::Relaxed) < 40 {
                log::info!("GPU: guest gen buffers target={:?} ids={:?}", target, ids);
            }
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
    match kind {
        DeleteKind::Buffer => GL_LIVE_BUFFERS.fetch_sub(count as u64, Ordering::Relaxed),
        DeleteKind::Framebuffer => GL_LIVE_FRAMEBUFFERS.fetch_sub(count as u64, Ordering::Relaxed),
        DeleteKind::Renderbuffer => {
            GL_LIVE_RENDERBUFFERS.fetch_sub(count as u64, Ordering::Relaxed)
        }
        DeleteKind::Texture => GL_LIVE_TEXTURES.fetch_sub(count as u64, Ordering::Relaxed),
    };
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
        // Never leave the attribute as a client array: with no VBO bound the
        // offset argument becomes a CPU address, and the engine's "clear this
        // attribute" calls pass 0x0/odd guest offsets that segfault the host
        // driver the moment a draw touches the attribute (r662: NVIDIA
        // dereferences NULL during the first road `glDrawArrays`).  Back the
        // attribute with a private zero-filled VBO instead.
        let _ = pointer;
        return gpu_void_clear(move || unsafe {
            static ZERO_VBO: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            let zero = *ZERO_VBO.get_or_init(|| {
                let mut buffer = 0u32;
                gl::GenBuffers(1, &mut buffer);
                if buffer == 0 {
                    buffer = next_fallback_id();
                }
                log::info!("GPU: internal ZERO_VBO id={buffer}");
                let zeros = [0u8; 4096];
                gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
                gl::BufferData(
                    gl::ARRAY_BUFFER,
                    zeros.len() as isize,
                    zeros.as_ptr() as *const _,
                    gl::STATIC_DRAW,
                );
                buffer
            });
            let mut previous = 0i32;
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);
            gl::BindBuffer(gl::ARRAY_BUFFER, zero);
            gl::VertexAttribPointer(index, size, kind, normalized, 0, std::ptr::null());
            gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
            drain_host_gl_errors();
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
        static SCRATCH_GEN_LOGS: AtomicU32 = AtomicU32::new(0);
        if SCRATCH_GEN_LOGS.fetch_add(1, Ordering::Relaxed) < 40 {
            log::info!(
                "GPU: internal client-leg scratch id={buffer} index={index}"
            );
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

fn route_delete_framebuffers(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let count = ureg(unicorn, RegisterARM::R0).min(4096) as i32;
    let ptr = ureg(unicorn, RegisterARM::R1);
    if count <= 0 || ptr == 0 {
        return 0;
    }
    let ids = read_u32_array(unicorn, ptr, count as usize);
    let gpu_target = current_gpu_target();
    gpu_void_clear(move || unsafe {
        let key_target = target_key(gpu_target);
        let private = private_default_fbo(gpu_target);
        let mut host_ids: Vec<u32> = Vec::with_capacity(ids.len());
        let mut dropped: Vec<(u8, u32)> = Vec::new();
        for guest_id in ids {
            if guest_id == 0 {
                continue;
            }
            let key = (key_target, guest_id);
            let host = if let Ok(aliases) = fbo_aliases().lock() {
                aliases.get(&key).copied()
            } else {
                None
            };
            match host {
                Some(host) => {
                    host_ids.push(host);
                    dropped.push(key);
                }
                None => {
                    if guest_id != private {
                        host_ids.push(guest_id);
                    }
                }
            }
        }
        if let Ok(mut aliases) = fbo_aliases().lock() {
            for key in dropped {
                aliases.remove(&key);
            }
        }
        if !host_ids.is_empty() {
            gl::DeleteFramebuffers(host_ids.len() as i32, host_ids.as_ptr());
        }
    });
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
    if current_gpu_target() == GpuTarget::Map {
        let seen = MATRIX_LOG_SEEN.with(|c| {
            let old = c.get();
            c.set(old.wrapping_add(1));
            old
        });
        if seen < 60 {
            let probe =
                read_f32_array(unicorn, values_ptr, components.min(16));
            let tail = read_f32_array(unicorn, values_ptr, components.max(4))
                .drain((components.saturating_sub(4)).min(components)..)
                .take(4)
                .collect::<Vec<f32>>();
            log::info!(
                "PROCMAPENGINE GPU trace matrix upload t={}ms loc={} n={} comps={} m0=[{:.1},{:.1},{:.1},{:.1}] translate=[{:.1},{:.1},{:.1},{:.1}]",
                crate::libs::dapi::emu_t0_elapsed_ms(),
                loc,
                count,
                components,
                probe.first().copied().unwrap_or(0.0),
                probe.get(1).copied().unwrap_or(0.0),
                probe.get(2).copied().unwrap_or(0.0),
                probe.get(3).copied().unwrap_or(0.0),
                tail.first().copied().unwrap_or(0.0),
                tail.get(1).copied().unwrap_or(0.0),
                tail.get(2).copied().unwrap_or(0.0),
                tail.get(3).copied().unwrap_or(1.0),
            );
        }
    }
    let values = read_f32_array(unicorn, values_ptr, (count.max(0) as usize).saturating_mul(components));
    if current_gpu_target() == GpuTarget::Map && loc >= 0 && components >= 16 && count >= 1 {
        // Keep the newest matrix per uniform location so the draw-time probe
        // can compute the clip coordinates the synth shader will produce
        // (r666: draws accepted with zero GL errors but the FBO readback stays
        // uniform background - suspects are out-of-range clip coords).
        if let Ok(mut matrices) = LAST_UPLOADED_MATRICES.lock() {
            let mut matrix = [0f32; 16];
            for (slot, value) in values.iter().take(16).enumerate() {
                matrix[slot] = *value;
            }
            matrices.insert(loc, matrix);
        }
    }
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

/// Diagnostic for the r666 road draws: reads the live host vertex attribute
/// state and the vertex bytes from the scratch VBO (client arrays were copied
/// there), applies the latest matrices uploaded to `u_matModelView` (loc 0)
/// and `u_matViewProj` (loc 1) exactly as the synth shader does, and logs the
/// resulting clip-space vertices.  Tells whether the draws land outside the
/// clip volume (rasteriser drops them silently, no GL error).
unsafe fn probe_map_draw_vertices(mode: u32, first: i32, count: i32) {
    static PROBES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    // Probe a whole frame's worth of draws: the first draws of a frame belong
    // to the cached-geometry element family (zeroed VBO legs) while the road
    // strips - latched by the freshly registered client legs - draw later in
    // the same frame (r685: cap 4 only ever saw the cached family).
    // r693: the street upload (bProcessData -> glBufferSubData into the road
    // VBO) lands minutes into the run, long after the general cap is spent;
    // keep probing big strip draws (count > 3000) separately so post-upload
    // frames are observed.
    static LARGE_PROBES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    // r697: the backend clock is not the emulator clock, so gate the big
    // strip probes on the engine-side fact instead of time: the street-coord
    // probe in `procmapengine.rs` sets `ROAD_GEOMETRY_UPLOADED` the first
    // time `vUpdateVBO` passes its gates, i.e. exactly when road VBOs start
    // carrying real data.  `EMU_PROBE_AFTER_MS=0` forces probes regardless.
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed_ms = START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
    let force = std::env::var("EMU_PROBE_AFTER_MS")
        .ok()
        .filter(|value| value == "0")
        .is_some();
    let (budget, spent) = if count > 3000 {
        if !force && !road_geometry_uploaded() {
            return;
        }
        (100, LARGE_PROBES.fetch_add(1, Ordering::Relaxed))
    } else if road_geometry_uploaded() {
        // r750: with the trap-world engine the road pass arrives as batches
        // of small strips (multi-draw count ~8), so the general 40-probe
        // budget - all spent before t<2 s - never observes them. Once the
        // street upload has happened, spend a second budget of 80 probes on
        // small strips too; that is exactly the t>60 s window we need to see
        // the clip positions of.
        static AFTER_UPLOAD_SMALL: AtomicU32 = AtomicU32::new(0);
        (80, AFTER_UPLOAD_SMALL.fetch_add(1, Ordering::Relaxed))
    } else {
        (40, PROBES.fetch_add(1, Ordering::Relaxed))
    };
    if spent >= budget {
        return;
    }
    let mut viewport = [0i32; 4];
    gl::GetIntegerv(gl::VIEWPORT, viewport.as_mut_ptr());
    let mut program = 0i32;
    gl::GetIntegerv(gl::CURRENT_PROGRAM, &mut program);
    let mut states = [(0i32, 0i32, 0i32, 0i32, 0i32); 4];
    let mut attrib_pointers = [0i32; 4];
    for (index, state) in states.iter_mut().enumerate() {
        let index = index as u32;
        gl::GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_ENABLED, &mut state.0);
        gl::GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_SIZE, &mut state.1);
        gl::GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_TYPE, &mut state.2);
        gl::GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_STRIDE, &mut state.3);
        gl::GetVertexAttribiv(index, gl::VERTEX_ATTRIB_ARRAY_BUFFER_BINDING, &mut state.4);
        // VBO offset (bytes) the attribute was specified with; offset-mode
        // strips do not start at buffer byte 0.
        let mut pointer: *mut std::ffi::c_void = std::ptr::null_mut();
        // r718: the pname must be VERTEX_ATTRIB_ARRAY_POINTER (0x886F);
        // ARRAY_BUFFER (0x8892) is not valid here, the query silently
        // failed with INVALID_ENUM and the probe printed ap=0 for every
        // attribute for many runs - the real latches were fine.
        gl::GetVertexAttribPointerv(
            index,
            gl::VERTEX_ATTRIB_ARRAY_POINTER,
            std::ptr::addr_of_mut!(pointer) as *const *mut std::ffi::c_void,
        );
        attrib_pointers[index as usize] = pointer as i32;
    }
    // Uniform inventory of the bound program: tells whether the synth shader
    // actually linked into this program (r669: name lookup found no matrix
    // uniforms in the program the road draws bind).
    let mut uniform_count = 0i32;
    let mut uniform_names: Vec<String> = Vec::new();
    if program != 0 {
        gl::GetProgramiv(program as u32, gl::ACTIVE_UNIFORMS, &mut uniform_count);
        for index in 0..uniform_count.max(0).min(8) {
            let mut name = [0u8; 64];
            let mut length = 0i32;
            let mut size = 0i32;
            let mut kind = 0u32;
            gl::GetActiveUniform(
                program as u32,
                index as u32,
                name.len() as i32,
                &mut length,
                &mut size,
                &mut kind,
                name.as_mut_ptr() as *mut std::ffi::c_char,
            );
            uniform_names.push(
                String::from_utf8_lossy(&name[..length.max(0).min(63) as usize]).into_owned(),
            );
        }
    }
    // Bytes at the head of whatever ARRAY_BUFFER the engine last bound: if the
    // engine keeps road geometry in its own VBO, the vertex data lives there.
    let mut engine_vbo_head: Vec<i16> = Vec::new();
    let mut array_binding = 0i32;
    gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut array_binding);
    if array_binding > 0 {
        // ARRAY_BUFFER_BINDING is already `array_binding`, so no re-bind is
        // needed for the read (r700: via map, `GetBufferSubData` is dead on
        // this host).
        let mut raw = vec![0u8; 32];
        unsafe {
            let data = gl::MapBufferRange(
                gl::ARRAY_BUFFER,
                0,
                raw.len() as isize,
                gl::MAP_READ_BIT,
            );
            if !data.is_null() {
                std::ptr::copy_nonoverlapping(data as *const u8, raw.as_mut_ptr(), raw.len());
                gl::UnmapBuffer(gl::ARRAY_BUFFER);
            }
        }
        engine_vbo_head = raw
            .chunks_exact(2)
            .map(|w| i16::from_le_bytes([w[0], w[1]]))
            .collect();
    }
    // The synth program is small; resolve its two matrix uniforms by name so
    // the probe applies them in the shader's order regardless of the
    // locations the driver assigned.
    let mut matrices = (None, None);
    if program != 0 {
        let mut uniform_count = 0i32;
        gl::GetProgramiv(program as u32, gl::ACTIVE_UNIFORMS, &mut uniform_count);
        for index in 0..uniform_count.max(0).min(16) {
            let mut name = [0u8; 64];
            let mut length = 0i32;
            let mut size = 0i32;
            let mut kind = 0u32;
            gl::GetActiveUniform(
                program as u32,
                index as u32,
                name.len() as i32,
                &mut length,
                &mut size,
                &mut kind,
                name.as_mut_ptr() as *mut std::ffi::c_char,
            );
            let name = String::from_utf8_lossy(
                &name[..length.max(0).min(63) as usize],
            )
            .into_owned();
            let c_name = std::ffi::CString::new(name.clone()).unwrap_or_default();
            let loc = gl::GetUniformLocation(program as u32, c_name.as_ptr());
            if loc < 0 {
                continue;
            }
            let stored = LAST_UPLOADED_MATRICES
                .lock()
                .ok()
                .and_then(|matrices| matrices.get(&(loc as i32)).copied());
            if name == "u_matModelView" {
                matrices.0 = stored;
            } else if name == "u_m4ProjMatStreets" {
                matrices.1 = stored;
            }
        }
    }
    let mut samples: Vec<[f32; 4]> = Vec::new();
    // r714: raw sampled operand values (xy leg, dir leg) for ground-truth
    // arithmetic on where the strip actually sits relative to the view.
    let mut raw_verts: Vec<String> = Vec::new();
    // How many spread samples end up inside the normalized clip cube: the
    // discriminator between "matrix wrong" (0 always) and "this strip just
    // happens to be off-screen" (occasionally >0).
    let mut visible_vertices = 0u32;
    // The road program transform is `u_m4ProjMatStreets * (u_matModelView *
    // pos)`.  If the engine never uploaded a streets projection (its loop has
    // no case for a dead `u_matViewProj` and case 6 may stay gated), fall back
    // to the same viewport ortho the synthesized shader would apply, so the
    // probe clips model the pixels that would actually be rasterized.
    if matrices.1.is_none() {
        let mut fallback = [0f32; 16];
        fallback[0] = 2.0 / viewport[2].max(1) as f32;
        fallback[5] = 2.0 / viewport[3].max(1) as f32;
        fallback[10] = 1.0;
        fallback[15] = 1.0;
        matrices.1 = Some(fallback);
    }
    let binding = states[0].4.max(0) as u32;
    if binding != 0 {
        let components = (states[0].1.max(1) as usize).min(4);
        let stride = if states[0].3 > 0 {
            states[0].3 as usize
        } else {
            components * 2
        };
        let want = 4usize.min(count.max(0) as usize);
        // Sample spread across the whole strip, not just the first quad:
        // road strips routinely start off-screen (the first vertex may be a
        // distant strip corner), and consecutive vertices of a strip share
        // the same centerline point (left/right extrusion pair).
        let spread = (count.max(1) as usize - 1).max(1);
        let vertices: Vec<usize> = (0..want)
            .map(|k| first.max(0) as usize + k * spread / (want.max(2) - 1).max(1))
            .collect();
        let mut raw = vec![0u8; ((first.max(0) as usize + spread + 1) * stride + 8).min(1 << 20)];
        let data_offset = attrib_pointers[0].max(0) as usize;
        let mut previous = 0i32;
        gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);
        gl::BindBuffer(gl::ARRAY_BUFFER, binding);
        // r700: `GetBufferSubData` never returns bytes on this host (every
        // readback in every run was zeros, even right after an upload the
        // log proved was accepted), so read the window through a read-only
        // map instead.
        {
            let data =
                gl::MapBufferRange(gl::ARRAY_BUFFER, data_offset as isize, raw.len() as isize, gl::MAP_READ_BIT);
            if !data.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(data as *const u8, raw.as_mut_ptr(), raw.len());
                    gl::UnmapBuffer(gl::ARRAY_BUFFER);
                }
            }
        }
        gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
        // r714: the synth shader consumes `extruded = a_VertexXY +
        // a_v2DirVecs`, so the clip model must read the direction leg too -
        // sampling only the extrusion vectors gave misleading `vis` (r713:
        // all samples y=0 while the pixels sat in the top-right corner).
        // r715: `a_v2DirVecs` is location 1 (engine manifest order xy, dir,
        // z, color - r715 detail VAPs show the direction leg at index 1).
        let mut dir_values: Vec<[f32; 2]> = Vec::new();
        let dir_binding = states.get(1).map(|state| state.4.max(0) as u32).unwrap_or(0);
        if dir_binding != 0 {
            let dir_stride = states
                .get(1)
                .map(|state| if state.3 > 0 { state.3 as usize } else { 4 })
                .unwrap_or(8);
            let dir_offset = attrib_pointers.get(1).copied().unwrap_or(0).max(0) as usize;
            let mut draw = vec![0u8; ((first.max(0) as usize + spread + 1) * dir_stride + 8).min(1 << 20)];
            gl::BindBuffer(gl::ARRAY_BUFFER, dir_binding);
            {
                let data = gl::MapBufferRange(
                    gl::ARRAY_BUFFER,
                    dir_offset as isize,
                    draw.len() as isize,
                    gl::MAP_READ_BIT,
                );
                if !data.is_null() {
                    std::ptr::copy_nonoverlapping(data as *const u8, draw.as_mut_ptr(), draw.len());
                    gl::UnmapBuffer(gl::ARRAY_BUFFER);
                }
            }
            gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
            let dir_float = states.get(1).map(|state| state.2 as u32 == gl::FLOAT).unwrap_or(true);
            dir_values = vertices
                .iter()
                .map(|&vertex| {
                    let base = vertex * dir_stride;
                    let dword = |offset: usize| -> f32 {
                        if dir_float {
                            let b: [u8; 4] = [
                                *draw.get(base + offset).unwrap_or(&0),
                                *draw.get(base + offset + 1).unwrap_or(&0),
                                *draw.get(base + offset + 2).unwrap_or(&0),
                                *draw.get(base + offset + 3).unwrap_or(&0),
                            ];
                            f32::from_le_bytes(b)
                        } else {
                            let lo = *draw.get(base + offset).unwrap_or(&0) as u16;
                            let hi = *draw.get(base + offset + 1).unwrap_or(&0) as u16;
                            (lo | (hi << 8)) as i16 as f32
                        }
                    };
                    [dword(0), dword(if dir_float { 4 } else { 2 })]
                })
                .collect();
        }
        // r725: running sum of the model-space sample centres, used for the
        // `u_v2Pan` view centring (see `SYNTH_VERT_SHADER`).
        let mut centre = [0f32; 2];
        let mut centre_n = 0usize;
        for (sample_index, &vertex) in vertices.iter().enumerate() {
            let base = vertex * stride;
            // 0x1406 is GL_FLOAT (GL_SHORT is 0x1402) - the engine feeds
            // packed float2 strips, so decode by the reported attribute type.
            let is_float = states[0].2 as u32 == gl::FLOAT;
            let word = |offset: usize| -> f32 {
                if is_float {
                    let b: [u8; 4] = [
                        *raw.get(base + offset).unwrap_or(&0),
                        *raw.get(base + offset + 1).unwrap_or(&0),
                        *raw.get(base + offset + 2).unwrap_or(&0),
                        *raw.get(base + offset + 3).unwrap_or(&0),
                    ];
                    f32::from_le_bytes(b)
                } else {
                    let lo = *raw.get(base + offset).unwrap_or(&0) as u16;
                    let hi = *raw.get(base + offset + 1).unwrap_or(&0) as u16;
                    (lo | (hi << 8)) as i16 as f32
                }
            };
            let mut pos = [0f32; 4];
            pos[0] = word(0);
            if components > 1 {
                pos[1] = word(if is_float { 4 } else { 2 });
            }
            if components > 2 {
                pos[2] = word(if is_float { 8 } else { 4 });
            }
            pos[3] = 1.0;
            match dir_values.get(sample_index) {
                Some(dir) => {
                    raw_verts.push(format!(
                        "({:.1},{:.1})+({:.1},{:.1})",
                        pos[0], pos[1], dir[0], dir[1]
                    ));
                    pos[0] += dir[0];
                    pos[1] += dir[1];
                }
                None => raw_verts.push(format!("({:.1},{:.1})+(-)", pos[0], pos[1])),
            }
            centre[0] += pos[0];
            centre[1] += pos[1];
            centre_n += 1;
            let apply = |input: [f32; 4], matrix: &[f32; 16]| -> [f32; 4] {
                let mut out = [0f32; 4];
                for (row, value) in out.iter_mut().enumerate() {
                    *value = (0..4).map(|column| matrix[column * 4 + row] * input[column]).sum();
                }
                out
            };
            let mut clip = pos;
            if let Some(model_view) = matrices.0 {
                clip = apply(clip, &model_view);
                // Mirror the synth shader's r701 modelview z-fold so `vis`
                // predicts the pixels that would actually rasterize.
                clip[2] = -clip[2].abs();
            }
            if let Some(view_proj) = matrices.1 {
                clip = apply(clip, &view_proj);
                // r711: the synth shader pins `gl_Position.z = 0.5 * w`
                // (no terrain to depth-sort), so depth never clips - mirror
                // it here or `vis` under-predicts the rasterized pixels.
                clip[2] = 0.5 * clip[3];
            }
            if clip[0].abs() <= clip[3].abs()
                && clip[1].abs() <= clip[3].abs()
                && clip[2].abs() <= clip[3].abs()
            {
                visible_vertices += 1;
            }
            samples.push(clip);
        }
        // r725: publish the view-space pan that centres this draw's road
        // block (modelview linear part applied to the model-space sample
        // centre; translation excluded - the synth shader subtracts it
        // after the modelview multiply).
        if centre_n > 0 {
            if let Some(model_view) = matrices.0 {
                let cx = centre[0] / centre_n as f32;
                let cy = centre[1] / centre_n as f32;
                if let Ok(mut pan) = LAST_MAP_PAN.lock() {
                    *pan = Some((
                        model_view[0] * cx + model_view[4] * cy,
                        model_view[1] * cx + model_view[5] * cy,
                    ));
                }
            }
        }
    }
    let matrix_catalog: Vec<String> = LAST_UPLOADED_MATRICES
        .lock()
        .map(|matrices| {
            matrices
                .iter()
                .map(|(loc, matrix)| {
                    format!(
                        "loc{}:[{:.3},{:.3},{:.3},{:.3}|t{:.0},{:.0},{:.0}]",
                        loc,
                        matrix[0],
                        matrix[5],
                        matrix[10],
                        matrix[15],
                        matrix[12],
                        matrix[13],
                        matrix[14]
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let attrib_dump = states
        .iter()
        .enumerate()
        .map(|(index, state)| {
            format!(
                "{}:(en={} sz={} ty={:#x} st={} vbo={} ap={:#x})",
                index, state.0, state.1, state.2, state.3, state.4, attrib_pointers[index]
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    log::info!(
        "GPU: map draw probe t={}ms mode={} first={} count={} viewport={:?} program={} uniforms={} names={:?} array_vbo={} engine_head={:?} attribs=[{}] mv={} vp={} vis={}/{} mats=[{}] clips=[{}] verts=[{}]",
        elapsed_ms,
        mode,
        first,
        count,
        viewport,
        program,
        uniform_count,
        uniform_names,
        array_binding,
        engine_vbo_head,
        attrib_dump,
        matrices.0.is_some(),
        matrices.1.is_some(),
        visible_vertices,
        samples.len(),
        matrix_catalog.join(" "),
        samples
            .iter()
            .map(|clip| {
                let (sx, sy) = if clip[3].abs() > 1e-6 {
                    (clip[0] / clip[3], clip[1] / clip[3])
                } else {
                    (f32::NAN, f32::NAN)
                };
                format!(
                    "({:.1},{:.1},{:.1},{:.1}|{sx:.2},{sy:.2})",
                    clip[0], clip[1], clip[2], clip[3]
                )
            })
            .collect::<Vec<_>>()
            .join(" "),
        raw_verts.join(" "),
    );
}

fn route_active_uniform(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    active_query(unicorn, false)
}

/// Per-draw uniform refresh for the synthesized road program.  The engine
/// runs `vUpdateShaderUniforms` only once per program switch, so its one-shot
/// uploads froze a stale matrix: r679 captured the 3D-perspective
/// `m_aflStreetProjMatrix` the view code left behind before the map view ever
/// sized itself.  Read the engine-computed values back from their globals
/// (Ghidra VAs equal the runtime VAs for procmapengine) right before the
/// draw and hand them to the program only when it declares the uniform.
fn refresh_map_engine_uniforms(unicorn: &mut Unicorn<'_, Context>) {
    const STREET_PROJ: u32 = 0x71_acd0; // rl_tclRenderLayer3D::m_aflStreetProjMatrix
    const LINE_WIDTH: u32 = 0x71_b448;  // rl_tclRenderLayer3D::ms_flLineWidthPerPixel
    let mut raw = [0u8; 64];
    if unicorn.mem_read(STREET_PROJ as u64, &mut raw).is_err() {
        return;
    }
    let mut matrix = [0f32; 16];
    for (slot, bytes) in matrix.iter_mut().zip(raw.chunks_exact(4)) {
        *slot = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
    if matrix.iter().all(|value| *value == 0.0) {
        // `vCalculateMapScale` memsets before filling: all-zero means the
        // projection has not been computed yet, uploading it would only
        // collapse the strip to the origin.
        return;
    }
    let width = f32::from_bits(read_u32(unicorn, LINE_WIDTH));
    // `vUpdateShaderUniforms` case 7 feeds `u_flLineWidth` the constant
    // 0.7f (0x3f333333, TS-unit half-width).  The engine only runs that loop
    // once per program switch (before this program existed), and the
    // `ms_flLineWidthPerPixel` global was still 0 in every observed run, so
    // replay the constant when neither source has a value.
    let width = if width == 0.0 { 0.7 } else { width };
    let _ = gpu_void_clear(move || unsafe {
        let mut program = 0i32;
        gl::GetIntegerv(gl::CURRENT_PROGRAM, &mut program);
        if program <= 0 {
            return;
        }
        let proj_name = std::ffi::CString::new("u_m4ProjMatStreets").unwrap();
        let loc = gl::GetUniformLocation(program as u32, proj_name.as_ptr());
        if loc >= 0 {
            gl::UniformMatrix4fv(loc, 1, gl::FALSE, matrix.as_ptr());
        }
        let width_name = std::ffi::CString::new("u_flLineWidth").unwrap();
        let loc = gl::GetUniformLocation(program as u32, width_name.as_ptr());
        if loc >= 0 {
            gl::Uniform1f(loc, width);
        }
        static UNIFORM_REFRESH_LOGS: AtomicU32 = AtomicU32::new(0);
        if UNIFORM_REFRESH_LOGS.fetch_add(1, Ordering::Relaxed) < 4 {
            log::info!(
                "GPU: map engine uniform refresh proj[0]={:.5} proj[5]={:.5} t[{:.0},{:.0},{:.0}] width={:.5}",
                matrix[0],
                matrix[5],
                matrix[12],
                matrix[13],
                matrix[14],
                width
            );
        }
        drain_host_gl_errors();
    });
}

/// Re-read every registered map-target client array right before an engine
/// draw and re-upload the backing VBO when its guest content changed.
/// GL client-array semantics read the array at DRAW time; the engine fills
/// its vertex staging after `glVertexAttribPointer`, so the snapshot taken
/// in the VAP wrapper is stale (r667: every road strip arrived as zeros).
fn refresh_map_client_arrays(unicorn: &mut Unicorn<'_, Context>) {
    let regs: Vec<(u32, u32, usize, u64, u32)> = match VAP_ATTRIB_REGS.lock() {
        Ok(regs) => regs
            .iter()
            .map(|(index, (pointer, len, hash, vbo))| (*index, *pointer, *len, *hash, *vbo))
            .collect(),
        Err(_) => return,
    };
    let mut scratch = match VAP_SCRATCH.lock() {
        Ok(scratch) => scratch,
        Err(_) => return,
    };
    for (index, pointer, len, old_hash, vbo) in regs {
        if len == 0 {
            continue;
        }
        if scratch.len() < len {
            scratch.resize(len, 0);
        }
        if unicorn.mem_read(pointer as u64, &mut scratch[..len]).is_err() {
            static REFRESH_FAIL_LOGS: AtomicU32 = AtomicU32::new(0);
            let misses = REFRESH_FAIL_LOGS.fetch_add(1, Ordering::Relaxed);
            if misses < 6 {
                log::info!(
                    "GPU: map client array refresh SKIP idx={} ptr={:#x} len={} (read failed)",
                    index,
                    pointer,
                    len
                );
            }
            continue;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&scratch[..len], &mut hasher);
        let hash = std::hash::Hasher::finish(&hasher);
        if hash == old_hash {
            continue;
        }
        let data = scratch[..len].to_vec();
        let head: Vec<i16> = data
            .chunks_exact(2)
            .take(6)
            .map(|w| i16::from_le_bytes([w[0], w[1]]))
            .collect();
        let uploaded = gpu_void_clear(move || unsafe {
            let mut previous = 0i32;
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                data.len() as isize,
                data.as_ptr() as *const _,
                gl::DYNAMIC_DRAW,
            );
            gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
            drain_host_gl_errors();
        });
        if uploaded {
            static REFRESH_LOGS: AtomicU32 = AtomicU32::new(0);
            let hits = REFRESH_LOGS.fetch_add(1, Ordering::Relaxed);
            if hits < 6 {
                log::info!(
                    "GPU: map client array refresh idx={} ptr={:#x} len={} vbo={} head={:?}",
                    index,
                    pointer,
                    len,
                    vbo,
                    head
                );
            }
            if let Ok(mut regs) = VAP_ATTRIB_REGS.lock() {
                if let Some(reg) = regs.get_mut(&index) {
                    reg.2 = hash;
                }
            }
            if let Ok(mut cache) = VAP_CACHE.lock() {
                if let Some(entry) = cache.get_mut(&pointer) {
                    *entry = (vbo, hash);
                }
            }
        }
    }
}

fn route_active_attrib(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    active_query(unicorn, true)
}

/// `glGetActiveUniform`/`glGetActiveAttrib` (ARM AAPCS: r0=program,
/// r1=index, r2=bufSize, r3=length*, stack: size*, type*, name*).  The map
/// engine's `vFindUniformLocations` discovers every uniform by enumerating
/// actives and strcmp-ing against its own name table, so without a real
/// answer the synthesized road shader's matrices resolve to no location and
/// `vUpdateShaderUniforms` never uploads them.
fn active_query(unicorn: &mut Unicorn<'_, Context>, attrib: bool) -> u32 {
    let program = ureg(unicorn, RegisterARM::R0);
    let index = ureg(unicorn, RegisterARM::R1);
    let buf_size = ureg(unicorn, RegisterARM::R2) as usize;
    let length_ptr = ureg(unicorn, RegisterARM::R3);
    let size_ptr = stack_arg(unicorn, 0);
    let type_ptr = stack_arg(unicorn, 1);
    let name_ptr = stack_arg(unicorn, 2);
    let output = gpu_call(move || {
        let mut name = [0u8; 256];
        let mut length = 0i32;
        let mut size = 0i32;
        let mut kind = 0u32;
        unsafe {
            if attrib {
                gl::GetActiveAttrib(
                    program,
                    index,
                    256,
                    &mut length,
                    &mut size,
                    &mut kind,
                    name.as_mut_ptr() as *mut std::ffi::c_char,
                );
            } else {
                gl::GetActiveUniform(
                    program,
                    index,
                    256,
                    &mut length,
                    &mut size,
                    &mut kind,
                    name.as_mut_ptr() as *mut std::ffi::c_char,
                );
            }
            drain_host_gl_errors();
        }
        let bytes: Vec<u8> = name.iter().copied().take_while(|c| *c != 0).collect();
        GpuOutput {
            ret: kind,
            out_u32: vec![length.max(0) as u32, size.max(0) as u32],
            out_bytes: bytes,
            ..Default::default()
        }
    });
    let Some(output) = output else {
        return 0;
    };
    if length_ptr != 0 {
        write_u32(unicorn, length_ptr, output.out_u32[0]);
    }
    if size_ptr != 0 {
        write_u32(unicorn, size_ptr, output.out_u32[1]);
    }
    if type_ptr != 0 {
        write_u32(unicorn, type_ptr, output.ret);
    }
    if name_ptr != 0 && buf_size > 0 {
        let mut bytes = output.out_bytes.clone();
        if bytes.len() + 1 > buf_size {
            bytes.truncate(buf_size.saturating_sub(1));
        }
        bytes.push(0);
        let _ = unicorn.mem_write(name_ptr as u64, &bytes);
    }
    0
}

fn route_shader_source(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let shader = ureg(unicorn, RegisterARM::R0);
    // Real source supersedes any `glShaderBinary` substitution waiting for
    // this shader (the HMI probes binary support then compiles source).
    if let Ok(mut m) = GL_SYNTH_SHADER_SOURCE.lock() {
        m.remove(&shader);
    }
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

/// The engine cannot resolve road attribute locations from the
/// `[BinaryShader]` manifest (nothing declares them in text), so it calls
/// `glVertexAttribPointer(-1, ...)` with live client pointers for
/// `a_VertexXY` and `a_v2DirVecs` (r683: legs `(-1, vec2, 8, 0xf60800)` and
/// `(-1, vec2, 8, 0xf60a08)` on every street element; `a_color` resolves to
/// index 1 fine).  Redirect each unseen pointer to the synthesized program's
/// real attribute location in first-seen order, so the latched state matches
/// what the real driver derives from the binary shader's symbol table.
/// Resolve an attribute of the synthesized map program by name and make
/// sure it is enabled (the engine's enable pass only uses ordinal indices
/// r683, so a redirected attribute above ordinal 1 would stay disabled and
/// fetch the (0,0,0,1) constant).  Runs on the backend so the current
/// program is the map target's.
fn synth_attrib_loc(name: &str) -> Option<u32> {
    let name = name.to_string();
    let loc = gpu_u32_clear(move || unsafe {
        let mut program = 0i32;
        gl::GetIntegerv(gl::CURRENT_PROGRAM, &mut program);
        if program <= 0 {
            return u32::MAX;
        }
        let c_name = std::ffi::CString::new(name).unwrap();
        let loc = gl::GetAttribLocation(program as u32, c_name.as_ptr());
        if loc >= 0 {
            gl::EnableVertexAttribArray(loc as u32);
        }
        drain_host_gl_errors();
        loc.max(0) as u32
    })?;
    (loc != u32::MAX).then_some(loc)
}

fn redirect_negative_attrib(pointer: u32, magnitude: f32) -> Option<u32> {
    static NEG_REGS: std::sync::LazyLock<Mutex<std::collections::HashMap<u32, u32>>> =
        std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(loc) = NEG_REGS.lock().ok().and_then(|m| m.get(&pointer).copied()) {
        return Some(loc);
    }
    static NEXT_INDEX: AtomicU32 = AtomicU32::new(0);
    // The engine emits the position leg before the direction leg in every
    // element window (verified across r669-r683), so first-seen order is the
    // stable discriminator; both stagings carry unit-ish float magnitudes, so
    // content comparison would be ambiguous.
    const NAMES: [&str; 2] = ["a_VertexXY", "a_v2DirVecs"];
    let order = (NEXT_INDEX.fetch_add(1, Ordering::Relaxed) as usize).min(NAMES.len() - 1);
    let c_and_log_name = NAMES[order].to_string();
    let loc = synth_attrib_loc(&c_and_log_name)?;
    if let Ok(mut m) = NEG_REGS.lock() {
        m.insert(pointer, loc);
    }
    static NEG_LOGS: AtomicU32 = AtomicU32::new(0);
    if NEG_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
        log::info!(
            "GPU: map negative-index VAP redirected ptr={pointer:#x} mag={magnitude:.2} -> attrib {loc} ({c_and_log_name})"
        );
    }
    Some(loc)
}

fn route_vertex_attrib_pointer(unicorn: &mut Unicorn<'_, Context>) -> u32 {
    let index = ureg(unicorn, RegisterARM::R0);
    let index = if current_gpu_target() == GpuTarget::Map
        && index == u32::MAX
        && stack_arg(unicorn, 1) >= 0x10000
    {
        let pointer = stack_arg(unicorn, 1);
        let mut head = [0u8; 8];
        let magnitude = match unicorn.mem_read(pointer as u64, &mut head) {
            Ok(()) => {
                let x = f32::from_le_bytes([head[0], head[1], head[2], head[3]]);
                let y = f32::from_le_bytes([head[4], head[5], head[6], head[7]]);
                x.abs().max(y.abs())
            }
            Err(_) => 0.0,
        };
        redirect_negative_attrib(pointer, magnitude).unwrap_or(index)
    } else if current_gpu_target() == GpuTarget::Map
        && index == 0
        && (ureg(unicorn, RegisterARM::R1) as i32) == 2
        && (stack_arg(unicorn, 0) as i32) == 8
        && stack_arg(unicorn, 1) > 0
        && stack_arg(unicorn, 1) < 0x10000
    {
        // r711g: the combined-streets pass latches BOTH road legs under its
        // manifest id 0 - `a_VertexXY` at offset 0 and `a_v2DirVecs` at
        // `coordcount << 3` over the bound resource VBO (r711f: `idx=0
        // stride=8 offset=0x5ed0 bound_vbo=9/12`), so the second write
        // clobbers the first.  A stride-8 vec2 offset leg with a live VBO
        // binding is unambiguously the direction leg: retarget it to the
        // synthesized program's `a_v2DirVecs` location.
        let bound = gpu_u32_clear(|| unsafe {
            let mut binding = 0i32;
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut binding);
            drain_host_gl_errors();
            binding.max(0) as u32
        });
        match bound.filter(|binding| *binding != 0).and_then(|_| synth_attrib_loc("a_v2DirVecs"))
        {
            Some(loc) => {
                static DIR_LOGS: AtomicU32 = AtomicU32::new(0);
                if DIR_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                    log::info!(
                        "GPU: map dir-leg idx0 stride=8 offset={:#x} bound_vbo={} -> attrib {loc} (a_v2DirVecs)",
                        stack_arg(unicorn, 1),
                        bound.unwrap_or(0)
                    );
                }
                loc
            }
            None => index,
        }
    } else if current_gpu_target() == GpuTarget::Map
        && index == 1
        && (ureg(unicorn, RegisterARM::R1) as i32) == 2
        && (stack_arg(unicorn, 0) as i32) == 8
        && stack_arg(unicorn, 1) == 0
    {
        // r718: the combined pass' manifest slots read the OTHER way round
        // than the client-array family: its position leg arrives as `idx=1
        // offset=0` and its direction leg as `idx=0 offset=coordcount<<3`
        // (r717 offset-mode log: `idx=1 offset=0x0` alternating with the
        // retargeted `idx=1 offset=0x5ed0`, while loc0 only ever saw the
        // big family's client stagings).  With a bound VBO this same-shape
        // vec2 leg is the position leg: force it onto `a_VertexXY` so both
        // legs land on distinct synth locations.
        let bound = gpu_u32_clear(|| unsafe {
            let mut binding = 0i32;
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut binding);
            drain_host_gl_errors();
            binding.max(0) as u32
        });
        match bound
            .filter(|binding| *binding != 0)
            .and_then(|_| synth_attrib_loc("a_VertexXY"))
        {
            Some(loc) => {
                static XY_LOGS: AtomicU32 = AtomicU32::new(0);
                if XY_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                    log::info!(
                        "GPU: map xy-leg idx1 stride=8 offset=0 bound_vbo={} -> attrib {loc} (a_VertexXY)",
                        bound.unwrap_or(0)
                    );
                }
                loc
            }
            None => index,
        }
    } else {
        index
    };
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
        // r716g: offset 0 with an ARRAY_BUFFER bound on the map target is
        // real VBO-offset semantics - the combined pass latches
        // `a_VertexXY` at offset 0 of its bound resource VBO (r716 probe:
        // loc0 froze on the internal ZERO_VBO id=6 and every xy leg read
        // zeros).  Only the unbound case is the engine's "clear this
        // attribute" idiom that wants the zero-VBO stand-in.
        let zero_only = current_gpu_target() != GpuTarget::Map
            || gpu_u32_clear(|| unsafe {
                let mut binding = 0i32;
                gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut binding);
                drain_host_gl_errors();
                binding.max(0) as u32
            })
            .unwrap_or(0)
                == 0;
        if zero_only {
            gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, Vec::new());
            return 0;
        }
    }

    // Client-array vs VBO-offset semantics are decided by the ARRAY_BUFFER
    // binding at call time.  The map engine keeps road geometry in its own
    // geometry VBOs and passes offsets, and it does NOT keep that buffer bound
    // across the VAP call (r671: idx=0 ptr=0x20, idx=1 ptr=0x24 with
    // ARRAY_BUFFER=0 at VAP time, then `glBindBuffer(ARRAY_BUFFER, 9)` right
    // before the draw).  An offset below a page is never a heap pointer, so
    // treat it as offset semantics unconditionally; bigger small values may
    // still arrive bound, and a genuinely unbound small-pointer client array
    // would fault on real hardware too.
    if current_gpu_target() == GpuTarget::Map && pointer < 0x10000 {
        let bound = gpu_u32_clear(|| unsafe {
            let mut binding = 0i32;
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut binding);
            drain_host_gl_errors();
            binding.max(0) as u32
        });
        static OFFSET_LOGS: AtomicU32 = AtomicU32::new(0);
        let hits = OFFSET_LOGS.fetch_add(1, Ordering::Relaxed);
        if hits < 8 {
            log::info!(
                "GPU: map VAP offset-mode idx={} size={} type={:#x} stride={} offset={:#x} bound_vbo={}",
                index,
                size,
                kind,
                stride,
                pointer,
                bound.unwrap_or(0)
            );
        }
        gpu_void_clear(move || unsafe {
            gl::VertexAttribPointer(
                index,
                size,
                kind,
                normalized,
                stride,
                pointer as *const std::ffi::c_void,
            );
            drain_host_gl_errors();
        });
        return 0;
    }
    // A pointer at or above 64 KiB is engine heap (the staging arenas live
    // there), never a VBO byte offset: the engine's real offsets are all
    // well below that (r683: 0x390..0x63e0).  Treat it as a client array
    // regardless of the current ARRAY_BUFFER binding - the engine binds its
    // own VBOs around the call but passes heap pointers (the real driver
    // latches the pointer at VAP time and the binding at draw time does not
    // matter).
    let per_vertex = (stride.max(0) as usize)
        .max(size.max(1) as usize)
        .max(4)
        .min(256);
    // Road DTM strips draw 8700+ vertices in one `glDrawArrays` (r661d: the
    // engine fan-out read past a 4096-vertex scratch copy and the async
    // host rasterizer segfaulted), so size the client-array window for that.
    let max_verts = 32768usize;
    let requested = per_vertex.saturating_mul(max_verts).min(MAX_TEXTURE_BYTES);

    if vap_legacy() {
        let Some((_, data)) = read_best_effort_bytes(unicorn, pointer, requested) else {
            gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, Vec::new());
            return 0;
        };
        gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, data);
        return 0;
    }

    // Read the client array into the reusable scratch (never a fresh Vec),
    // shrinking the length until the read succeeds (best effort).
    let mut scratch = match VAP_SCRATCH.lock() {
        Ok(scratch) => scratch,
        Err(_) => return 0,
    };
    let mut len = requested.max(4);
    let mut readable = false;
    while len >= 4 {
        if scratch.len() < len {
            scratch.resize(len, 0);
        }
        if unicorn.mem_read(pointer as u64, &mut scratch[..len]).is_ok() {
            readable = true;
            break;
        }
        len /= 2;
    }
    if !readable {
        gpu_vertex_attrib_pointer(index, size, kind, normalized, stride, pointer, Vec::new());
        return 0;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&scratch[..len], &mut hasher);
    let hash = std::hash::Hasher::finish(&hasher);

    let cached = VAP_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(&pointer).copied())
        .filter(|(_, cached_hash)| *cached_hash == hash)
        .map(|(buffer, _)| buffer);

    if let Some(buffer) = cached {
        // Unchanged geometry: just point the attribute at the existing VBO.
        if current_gpu_target() == GpuTarget::Map {
            if let Ok(mut regs) = VAP_ATTRIB_REGS.lock() {
                regs.insert(index, (pointer, len, hash, buffer));
            }
        }
        gpu_void_clear(move || unsafe {
            let mut previous = 0i32;
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);
            gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
            gl::VertexAttribPointer(index, size, kind, normalized, stride, std::ptr::null());
            gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
        });
        return 0;
    }

    // New or changed geometry at this pointer: (re)specify its dedicated VBO.
    let data = scratch[..len].to_vec();
    let output = gpu_call(move || {
        let mut buffer = VAP_CACHE
            .lock()
            .ok()
            .and_then(|cache| cache.get(&pointer).copied())
            .map(|(buffer, _)| buffer)
            .unwrap_or(0);
        let mut previous = 0i32;
        unsafe {
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);
            if buffer == 0 {
                gl::GenBuffers(1, &mut buffer);
                if buffer == 0 {
                    buffer = next_fallback_id();
                }
            }
            gl::BindBuffer(gl::ARRAY_BUFFER, buffer);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                data.len() as isize,
                data.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );
            gl::VertexAttribPointer(index, size, kind, normalized, stride, std::ptr::null());
            gl::BindBuffer(gl::ARRAY_BUFFER, previous as u32);
            drain_host_gl_errors();
        }
        GpuOutput {
            out_u32: vec![buffer],
            ..Default::default()
        }
    });
    if let Some(output) = output {
        if let Some(&buffer) = output.out_u32.first() {
            if let Ok(mut cache) = VAP_CACHE.lock() {
                cache.insert(pointer, (buffer, hash));
            }
            if current_gpu_target() == GpuTarget::Map {
                // Remember which guest client array backs each attribute so
                // the draw path can re-read it.  Client-array semantics are
                // "read at draw time": the engine fills its vertex staging
                // buffer AFTER glVertexAttribPointer, and a snapshot taken at
                // VAP time only ever captures zeros (r667 probe: every road
                // draw fed the host an all-zero strip).
                if let Ok(mut regs) = VAP_ATTRIB_REGS.lock() {
                    regs.insert(
                        index,
                        (pointer, len, hash, buffer),
                    );
                }
                static REG_LOGS: AtomicU32 = AtomicU32::new(0);
                let reg_hits = REG_LOGS.fetch_add(1, Ordering::Relaxed);
                if reg_hits < 8 {
                    let head: Vec<i16> = scratch[..len]
                        .chunks_exact(2)
                        .take(8)
                        .map(|w| i16::from_le_bytes([w[0], w[1]]))
                        .collect();
                    log::info!(
                        "GPU: map vap register idx={} ptr={:#x} len={} vbo={} head={:?}",
                        index,
                        pointer,
                        len,
                        buffer,
                        head
                    );
                }
            }
        }
    }
    0
}

/// Texture name most recently bound on the HMI target, mirrored from the guest
/// `glBindTexture` calls (the guest and host names are identical there).
static HMI_LAST_BOUND_TEXTURE: AtomicU32 = AtomicU32::new(0);

/// The texture prochmi imports procmap's map surface into.  `GUI_GL_Texture`
/// allocates exactly one 800x480 texture with a NULL data pointer and the
/// display engine fills it on the real unit; the import happens once, so without
/// help the layer keeps whatever it was allocated with (black) and the HMI
/// paints an opaque black rectangle over the map.  Recognise the texture by its
/// unique size and keep it fed from `MAP_SURFACE_BYTES`.
static MAP_LAYER_TEXTURE: AtomicU32 = AtomicU32::new(0);

pub fn map_layer_texture() -> u32 {
    MAP_LAYER_TEXTURE.load(Ordering::Acquire)
}

/// True when `addr` points into the guest copy of the shared map surface.
fn map_surface_contains(addr: u32) -> bool {
    map_surface_bases()
        .lock()
        .map(|bases| {
            bases.values().any(|&base| {
                addr >= base && addr < base.wrapping_add(MAP_SURFACE_SIZE as u32)
            })
        })
        .unwrap_or(false)
}

// Recognise prochmi's map-layer texture: the unique 800x480 upload on the HMI
// target, or any upload whose source is the shared map surface itself.
fn note_map_layer_texture(
    unicorn: &mut Unicorn<'_, Context>,
    width: i32,
    height: i32,
    data_addr: u32,
) {
    if current_gpu_target() != GpuTarget::Hmi {
        return;
    }
    if !(map_surface_contains(data_addr)
        || (width == MAP_SURFACE_WIDTH as i32 && height == MAP_SURFACE_HEIGHT as i32))
    {
        return;
    }
    let name = HMI_LAST_BOUND_TEXTURE.load(Ordering::Relaxed);
    if name == 0 || MAP_LAYER_TEXTURE.swap(name, Ordering::AcqRel) == name {
        return;
    }
    log::info!("GPU: map layer texture of the HMI is {:#x}", name);
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
    note_map_layer_texture(unicorn, width, height, data_addr);
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
    note_map_layer_texture(unicorn, width, height, data_addr);
    let data = texture_bytes(unicorn, format, kind, width, height, data_addr);
    gpu_tex_sub_image2d(target, level, x, y, width, height, format, kind, data);
    0
}

fn gl_backend_api(unicorn: &mut Unicorn<'_, Context>, name: &str) -> Option<u32> {
    let result = gl_backend_api_inner(unicorn, name);
    // Attribute sticky host GL errors to the exact guest GL call: ES errors
    // queue globally, so without this a draw reports the victim, not the
    // culprit (r674: road draws failed with 1282 of unknown origin).
    if current_gpu_target() == GpuTarget::Map {
        static ERROR_TAG_LOGS: AtomicU32 = AtomicU32::new(0);
        if ERROR_TAG_LOGS.fetch_add(1, Ordering::Relaxed) < 4000 {
            let tagged = gpu_u32_clear(|| unsafe {
                let mut seen = 0u32;
                let mut first = 0u32;
                for _ in 0..4 {
                    match gl::GetError() {
                        0 => break,
                        err if seen == 0 => first = err,
                        _ => {}
                    }
                    seen += 1;
                }
                first | seen.wrapping_shl(16)
            });
            let packed = tagged.unwrap_or(0);
            if packed & 0xffff != 0 || packed >> 16 > 1 {
                log::info!(
                    "GPU: map GL error {:#x} attributed to {} (residual {})",
                    packed & 0xffff,
                    name,
                    packed >> 16
                );
            }
        }
    }
    result
}

fn gl_backend_api_inner(unicorn: &mut Unicorn<'_, Context>, name: &str) -> Option<u32> {
    match name {
        "glShaderBinary" => {
            // The map engine hands us GPU-specific precompiled binaries
            // (Vivante) that the host driver must reject; substitute a GLSL
            // ES source for its shader objects instead (see
            // `SYNTH_VERT_SHADER`).  Everyone else keeps the old
            // unsupported-binary answer so their source fallback runs.
            // On by default (a bare `nissan_connect3_emulator` run draws the
            // map); set `EMU_SYNTH_SHADERS=0` to restore the reject-binaries
            // behavior.
            if current_gpu_target() != GpuTarget::Map
                || std::env::var("EMU_SYNTH_SHADERS").as_deref() == Ok("0")
            {
                PENDING_GL_ERROR.with(|flag| flag.set(true));
                return Some(0);
            }
            let count = ureg(unicorn, RegisterARM::R0).min(64);
            let shaders = ureg(unicorn, RegisterARM::R1);
            for i in 0..count {
                let id = read_u32(unicorn, shaders.wrapping_add(i * 4));
                if id == 0 || id == u32::MAX {
                    continue;
                }
                let stage = GL_SHADER_STAGES.lock().ok().and_then(|m| m.get(&id).copied());
                let source = match stage {
                    Some(GL_VERTEX_SHADER_TYPE) => SYNTH_VERT_SHADER,
                    _ => SYNTH_FRAG_SHADER,
                };
                // The binary path never calls `glCompileShader` (that is the
                // point of binaries), so upload and compile the substituted
                // source right here or `glLinkProgram` links empty shaders.
                if let Ok(cstring) = CString::new(source) {
                    let diag_stage = stage;
                    gpu_void_clear(move || unsafe {
                        let pointer = cstring.as_ptr();
                        let length = cstring.as_bytes().len() as i32;
                        gl::ShaderSource(id, 1, &pointer, &length);
                        gl::CompileShader(id);
                        static SUB_LOGS: AtomicU32 = AtomicU32::new(0);
                        let hits = SUB_LOGS.fetch_add(1, Ordering::Relaxed);
                        if hits < 6 {
                            let mut status = 0i32;
                            gl::GetShaderiv(id, gl::COMPILE_STATUS, &mut status);
                            let mut log_len = 0i32;
                            gl::GetShaderiv(id, gl::INFO_LOG_LENGTH, &mut log_len);
                            let mut info =
                                vec![0u8; (log_len.max(0).min(255) as usize) + 1];
                            if log_len > 0 {
                                gl::GetShaderInfoLog(
                                    id,
                                    info.len() as i32,
                                    &mut log_len,
                                    info.as_mut_ptr() as *mut std::ffi::c_char,
                                );
                            }
                            log::info!(
                                "GPU: synth shader substitute id={} stage={:?} compile={} info={}",
                                id,
                                diag_stage,
                                status,
                                String::from_utf8_lossy(&info).trim()
                            );
                        }
                    });
                }
                if let Ok(mut m) = GL_SYNTH_SHADER_SOURCE.lock() {
                    m.remove(&id);
                }
            }
            return Some(0);
        }
        "glDrawArrays" => {
            let mode = ureg(unicorn, RegisterARM::R0);
            let first = ureg(unicorn, RegisterARM::R1) as i32;
            let count = ureg(unicorn, RegisterARM::R2) as i32;
            if current_gpu_target() == GpuTarget::Map
                && std::env::var_os("EMU_NO_ENGINE_DRAW").is_some()
            {
                return Some(0);
            }
            let count = if current_gpu_target() == GpuTarget::Map {
                std::env::var("EMU_ENGINE_DRAW_MAX")
                    .ok()
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(count)
                    .min(count)
            } else {
                count
            };
            let drew = if current_gpu_target() == GpuTarget::Map {
                refresh_map_client_arrays(unicorn);
                refresh_map_engine_uniforms(unicorn);
                // Keep any host GL error the road draw produces: the draw is
                // the first thing in the emulator that exercises client-array
                // sourced GL_SHORT attributes, and a silently rejected draw
                // would otherwise look like "rendered but invisible".
                gpu_void_clear(move || unsafe {
                    // r750: same colour safety net as the multi-draw arm.
                    // The big street strip (count ~3196) arrives through this
                    // entry point, and the engine pushes a black/transparent
                    // `a_color` constant, so without a forced stroke colour
                    // the real road geometry rasterises invisible.
                    let mut program = 0i32;
                    gl::GetIntegerv(gl::CURRENT_PROGRAM, &mut program);
                    let mut forced = false;
                    if count >= 300 {
                        let cname = std::ffi::CString::new("a_color").unwrap();
                        let loc = if program > 0 {
                            gl::GetAttribLocation(program as u32, cname.as_ptr())
                        } else {
                            -1
                        };
                        let tracked = LAST_MAP_ATTRIB4.lock().ok().and_then(|g| *g);
                        if loc >= 0
                            && tracked.map_or(true, |(_, c)| {
                                c[3] < 0.02 || c[0] + c[1] + c[2] < 0.02
                            })
                        {
                            gl::DisableVertexAttribArray(loc as u32);
                            gl::VertexAttrib4f(loc as u32, 0.85, 0.55, 0.10, 1.0);
                            forced = true;
                        }
                    }
                    probe_map_draw_vertices(mode, first, count);
                    gl::DrawArrays(mode, first, count);
                    let mut errors = [0u32; 4];
                    let mut n = 0usize;
                    while n < errors.len() {
                        let err = gl::GetError();
                        if err == 0 {
                            break;
                        }
                        errors[n] = err;
                        n += 1;
                    }
                    if n > 0 {
                        static DRAW_ERROR_LOGS: std::sync::atomic::AtomicU32 =
                            std::sync::atomic::AtomicU32::new(0);
                        let hits = DRAW_ERROR_LOGS.fetch_add(1, Ordering::Relaxed);
                        if hits < 10 {
                            log::info!(
                                "GPU: map draw mode={} first={} count={} host errors={:?}",
                                mode,
                                first,
                                count,
                                &errors[..n]
                            );
                        }
                    }
                })
            } else {
                gpu_void_v3(gl::DrawArrays, mode, first, count)
            };
            if drew {
                note_real_gl_output(unicorn, "draw");
            }
            return Some(0);
        }
        "glDrawElements" => {
            let mode = ureg(unicorn, RegisterARM::R0);
            let count = ureg(unicorn, RegisterARM::R1) as i32;
            let kind = ureg(unicorn, RegisterARM::R2);
            let offset = ureg(unicorn, RegisterARM::R3);
            if current_gpu_target() == GpuTarget::Map {
                refresh_map_client_arrays(unicorn);
            }
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
            let prog = ureg(unicorn, RegisterARM::R0);
            let shader = ureg(unicorn, RegisterARM::R1);
            if current_gpu_target() == GpuTarget::Map {
                static ATTACH_LOGS: AtomicU32 = AtomicU32::new(0);
                let hits = ATTACH_LOGS.fetch_add(1, Ordering::Relaxed);
                if hits < 10 {
                    let _ = gpu_void_clear(move || unsafe {
                        let mut status = 0i32;
                        gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut status);
                        let mut source_len = 0i32;
                        gl::GetShaderiv(shader, gl::SHADER_SOURCE_LENGTH, &mut source_len);
                        log::info!(
                            "GPU: map attach prog={} shader={} compile={} source_len={}",
                            prog,
                            shader,
                            status,
                            source_len
                        );
                    });
                }
            }
            gpu_void_v2(gl::AttachShader, prog, shader);
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
            let fbo_target = ureg(unicorn, RegisterARM::R0);
            let guest_fb = ureg(unicorn, RegisterARM::R1);
            let gpu_target = current_gpu_target();
            gpu_void_clear(move || unsafe {
                let host_fb = resolve_guest_fbo(gpu_target, guest_fb);
                gl::BindFramebuffer(fbo_target, host_fb);
            });
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
            let name = ureg(unicorn, RegisterARM::R1);
            if name != 0 && current_gpu_target() == GpuTarget::Hmi {
                HMI_LAST_BOUND_TEXTURE.store(name, Ordering::Relaxed);
            }
            gpu_void_v2(gl::BindTexture, ureg(unicorn, RegisterARM::R0), name);
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
            if probe_clears() {
                let mask = ureg(unicorn, RegisterARM::R0);
                gpu_void_clear(move || unsafe {
                    let mut color = [0f32; 4];
                    let mut fbo = 0i32;
                    gl::GetFloatv(0x0B22 /* GL_CURRENT_COLOR = clear color */, color.as_mut_ptr());
                    gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fbo);
                    log::info!(
                        "GPU: CLEAR probe mask={:#x} fbo={} color=({:.3},{:.3},{:.3},{:.3})",
                        mask,
                        fbo,
                        color[0],
                        color[1],
                        color[2],
                        color[3]
                    );
                    gl::Clear(mask);
                });
                return Some(0);
            }
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
            let program = ureg(unicorn, RegisterARM::R0);
            gpu_void_v1(gl::LinkProgram, program);
            if current_gpu_target() == GpuTarget::Map {
                static LINK_LOGS: AtomicU32 = AtomicU32::new(0);
                let hits = LINK_LOGS.fetch_add(1, Ordering::Relaxed);
                if hits < 30 {
                    let _ = gpu_void_clear(move || unsafe {
                        let mut status = 0i32;
                        let mut uniforms = 0i32;
                        gl::GetProgramiv(program, gl::LINK_STATUS, &mut status);
                        gl::GetProgramiv(program, gl::ACTIVE_UNIFORMS, &mut uniforms);
                        let mut log_len = 0i32;
                        gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut log_len);
                        let mut info = vec![0u8; (log_len.max(0).min(255) as usize) + 1];
                        if log_len > 0 {
                            gl::GetProgramInfoLog(
                                program,
                                info.len() as i32,
                                &mut log_len,
                                info.as_mut_ptr() as *mut std::ffi::c_char,
                            );
                        }
                        log::info!(
                            "GPU: map link prog={} link={} uniforms={} info={}",
                            program,
                            status,
                            uniforms,
                            String::from_utf8_lossy(&info).trim()
                        );
                    });
                }
            }
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
            GL_LIVE_SHADERS.fetch_add(1, Ordering::Relaxed);
            let kind = ureg(unicorn, RegisterARM::R0);
            let created = gpu_ret_u1(gl::CreateShader, kind);
            if let Some(id) = created {
                if id != 0 {
                    if let Ok(mut m) = GL_SHADER_STAGES.lock() {
                        m.insert(id, kind);
                    }
                }
            }
            return created;
        }
        "glCreateProgram" => {
            GL_LIVE_PROGRAMS.fetch_add(1, Ordering::Relaxed);
            return gpu_ret_u0(gl::CreateProgram);
        }
        "glCompileShader" => {
            let id = ureg(unicorn, RegisterARM::R0);
            let synth = GL_SYNTH_SHADER_SOURCE
                .lock()
                .ok()
                .and_then(|mut m| m.remove(&id));
            if let Some(source) = synth {
                match CString::new(source) {
                    Ok(cstring) => {
                        gpu_void_clear(move || unsafe {
                            let pointer = cstring.as_ptr();
                            let length = cstring.as_bytes().len() as i32;
                            gl::ShaderSource(id, 1, &pointer, &length);
                            gl::CompileShader(id);
                        });
                    }
                    Err(_) => {}
                }
                return Some(0);
            }
            gpu_void_v1(gl::CompileShader, id);
            return Some(0);
        }
        "glDeleteProgram" => {
            gpu_void_v1(gl::DeleteProgram, ureg(unicorn, RegisterARM::R0));
            return Some(0);
        }
        "glDeleteShader" => {
            let id = ureg(unicorn, RegisterARM::R0);
            if let Ok(mut m) = GL_SHADER_STAGES.lock() {
                m.remove(&id);
            }
            if let Ok(mut m) = GL_SYNTH_SHADER_SOURCE.lock() {
                m.remove(&id);
            }
            gpu_void_v1(gl::DeleteShader, id);
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
        "glDeleteFramebuffers" => return Some(route_delete_framebuffers(unicorn)),
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
                // Keep the "default framebuffer is name 0" illusion intact:
                // a bind the guest made as 0 landed on its private default
                // FBO, and a guessed name may have been re-pointed at a fresh
                // host object. Report what the guest thinks it bound.
                let value = match pname {
                    GL_FRAMEBUFFER_BINDING | GL_READ_FRAMEBUFFER_BINDING => {
                        guest_framebinding(current_gpu_target(), value)
                    }
                    _ => value,
                };
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
            if current_gpu_target() == GpuTarget::Map {
                static BUFFER_LOGS: AtomicU32 = AtomicU32::new(0);
                let hits = BUFFER_LOGS.fetch_add(1, Ordering::Relaxed);
                if hits < 24 {
                    let mut head = [0u8; 16];
                    let read_ok = unicorn
                        .mem_read(data_addr as u64, &mut head)
                        .is_ok();
                    let binding = gpu_u32_clear(move || unsafe {
                        let mut bound = 0i32;
                        gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut bound);
                        bound.max(0) as u32
                    })
                    .unwrap_or(0);
                    log::info!(
                        "GPU: map bufferData target={:#x} size={:#x} src={:#x} read={} bound_vbo={} head={:02x?}",
                        target,
                        size,
                        data_addr,
                        read_ok,
                        binding,
                        head
                    );
                }
            }
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
        "glMultiDrawArraysEXT" => {
            // `vDrawCombinedStreets` draws every street bank with one
            // `glMultiDrawArraysEXT(5, first[], count[], primcount)` whose
            // first/count vectors live in guest memory (`ms_apu32First`,
            // `ms_apu32Count`).  Expand to host `glDrawArrays` per primitive;
            // the client-array refresh runs once for the shared VBO latches.
            let mode = ureg(unicorn, RegisterARM::R0) as i32;
            let first_ptr = ureg(unicorn, RegisterARM::R1);
            let count_ptr = ureg(unicorn, RegisterARM::R2);
            let primcount = (ureg(unicorn, RegisterARM::R3) as i32).clamp(0, 4096);
            if primcount > 0 && first_ptr != 0 && count_ptr != 0 {
                let firsts = read_u32_array(unicorn, first_ptr, primcount as usize);
                let counts = read_u32_array(unicorn, count_ptr, primcount as usize);
                static MULTI_LOGS: AtomicU32 = AtomicU32::new(0);
                if MULTI_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                    log::info!(
                        "GPU: map multi-draw mode={mode} primcount={primcount} first={:?} count={:?}",
                        firsts.iter().take(4).collect::<Vec<_>>(),
                        counts.iter().take(4).collect::<Vec<_>>()
                    );
                }
                refresh_map_client_arrays(unicorn);
                refresh_map_engine_uniforms(unicorn);
                let ranges: Vec<(i32, i32)> = firsts
                    .iter()
                    .zip(counts.iter())
                    .map(|(first, count)| (*first as i32, *count as i32))
                    .filter(|(_, count)| *count > 0)
                    .collect();
                let drew = gpu_void_clear(move || unsafe {
                    // r711: color safety net.  The combined pass pushes the
                    // road RGBA through `glVertexAttrib4fv` onto `a_color`;
                    // if that never arrived (or was black/transparent), draw
                    // a bright stroke so the real geometry becomes visible.
                    let mut program = 0i32;
                    gl::GetIntegerv(gl::CURRENT_PROGRAM, &mut program);
                    let cname = std::ffi::CString::new("a_color").unwrap();
                    let loc = if program > 0 {
                        gl::GetAttribLocation(program as u32, cname.as_ptr())
                    } else {
                        -1
                    };
                    let tracked = LAST_MAP_ATTRIB4.lock().ok().and_then(|g| *g);
                    let mut forced = false;
                    if loc >= 0
                        && tracked.map_or(true, |(_, c)| {
                            c[3] < 0.02 || c[0] + c[1] + c[2] < 0.02
                        })
                    {
                        gl::DisableVertexAttribArray(loc as u32);
                        gl::VertexAttrib4f(loc as u32, 0.85, 0.55, 0.10, 1.0);
                        forced = true;
                        static FORCE_LOGS: AtomicU32 = AtomicU32::new(0);
                        if FORCE_LOGS.fetch_add(1, Ordering::Relaxed) < 3 {
                            log::info!(
                                "GPU: map multi-draw forced road color (tracked={tracked:?})"
                            );
                        }
                    }
                    probe_map_draw_vertices(mode as u32, 0, ranges.iter().map(|(_, c)| *c).sum());
                    // r725: centre the view on this draw's own road block
                    // (pan derived from the engine's real geometry, see
                    // `SYNTH_VERT_SHADER`'s `u_v2Pan` comment).
                    if program > 0 {
                        let pname = std::ffi::CString::new("u_v2Pan").unwrap();
                        let loc = gl::GetUniformLocation(program as u32, pname.as_ptr());
                        if loc >= 0 {
                            let pan = LAST_MAP_PAN.lock().ok().and_then(|g| *g).unwrap_or((0.0, 0.0));
                            gl::Uniform2f(loc, pan.0, pan.1);
                            static PAN_LOGS: AtomicU32 = AtomicU32::new(0);
                            if PAN_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                                log::info!(
                                    "GPU: map u_v2Pan=({:.1}, {:.1}) n={}",
                                    pan.0,
                                    pan.1,
                                    PAN_LOGS.load(Ordering::Relaxed)
                                );
                            }
                        }
                    }
                    for (first, count) in ranges {
                        gl::DrawArrays(mode as u32, first, count);
                    }
                    drain_host_gl_errors();
                });
                if drew {
                    note_real_gl_output(unicorn, "draw");
                }
            }
            return Some(0);
        }
        "glMapBufferOES" => {
            // The engine's dynamic-geometry path (r684): it reserves VBOs with
            // `glBufferData(size, NULL)`, then fills them through
            // `glMapBufferOES`/CPU writes/`glUnmapBufferOES` (its wrapper calls
            // the pointer returned by `eglGetProcAddress`).  Hand out a
            // reusable guest arena; the unmap uploads it into the VBO bound to
            // the target, which is what the real driver's unmapping does.
            let target = ureg(unicorn, RegisterARM::R0);
            let size = (ureg(unicorn, RegisterARM::R1) as usize).min(MAX_TEXTURE_BYTES).max(16);
            let mut arenas = match MAP_BUFFER_ARENAS.lock() {
                Ok(arenas) => arenas,
                Err(_) => return Some(0),
            };
            let (address, capacity, pending) = arenas.entry(target).or_insert((0, 0, 0));
            if *capacity < size {
                *address = alloc_guest_bytes(unicorn, size);
                *capacity = if *address == 0 { 0 } else { size };
            }
            *pending = size;
            static MAP_LOGS: AtomicU32 = AtomicU32::new(0);
            if MAP_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                log::info!(
                    "GPU: map MapBufferOES target={target:#x} size={size:#x} -> guest {:#x}",
                    *address
                );
            }
            let address = *address;
            drop(arenas);
            return Some(address);
        }
        "glUnmapBufferOES" => {
            let target = ureg(unicorn, RegisterARM::R0);
            let (address, size) = MAP_BUFFER_ARENAS
                .lock()
                .ok()
                .and_then(|mut arenas| arenas.get_mut(&target).map(|e| (e.0, e.2)))
                .unwrap_or((0, 0));
            if address == 0 || size == 0 {
                return Some(1);
            }
            let mut data = vec![0u8; size];
            if unicorn.mem_read(address as u64, &mut data).is_err() {
                return Some(1);
            }
            let (binding_pname, binding_enum) = if target == 0x8893 {
                (gl::ELEMENT_ARRAY_BUFFER_BINDING, gl::ELEMENT_ARRAY_BUFFER)
            } else {
                (gl::ARRAY_BUFFER_BINDING, gl::ARRAY_BUFFER)
            };
            let head: Vec<f32> = data
                .chunks_exact(4)
                .take(4)
                .map(|w| f32::from_le_bytes([w[0], w[1], w[2], w[3]]))
                .collect();
            let _ = gpu_void_clear(move || unsafe {
                let mut bound = 0i32;
                gl::GetIntegerv(binding_pname, &mut bound);
                if bound > 0 {
                    gl::BindBuffer(binding_enum, bound as u32);
                    gl::BufferData(
                        binding_enum,
                        data.len() as isize,
                        data.as_ptr() as *const _,
                        gl::DYNAMIC_DRAW,
                    );
                }
                drain_host_gl_errors();
            });
            static UNMAP_LOGS: AtomicU32 = AtomicU32::new(0);
            if UNMAP_LOGS.fetch_add(1, Ordering::Relaxed) < 6 {
                log::info!(
                    "GPU: map UnmapBufferOES target={target:#x} size={size:#x} head={head:?}",
                );
            }
            return Some(1);
        }
        "glBufferSubData" => {
            let target = ureg(unicorn, RegisterARM::R0);
            let offset = ureg(unicorn, RegisterARM::R1) as isize;
            let size = (ureg(unicorn, RegisterARM::R2) as usize).min(MAX_TEXTURE_BYTES);
            let data_addr = ureg(unicorn, RegisterARM::R3);
            let mut readback_vbo: Option<u32> = None;
            if current_gpu_target() == GpuTarget::Map {
                // r693: this call is the road-geometry upload; `log_api`
                // throttles per thread and hid it in every run so far, so log
                // it explicitly for the map target.
                static SUB_LOGS: AtomicU32 = AtomicU32::new(0);
                if SUB_LOGS.fetch_add(1, Ordering::Relaxed) < 12 {
                    let mut head = [0u8; 16];
                    let read_ok = unicorn.mem_read(data_addr as u64, &mut head).is_ok();
                    let binding = gpu_u32_clear(move || unsafe {
                        let mut bound = 0i32;
                        gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut bound);
                        bound.max(0) as u32
                    })
                    .unwrap_or(0);
                    log::info!(
                        "GPU: map bufferSubData target={target:#x} offset={offset:#x} size={size:#x} src={data_addr:#x} read={read_ok} bound_vbo={binding} head={head:02x?}",
                    );
                    readback_vbo = Some(binding);
                }
            }
            let data = read_best_effort_bytes(unicorn, data_addr, size)
                .map(|(_, data)| data)
                .unwrap_or_default();
            gpu_buffer_sub_data(target, offset, size, data);
            if let Some(binding) = readback_vbo {
                let readback_offset = offset.max(0) as isize;
                let _ = gpu_void_clear(move || unsafe {
                    // r700: `GetBufferSubData` has never returned non-zero
                    // bytes on this host in any run, so redo the readback
                    // through a read-only map and report the GL errors that
                    // `drain_host_gl_errors` would otherwise swallow.
                    let mut previous = 0i32;
                    gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut previous);
                    let mut raw = [0u8; 16];
                    gl::BindBuffer(gl::ARRAY_BUFFER, binding);
                    let mut raw_err = 0u32;
                    let data = gl::MapBufferRange(
                        gl::ARRAY_BUFFER,
                        readback_offset,
                        16,
                        gl::MAP_READ_BIT,
                    );
                    if data.is_null() {
                        raw_err = gl::GetError();
                    } else {
                        std::ptr::copy_nonoverlapping(
                            data as *const u8,
                            raw.as_mut_ptr(),
                            raw.len(),
                        );
                        if gl::UnmapBuffer(gl::ARRAY_BUFFER) == 0 {
                            raw_err = 0xffff;
                        }
                    }
                    gl::BindBuffer(gl::ARRAY_BUFFER, previous.max(0) as u32);
                    let pending = gl::GetError();
                    drain_host_gl_errors();
                    log::info!(
                        "GPU: map subdata readback vbo={binding} offset={:#x} raw={raw:02x?} map_err={raw_err:#x} pending={pending:#x}",
                        readback_offset,
                    );
                });
            }
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
        "glGetActiveUniform" => return Some(route_active_uniform(unicorn)),
        "glGetActiveAttrib" => return Some(route_active_attrib(unicorn)),
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
            // r711: remember the last engine-supplied constant attribute for
            // the map target (the combined pass pushes the road RGBA through
            // this call, index 1 = `a_color`); the multi-draw arm falls back
            // to a visible stroke color when it never arrived or is empty.
            if current_gpu_target() == GpuTarget::Map && index < 8 && values.len() == 4 {
                if let Ok(mut last) = LAST_MAP_ATTRIB4.lock() {
                    *last = Some((index, [values[0], values[1], values[2], values[3]]));
                }
                static V4F_LOGS: AtomicU32 = AtomicU32::new(0);
                if V4F_LOGS.fetch_add(1, Ordering::Relaxed) < 8 {
                    log::info!(
                        "GPU: map 4fv idx={} v=[{:.3}, {:.3}, {:.3}, {:.3}] n={}",
                        index,
                        values[0],
                        values[1],
                        values[2],
                        values[3],
                        V4F_LOGS.load(Ordering::Relaxed)
                    );
                }
            }
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

pub fn request_draw_list(unicorn: &mut Unicorn<'_, Context>, count: i32) {
    let count = count.clamp(1, 1024);
    if !ensure_backend(unicorn) {
        return;
    }
    if gpu_void_clear(move || unsafe {
        gl::DrawArrays(gl::TRIANGLES, 0, count);
    }) {
        note_real_gl_output(unicorn, "draw");
    }
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
