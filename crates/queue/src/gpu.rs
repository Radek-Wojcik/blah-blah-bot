//! GPU guard: decides from NVML samples whether a stage may start.
//!
//! The decision logic ([`Guard`]) is pure and works on [`GpuSample`] values, so it is tested
//! on recorded traces. [`NvmlSampler`] produces the samples on a real machine.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::stage::Stage;
use crate::supervisor::{GateDecision, GpuGate};

const MIB: u64 = 1024 * 1024;

/// A process holding GPU memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuProcess {
    pub pid: u32,
    /// `None` when the driver does not report the amount (common on Windows).
    pub used_mib: Option<u64>,
}

/// One reading of the GPU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuSample {
    /// Time of the reading, milliseconds on any monotonic clock.
    pub t_ms: u64,
    pub util_percent: u32,
    pub free_mib: u64,
    pub total_mib: u64,
    pub processes: Vec<GpuProcess>,
}

/// What to do when no reading is available (no NVIDIA driver, NVML failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnUnavailable {
    /// Wait and show the reason. Processing without an NVIDIA GPU is not supported.
    Block,
    /// Start anyway; the user's setup is trusted (for example an external server).
    Allow,
}

/// Configurable thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardPolicy {
    /// The GPU must stay below `max_util_percent` for this long before a stage starts.
    pub idle_ms: u64,
    pub max_util_percent: u32,
    /// Free memory a stage needs, in MiB: the measured peak plus a margin.
    pub asr_mib: u64,
    pub diarize_mib: u64,
    pub vision_mib: u64,
    /// Free memory the managed language model needs for `extract` and `reduce`, in MiB.
    pub llm_mib: u64,
    pub on_unavailable: OnUnavailable,
}

impl Default for GuardPolicy {
    fn default() -> Self {
        Self {
            idle_ms: 10_000,
            max_util_percent: 40,
            // Stage 0 peaks above baseline: 1.5 GB, 2.3 GB, 7.7 GB. Margin about 50 %.
            asr_mib: 3_000,
            diarize_mib: 3_500,
            vision_mib: 9_500,
            // Same budget as vision: the default model is of the same size class.
            llm_mib: 9_500,
            on_unavailable: OnUnavailable::Block,
        }
    }
}

impl GuardPolicy {
    /// Free memory the stage needs, in MiB.
    pub fn required_mib(&self, stage: Stage) -> u64 {
        match stage {
            Stage::Asr => self.asr_mib,
            Stage::Diarize => self.diarize_mib,
            Stage::Vision => self.vision_mib,
            Stage::Extract | Stage::Reduce => self.llm_mib,
            Stage::Merge | Stage::SpeakerId | Stage::Clips | Stage::AudioCleanup => 0,
        }
    }
}

/// Remembers since when the GPU has been idle.
#[derive(Debug, Clone)]
pub struct Guard {
    policy: GuardPolicy,
    idle_since: Option<u64>,
}

impl Guard {
    pub fn new(policy: GuardPolicy) -> Self {
        Self {
            policy,
            idle_since: None,
        }
    }

    pub fn policy(&self) -> &GuardPolicy {
        &self.policy
    }

    /// Evaluate one reading. Call it repeatedly while a job waits: the idle period is
    /// measured across readings.
    pub fn evaluate(&mut self, sample: Option<&GpuSample>, stage: Stage) -> GateDecision {
        let Some(s) = sample else {
            self.idle_since = None;
            return match self.policy.on_unavailable {
                OnUnavailable::Allow => GateDecision::Go,
                OnUnavailable::Block => {
                    GateDecision::Wait("No NVIDIA GPU readings are available".into())
                }
            };
        };

        if s.util_percent > self.policy.max_util_percent {
            self.idle_since = None;
            return GateDecision::Wait(format!(
                "GPU is busy ({}% utilization, limit {}%)",
                s.util_percent, self.policy.max_util_percent
            ));
        }
        let since = *self.idle_since.get_or_insert(s.t_ms);

        let need = self.policy.required_mib(stage);
        if s.free_mib < need {
            return GateDecision::Wait(format!(
                "Not enough free GPU memory ({:.1} GB free, {:.1} GB needed)",
                s.free_mib as f64 / 1024.0,
                need as f64 / 1024.0
            ));
        }

        let idle = s.t_ms.saturating_sub(since);
        if idle < self.policy.idle_ms {
            return GateDecision::Countdown(format!(
                "Waiting for the GPU to stay idle ({} of {} s)",
                idle / 1000,
                self.policy.idle_ms / 1000
            ));
        }
        GateDecision::Go
    }
}

/// Source of readings.
pub trait GpuSampler: Send {
    /// A reading now, or `None` when NVML cannot be used.
    fn sample(&mut self) -> Option<GpuSample>;
}

/// Readings from NVML for the first GPU.
pub struct NvmlSampler {
    nvml: Option<nvml_wrapper::Nvml>,
    started: std::time::Instant,
}

impl NvmlSampler {
    /// Never fails: without a driver every reading is `None`.
    pub fn new() -> Self {
        let nvml = match nvml_wrapper::Nvml::init() {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("NVML is not available: {e}");
                None
            }
        };
        Self {
            nvml,
            started: std::time::Instant::now(),
        }
    }
}

impl Default for NvmlSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuSampler for NvmlSampler {
    fn sample(&mut self) -> Option<GpuSample> {
        let nvml = self.nvml.as_ref()?;
        let dev = nvml.device_by_index(0).ok()?;
        let util = dev.utilization_rates().ok()?;
        let mem = dev.memory_info().ok()?;
        let mut processes = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let compute = dev.running_compute_processes().unwrap_or_default();
        let graphics = dev.running_graphics_processes().unwrap_or_default();
        for p in compute.into_iter().chain(graphics) {
            if !seen.insert(p.pid) {
                continue;
            }
            let used_mib = match p.used_gpu_memory {
                nvml_wrapper::enums::device::UsedGpuMemory::Used(b) => Some(b / MIB),
                nvml_wrapper::enums::device::UsedGpuMemory::Unavailable => None,
            };
            processes.push(GpuProcess {
                pid: p.pid,
                used_mib,
            });
        }
        Some(GpuSample {
            t_ms: self.started.elapsed().as_millis() as u64,
            util_percent: util.gpu,
            free_mib: mem.free / MIB,
            total_mib: mem.total / MIB,
            processes,
        })
    }
}

/// The gate used by the supervisor: samples, then evaluates.
pub struct NvmlGate<S: GpuSampler> {
    inner: Mutex<(S, Guard)>,
}

impl<S: GpuSampler> NvmlGate<S> {
    pub fn new(sampler: S, policy: GuardPolicy) -> Self {
        Self {
            inner: Mutex::new((sampler, Guard::new(policy))),
        }
    }
}

impl<S: GpuSampler> GpuGate for NvmlGate<S> {
    fn check(&self, stage: Stage) -> GateDecision {
        let Ok(mut g) = self.inner.lock() else {
            return GateDecision::Wait("internal error in the GPU guard".into());
        };
        let (sampler, guard) = &mut *g;
        let sample = sampler.sample();
        guard.evaluate(sample.as_ref(), stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Trace {
        samples: Vec<GpuSample>,
    }

    fn trace(name: &str) -> Trace {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("traces")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Index of the first reading at which the guard says go, if any.
    fn first_go(t: &Trace, stage: Stage) -> Option<usize> {
        let mut g = Guard::new(GuardPolicy::default());
        t.samples
            .iter()
            .position(|s| g.evaluate(Some(s), stage) == GateDecision::Go)
    }

    #[test]
    fn idle_gpu_starts_after_the_idle_period() {
        let t = trace("idle.json");
        let i = first_go(&t, Stage::Asr).expect("starts");
        assert!(t.samples[i].t_ms - t.samples[0].t_ms >= 10_000);
        assert!(i > 0, "never starts on the very first reading");
    }

    #[test]
    fn idle_wait_counter_advances_each_second() {
        let mut g = Guard::new(GuardPolicy {
            idle_ms: 5_000,
            ..GuardPolicy::default()
        });
        for sec in 0..5u64 {
            let sample = GpuSample {
                t_ms: sec * 1000,
                util_percent: 10,
                free_mib: 10_000,
                total_mib: 12_000,
                processes: Vec::new(),
            };
            assert_eq!(
                g.evaluate(Some(&sample), Stage::Asr),
                GateDecision::Countdown(format!("Waiting for the GPU to stay idle ({sec} of 5 s)"))
            );
        }
        let ready = GpuSample {
            t_ms: 5_000,
            util_percent: 10,
            free_mib: 10_000,
            total_mib: 12_000,
            processes: Vec::new(),
        };
        assert_eq!(g.evaluate(Some(&ready), Stage::Asr), GateDecision::Go);
    }

    #[test]
    fn recorded_desktop_load_counts_as_idle() {
        // A real reading of the reference machine with only the desktop running: the GPU
        // hovers around 25 % utilization, which must not keep processing waiting forever.
        let t = trace("idle-recorded.json");
        assert!(first_go(&t, Stage::Asr).is_some());
    }

    #[test]
    fn busy_gpu_never_starts() {
        assert_eq!(first_go(&trace("busy.json"), Stage::Asr), None);
    }

    #[test]
    fn game_that_ends_resets_the_idle_clock() {
        let t = trace("busy-then-free.json");
        let i = first_go(&t, Stage::Asr).expect("starts once the load ends");
        // The load ends at the first low reading; the idle period counts from there.
        let end = t.samples.iter().position(|s| s.util_percent <= 40).unwrap();
        assert!(t.samples[i].t_ms - t.samples[end].t_ms >= 10_000);
    }

    #[test]
    fn short_dip_does_not_count_as_idle() {
        // Idle for 4 s, then a spike, then idle again: the clock restarts at the spike.
        let t = trace("flapping.json");
        let i = first_go(&t, Stage::Asr).expect("starts eventually");
        let spike = t.samples.iter().position(|s| s.util_percent > 40).unwrap();
        let after = spike + 1;
        assert!(t.samples[i].t_ms - t.samples[after].t_ms >= 10_000);
    }

    #[test]
    fn vision_needs_more_memory_than_asr() {
        let t = trace("memory-held.json");
        assert!(first_go(&t, Stage::Asr).is_some());
        assert_eq!(first_go(&t, Stage::Vision), None);
    }

    #[test]
    fn the_language_model_stages_need_as_much_memory_as_vision() {
        let p = GuardPolicy::default();
        assert_eq!(p.required_mib(Stage::Extract), p.vision_mib);
        assert_eq!(p.required_mib(Stage::Reduce), 9_500);
        let t = trace("memory-held.json");
        assert_eq!(first_go(&t, Stage::Extract), None);
        assert_eq!(first_go(&t, Stage::Reduce), None);
    }

    #[test]
    fn stages_without_a_model_never_wait_for_memory() {
        let t = trace("memory-held.json");
        assert!(first_go(&t, Stage::Merge).is_some());
    }

    #[test]
    fn missing_readings_block_or_allow_by_policy() {
        let mut block = Guard::new(GuardPolicy::default());
        assert!(matches!(
            block.evaluate(None, Stage::Asr),
            GateDecision::Wait(_)
        ));
        let mut allow = Guard::new(GuardPolicy {
            on_unavailable: OnUnavailable::Allow,
            ..GuardPolicy::default()
        });
        assert_eq!(allow.evaluate(None, Stage::Asr), GateDecision::Go);
    }

    #[test]
    fn reasons_are_readable() {
        let t = trace("busy.json");
        let mut g = Guard::new(GuardPolicy::default());
        match g.evaluate(Some(&t.samples[0]), Stage::Asr) {
            GateDecision::Wait(r) => assert!(r.contains("busy"), "{r}"),
            other => panic!("{other:?}"),
        }
    }
}
