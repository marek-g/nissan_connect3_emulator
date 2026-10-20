//! Host pointer (SDL mouse) -> guest touch bridge for the HMI panel.
//!
//! The GPU backend thread (`gpu::poll_events`, a plain host thread without the
//! VM lock) converts SDL mouse events into `TouchEvent`s expressed in the
//! 800x480 panel coordinate system and pushes them here.  The prochmi GUI
//! mainloop hook pops at most one event per pump iteration and drives the
//! guest's own `GUI_TouchPad_vPressed/vMoved/vReleased` entry points - the
//! same functions `clHSI_CMKeyboard::bExecuteMessage` calls on the device when
//! the real touch controller delivers a sample, so the widget engine sees a
//! stock input path (guest code untouched).
//!
//! The queue is guarded by a plain host mutex that is only ever held for an
//! O(1) push/pop (never across a sleep, never while owning the VM lock), per
//! the emulator-wide no-block-in-guest-context rule.  `try_lock` on both sides
//! makes a lost event a possible-but-rare outcome instead of a deadlock.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// The head-unit panel resolution the HMI and touch controller use.
pub const PANEL_WIDTH: u16 = 800;
pub const PANEL_HEIGHT: u16 = 480;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TouchKind {
    Press,
    Motion,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub kind: TouchKind,
    pub x: u16,
    pub y: u16,
}

static QUEUE: Mutex<VecDeque<TouchEvent>> = Mutex::new(VecDeque::new());
static DROPPED_EVENTS: AtomicU32 = AtomicU32::new(0);

/// Clamp raw panel-space coordinates into the displayable range.
pub fn clamp_panel(x: i32, y: i32) -> (u16, u16) {
    (
        x.clamp(0, PANEL_WIDTH as i32 - 1) as u16,
        y.clamp(0, PANEL_HEIGHT as i32 - 1) as u16,
    )
}

/// Producer side (GPU backend thread, and the scripted tap injector).
///
/// Motion events are coalesced: a trailing motion is replaced in place so a
/// fast drag cannot flood the queue; when the queue is full the oldest motion
/// is evicted first so press/release pairs are never split.
pub fn push_touch(kind: TouchKind, x: u16, y: u16) {
    let Ok(mut queue) = QUEUE.try_lock() else {
        return;
    };
    if kind == TouchKind::Motion {
        if matches!(queue.back(), Some(last) if last.kind == TouchKind::Motion) {
            queue.pop_back();
        }
        if queue.len() >= 64 {
            match queue.iter().position(|event| event.kind == TouchKind::Motion) {
                Some(index) => {
                    queue.remove(index);
                }
                None => {
                    queue.pop_front();
                }
            }
            DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    } else if queue.len() >= 128 {
        // Press/release pairs must survive even a pathological flood; drop
        // the whole queue's worth of stale motions by force here.
        queue.clear();
        DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
    }
    queue.push_back(TouchEvent { kind, x, y });
}

/// Consumer side (prochmi GUI mainloop hook): one event per call, non-blocking.
pub fn pop_touch() -> Option<TouchEvent> {
    QUEUE.try_lock().ok()?.pop_front()
}

pub fn dropped_events() -> u32 {
    DROPPED_EVENTS.load(Ordering::Relaxed)
}

/// The head-unit's hardwired keys the host keyboard stands in for (the
/// widget-engine's `GUI_KeyInjector` takes an id into its 51-entry table; the
/// prochmi hook resolves each of these semantic names to that id at runtime
/// through `GUI_ContentString::ms_apcStrings`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HardKey {
    Back,
    Phone,
    Radio,
    Cd,
    Aux,
    Previous,
    Next,
    Camera,
    Map,
    Nav,
    Info,
    NightDay,
    Setup,
}

/// Queue item for the physical-key bridge.  `Encoder` covers the rotary
/// inputs (volume knob, map/menu knob) which have no release state - the
/// prochmi hook turns each into one `v(Volume)EncoderMoved(dir)` call.
#[derive(Clone, Copy, Debug)]
pub enum KeyEvent {
    Press(HardKey),
    Release(HardKey),
    Encoder { volume: bool, dir: i8 },
}

static KEY_QUEUE: Mutex<VecDeque<KeyEvent>> = Mutex::new(VecDeque::new());

/// Producer side (GPU backend thread SDL keyboard, and scripted tests).
pub fn push_key(event: KeyEvent) {
    if let Ok(mut queue) = KEY_QUEUE.try_lock() {
        if queue.len() >= 64 {
            queue.pop_front();
        }
        queue.push_back(event);
    }
}

/// Consumer side (prochmi GUI mainloop hook): one event per pump iteration.
pub fn pop_key() -> Option<KeyEvent> {
    KEY_QUEUE.try_lock().ok()?.pop_front()
}
