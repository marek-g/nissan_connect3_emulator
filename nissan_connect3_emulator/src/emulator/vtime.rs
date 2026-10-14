//! Virtual world clock (`EMU_REALTIME_FACTOR`).
//!
//! The engine's code is written against the assumption that a queued job is
//! executed long before anything changes the state it reads at execution time
//! (e.g. the `MapObjectsUpdate` job reads `request.state == 2` when it *runs*,
//! not when it is created).  On real hardware that holds trivially: producer
//! and consumer are hardware-speed threads.  In the emulator every guest
//! process shares serialized host time, so consumer latency (seconds of wall
//! time) exceeds the state windows that the guest measures in *its own* clock
//! - the relationship producer/consumer is inverted and behaviour becomes a
//! scheduling lottery (copy branch missed, harvest windows missed, ...).
//!
//! Speeding the consumer up is hard (it is bound to the serialized quantum
//! rotation), so instead the guest's clock is dilated: with factor `F` one
//! wall second advances only `F` guest seconds.  Timer-driven producers
//! (HMI's ~1.5 s render trigger, OSAL tick waits, message timeouts, watchdog
//! deadlines) slow down in wall terms, while the consumer keeps executing at
//! its full wall instruction rate - so it finishes the work of one guest
//! millisecond within a fraction of the corresponding guest window and the
//! original producer/consumer ratio is restored.
//!
//! The factor is a *dilation*, not a simulation of a slower CPU: all clocks
//! the guest can read (`CLOCK_MONOTONIC`/`CLOCK_MONOTONIC_RAW`,
//! `CLOCK_REALTIME`, `gettimeofday`, `st_atime`/`st_mtime` of served files)
//! derive from one virtual "now", and every duration the guest asks us to
//! wait for (nanosleep, monitor ticks, futex/mqueue/signal/event/IOSC
//! timeouts) is converted to a host deadline of `duration / F`.  Guest code
//! cannot observe the difference except by comparing against independent
//! sources - and everything we stamp into guest messages (provider pacing,
//! synthetic periodic events) goes through [`sleep_virtual`]/[`host_interval`]
//! too, so the whole guest-visible world stays mutually consistent.  Wall
//! time still drives fairness (scheduler quanta), host liveness debounces and
//! probe log stamps (`emu_t0_elapsed_ms`), which are not guest-visible clocks.
//!
//! ARM Linux 2.6.32 has no vDSO and the emulator implements every time source
//! the guest can reach (syscall + OSAL monitor services), so there is no
//! bypass path: `timer_create` is inert, no `poll`/`select`/`alarm`/`time`
//! syscalls are implemented, and the vsyscall page is never mapped.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The dilation factor: guest time elapses `F` times slower than wall time.
/// `0.1` means ten wall seconds pass per guest second.  Values outside
/// `(0.0, 1.0]` fall back to `1.0` (undilated, historical behaviour).
pub fn factor() -> f64 {
    static FACTOR: OnceLock<f64> = OnceLock::new();
    *FACTOR.get_or_init(|| match std::env::var("EMU_REALTIME_FACTOR") {
        Ok(raw) => match raw.parse::<f64>() {
            Ok(f) if f > 0.0 && f <= 1.0 => {
                log::info!("vtime: EMU_REALTIME_FACTOR={f} (guest time dilated {:.1}x)", 1.0 / f);
                f
            }
            _ => {
                log::warn!("vtime: ignoring EMU_REALTIME_FACTOR={raw:?} (need 0 < F <= 1)");
                1.0
            }
        },
        Err(_) => 1.0,
    })
}

/// Guest monotonic uptime: emulator start is the guest's boot time, dilated.
pub fn vnow() -> Duration {
    static BOOT: OnceLock<Instant> = OnceLock::new();
    let boot = *BOOT.get_or_init(Instant::now);
    Duration::from_secs_f64(boot.elapsed().as_secs_f64() * factor())
}

/// Guest wall clock: the host epoch at emulator boot, advanced at the
/// dilated rate.  The guest effectively believes the machine booted later
/// than it did; nothing in the guest can tell (no independent time source is
/// exposed - file mtimes served by the emulator and provider message
/// timestamps are stamped from this same clock).
pub fn vepoch() -> Duration {
    static EPOCH0: OnceLock<Duration> = OnceLock::new();
    let base = *EPOCH0.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
    });
    base + vnow()
}

/// Convert a duration the guest expressed in *guest* time into a host
/// deadline (the host must wait `duration / F` wall seconds for it to expire
/// in the guest's view).
pub fn deadline(virtual_duration: Duration) -> Instant {
    Instant::now() + host_interval(virtual_duration)
}

/// Convert an absolute deadline expressed on the virtual epoch clock
/// (`mq_timed*` style) into a host deadline.  Already-expired deadlines map
/// to "now".
pub fn epoch_deadline(virtual_abs: Duration) -> Instant {
    match virtual_abs.checked_sub(vepoch()) {
        None => Instant::now(),
        Some(rest) => Instant::now() + host_interval(rest),
    }
}

/// Wall duration the host must actually wait for `virtual_duration` of guest
/// time to pass.
pub fn host_interval(virtual_duration: Duration) -> Duration {
    Duration::from_secs_f64(virtual_duration.as_secs_f64() / factor())
}

/// Sleep the calling *host* thread for a pacing interval expressed in guest
/// time.  For host-side providers that pace guest-visible event streams
/// (camera nudges, position ticks, synthetic periodic publishes) so their
/// cadence stays constant in the guest's world no matter how much the whole
/// timeline is dilated.  (Host liveness logic - wake polling, takeover
/// debounces, log stamps - must keep using wall time.)
pub fn sleep_virtual(virtual_duration: Duration) {
    std::thread::sleep(host_interval(virtual_duration));
}
