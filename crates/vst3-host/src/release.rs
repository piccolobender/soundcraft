//! Deferred release of plugin instances.
//!
//! Some plugins post work to the main thread's run loop that still points at the instance: Auto-Tune
//! Pro queues `juce::MessageManager` callbacks (`checkConformToScaleStatus`, parameter gestures)
//! from its processor and controller. An instance terminated and released before the run loop gets
//! to them leaves those callbacks with a dangling pointer, and the app crashes on the main thread
//! a moment later — typically right after a bounce, whose render instances are dropped as soon as
//! the file is written. With a delay set (the desktop app sets one), instances are parked here and
//! terminated on the main thread once the run loop has had time to run what they queued.

use crate::ffi::{Instance, main_queue};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Delay before a parked instance is released, in milliseconds (0: release at once).
static DELAY_MS: AtomicU64 = AtomicU64::new(0);

struct Parked {
    due: Instant,
    /// Held only so that it is dropped (released) when due.
    _inst: Instance,
}

static PARKED: Mutex<Vec<Parked>> = Mutex::new(Vec::new());

pub fn set_delay(delay: Option<Duration>) {
    let ms = delay.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    DELAY_MS.store(ms, Ordering::Relaxed);
    if ms == 0 {
        release_all();
    }
}

/// Releases `inst` now, or parks it for a later release on the main thread.
pub fn release(inst: Instance) {
    let ms = DELAY_MS.load(Ordering::Relaxed);
    if ms == 0 || !main_queue::available() {
        drop(inst);
        return;
    }
    let delay = Duration::from_millis(ms);
    PARKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Parked { due: Instant::now() + delay, _inst: inst });
    main_queue::after(delay, release_due);
}

/// Releases the parked instances that are due (main thread, from the run loop).
fn release_due() {
    let now = Instant::now();
    let due: Vec<Parked> = {
        let mut parked = PARKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (due, keep): (Vec<Parked>, Vec<Parked>) = parked.drain(..).partition(|p| p.due <= now);
        *parked = keep;
        due
    };
    // Dropped outside the lock: a plugin's teardown may drop further instances.
    drop(due);
}

/// Releases every parked instance now (before the module exits run at shutdown).
pub fn release_all() {
    let all: Vec<Parked> = std::mem::take(&mut *PARKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    drop(all);
}
