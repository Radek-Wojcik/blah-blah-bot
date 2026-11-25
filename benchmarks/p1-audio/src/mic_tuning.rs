//! Optional tuning of the microphone capture thread (experiment, off by default; same logic
//! as `crates/recorder/src/mic_tuning.rs`, set by `--mic-buffer-ms` and `--mic-mmcss`).
//!
//! Two knobs, both for the microphone source only: a fixed WASAPI event buffer duration
//! instead of the device default, and registration of the capture thread with the Multimedia
//! Class Scheduler Service (MMCSS) as a `Pro Audio` task. Every other source and the
//! default settings behave exactly as before.
//!

/// Longest accepted buffer duration in ms.
const MAX_BUFFER_MS: u32 = 1000;

/// Capture tuning of the microphone track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct MicTuning {
    /// Event buffer duration in ms; `None` keeps the device default.
    pub buffer_ms: Option<u32>,
    /// Register the capture thread with MMCSS as a `Pro Audio` task.
    pub mmcss: bool,
}

impl MicTuning {
    /// Buffer duration in 100 ns units, the unit WASAPI takes; `0` is the device default.
    pub fn buffer_hns(&self) -> i64 {
        self.buffer_ms.map_or(0, ms_to_hns)
    }

    /// Build from the command line flags, rejecting a buffer duration out of range.
    pub fn from_flags(buffer_ms: Option<u32>, mmcss: bool) -> Result<Self, String> {
        if let Some(ms) = buffer_ms {
            if !(1..=MAX_BUFFER_MS).contains(&ms) {
                return Err(format!("--mic-buffer-ms must be 1 to {MAX_BUFFER_MS}"));
            }
        }
        Ok(Self { buffer_ms, mmcss })
    }
}

/// Milliseconds to 100 ns units.
pub fn ms_to_hns(ms: u32) -> i64 {
    i64::from(ms) * 10_000
}

/// MMCSS registration of the current thread; reverted when dropped.
#[cfg(windows)]
pub struct MmcssGuard {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl MmcssGuard {
    /// Register the calling thread as a `Pro Audio` task. `None` (with a logged warning)
    /// when Windows refuses; the capture then simply runs without it.
    pub fn enter() -> Option<Self> {
        let mut task_index = 0u32;
        // SAFETY: the task name is a valid wide string literal and `task_index` outlives the call.
        let result = unsafe {
            windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(
                windows::core::w!("Pro Audio"),
                &mut task_index,
            )
        };
        match result {
            Ok(handle) => Some(Self { handle }),
            Err(e) => {
                eprintln!("warning: MMCSS registration failed ({e}), capturing without it");
                None
            }
        }
    }
}

#[cfg(windows)]
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `AvSetMmThreadCharacteristicsW` call on
        // this thread and is reverted exactly once.
        if let Err(e) = unsafe {
            windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.handle)
        } {
            let _ = e;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_the_device_buffer() {
        let t = MicTuning::default();
        assert_eq!(t.buffer_hns(), 0);
        assert!(!t.mmcss);
    }

    #[test]
    fn buffer_flag_converts_to_hns() {
        let t = MicTuning::from_flags(Some(100), true).unwrap();
        assert_eq!(t.buffer_hns(), 1_000_000);
        assert!(t.mmcss);
    }

    #[test]
    fn flags_validate_the_buffer_range() {
        assert!(MicTuning::from_flags(Some(100), true).is_ok());
        assert!(MicTuning::from_flags(Some(0), false).is_err());
        assert!(MicTuning::from_flags(Some(5000), false).is_err());
        assert_eq!(
            MicTuning::from_flags(None, false).unwrap(),
            MicTuning::default()
        );
    }

    #[test]
    fn converts_ms_to_hns() {
        assert_eq!(ms_to_hns(1), 10_000);
        assert_eq!(ms_to_hns(100), 1_000_000);
    }
}
