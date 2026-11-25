//! Optional tuning of the microphone capture thread (experiment, off by default).
//!
//! Two knobs, both for the microphone source only: a fixed WASAPI event buffer duration
//! instead of the device default, and registration of the capture thread with the Multimedia
//! Class Scheduler Service (MMCSS) as a `Pro Audio` task. Every other source and the
//! default settings behave exactly as before.
//!
//! The tuning is read from `BBB_MIC_TUNING`, a comma separated list: `buffer100` (event
//! buffer of 100 ms, any value from 1 to 1000) and `mmcss`. Example:
//! `BBB_MIC_TUNING=buffer100,mmcss`.

/// Environment variable that switches the tuning on.
pub const ENV_VAR: &str = "BBB_MIC_TUNING";

/// Longest accepted buffer duration in ms.
const MAX_BUFFER_MS: u32 = 1000;

/// Capture tuning of the microphone track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MicTuning {
    /// Event buffer duration in ms; `None` keeps the device default.
    pub buffer_ms: Option<u32>,
    /// Register the capture thread with MMCSS as a `Pro Audio` task.
    pub mmcss: bool,
}

impl MicTuning {
    /// True when any knob differs from the default behaviour.
    pub fn is_active(&self) -> bool {
        self.buffer_ms.is_some() || self.mmcss
    }

    /// Buffer duration in 100 ns units, the unit WASAPI takes; `0` is the device default.
    pub fn buffer_hns(&self) -> i64 {
        self.buffer_ms.map_or(0, ms_to_hns)
    }

    /// Parse `buffer<N>` and `mmcss` tokens, comma separated, case-insensitive. An empty
    /// text is the default tuning.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut tuning = Self::default();
        for token in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let lower = token.to_ascii_lowercase();
            if lower == "mmcss" {
                tuning.mmcss = true;
            } else if let Some(digits) = lower.strip_prefix("buffer") {
                let ms: u32 = digits
                    .parse()
                    .map_err(|_| format!("`{token}`: the buffer duration is not a number"))?;
                if !(1..=MAX_BUFFER_MS).contains(&ms) {
                    return Err(format!(
                        "`{token}`: the buffer duration must be 1 to {MAX_BUFFER_MS} ms"
                    ));
                }
                tuning.buffer_ms = Some(ms);
            } else {
                return Err(format!(
                    "unknown option `{token}`, use `bufferN` or `mmcss`"
                ));
            }
        }
        Ok(tuning)
    }

    /// The tuning requested through [`ENV_VAR`]. A missing or invalid value gives the
    /// default tuning; an invalid value is logged once.
    pub fn from_env() -> Self {
        static CACHED: std::sync::OnceLock<MicTuning> = std::sync::OnceLock::new();
        *CACHED.get_or_init(|| match std::env::var(ENV_VAR) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "ignoring {ENV_VAR}");
                Self::default()
            }),
            Err(_) => Self::default(),
        })
    }

    /// Short text for logs and the journal, for example `buffer=100ms,mmcss`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ms) = self.buffer_ms {
            parts.push(format!("buffer={ms}ms"));
        }
        if self.mmcss {
            parts.push("mmcss".to_string());
        }
        if parts.is_empty() {
            "default".to_string()
        } else {
            parts.join(",")
        }
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
                tracing::warn!(error = %e, "MMCSS registration failed, capturing without it");
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
            tracing::debug!(error = %e, "MMCSS revert failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inactive_and_uses_the_device_buffer() {
        let t = MicTuning::default();
        assert!(!t.is_active());
        assert_eq!(t.buffer_hns(), 0);
        assert_eq!(t.describe(), "default");
    }

    #[test]
    fn parses_buffer_and_mmcss() {
        let t = MicTuning::parse("buffer100,mmcss").unwrap();
        assert_eq!(
            t,
            MicTuning {
                buffer_ms: Some(100),
                mmcss: true
            }
        );
        assert!(t.is_active());
        assert_eq!(t.buffer_hns(), 1_000_000);
        assert_eq!(t.describe(), "buffer=100ms,mmcss");
    }

    #[test]
    fn parsing_tolerates_case_spaces_and_empty_text() {
        assert_eq!(MicTuning::parse("").unwrap(), MicTuning::default());
        assert_eq!(
            MicTuning::parse(" MMCSS , Buffer20 ").unwrap(),
            MicTuning {
                buffer_ms: Some(20),
                mmcss: true
            }
        );
    }

    #[test]
    fn parsing_rejects_bad_tokens() {
        assert!(MicTuning::parse("buffer").is_err());
        assert!(MicTuning::parse("bufferx").is_err());
        assert!(MicTuning::parse("buffer0").is_err());
        assert!(MicTuning::parse("buffer1001").is_err());
        assert!(MicTuning::parse("turbo").is_err());
    }

    #[test]
    fn converts_ms_to_hns() {
        assert_eq!(ms_to_hns(1), 10_000);
        assert_eq!(ms_to_hns(100), 1_000_000);
    }
}
