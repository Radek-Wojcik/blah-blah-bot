//! Windows thread power-management guard that prevents idle system sleep while a recording
//! or background processing job is running.
//!
//! Only `ES_SYSTEM_REQUIRED` is requested (never `ES_DISPLAY_REQUIRED`), so the display can
//! still turn off normally while the machine stays awake.

use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};

/// RAII guard that requests `ES_CONTINUOUS | ES_SYSTEM_REQUIRED` on the current thread and
/// clears the request (`ES_CONTINUOUS`) when dropped.
#[derive(Debug)]
pub struct SystemWakeLock {
    active: bool,
}

impl SystemWakeLock {
    /// Request that Windows keep the system awake until this guard is dropped.
    pub fn acquire() -> Self {
        // SAFETY: plain Win32 call with valid execution state flags; sets a per-thread flag.
        let prev = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
        let active = prev.0 != 0;
        if !active {
            tracing::warn!("SetThreadExecutionState failed to acquire system wake lock");
        }
        Self { active }
    }

    /// Alias for [`SystemWakeLock::acquire`].
    pub fn new() -> Self {
        Self::acquire()
    }

    /// True when Windows accepted the execution state request.
    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Default for SystemWakeLock {
    fn default() -> Self {
        Self::acquire()
    }
}

impl Drop for SystemWakeLock {
    fn drop(&mut self) {
        // SAFETY: plain Win32 call with ES_CONTINUOUS to clear the per-thread wake request.
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_drop_system_wake_lock_on_worker_thread() {
        let handle = std::thread::Builder::new()
            .name("queue-driver-test".into())
            .spawn(|| {
                let lock = SystemWakeLock::acquire();
                assert!(lock.is_active(), "SetThreadExecutionState should succeed");
                drop(lock);
            })
            .expect("spawn test thread");
        handle.join().expect("thread finished cleanly");
    }
}
