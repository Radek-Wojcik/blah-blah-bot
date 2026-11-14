//! Small operating-system helpers: process CPU time, memory, the performance counter, OS version.

use std::sync::OnceLock;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, GetProcessTimes};
use windows::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};

fn filetime_100ns(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// CPU time (user plus kernel) used by this process so far, in seconds.
pub fn process_cpu_seconds() -> f64 {
    let (mut c, mut e, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo handle of the current process is always valid and the out
    // pointers refer to live locals.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    if ok.is_err() {
        return 0.0;
    }
    (filetime_100ns(k) + filetime_100ns(u)) as f64 / 1e7
}

/// CPU cycles used by this process so far. Unlike `GetProcessTimes`, which is sampled by the
/// scheduler tick and misses threads that wake for a few microseconds every 10 ms, this
/// counter is exact.
pub fn process_cycles() -> u64 {
    let mut c = 0u64;
    // SAFETY: the pseudo handle is always valid and the out pointer refers to a live local.
    let _ = unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut c) };
    c
}

/// Cycles per second of a busy core, measured by spinning this thread for 200 ms. Converts
/// cycle counts to time; at a lower clock the real CPU share is somewhat higher, so the
/// result is an estimate rather than a bound.
pub fn calibrate_cycles_per_second() -> f64 {
    let mut start = 0u64;
    // SAFETY: the pseudo handle is always valid and the out pointer refers to a live local.
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut start) };
    let t0 = std::time::Instant::now();
    let mut x = 1u64;
    while t0.elapsed().as_millis() < 200 {
        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
    let wall = t0.elapsed().as_secs_f64();
    let mut end = 0u64;
    // SAFETY: as above.
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut end) };
    (end.saturating_sub(start)) as f64 / wall
}

/// Working set of this process: (current, peak) in bytes.
pub fn working_set_bytes() -> (u64, u64) {
    let mut pmc = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: `pmc` is a live local of the size passed in `cb`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) };
    if ok.as_bool() {
        (pmc.WorkingSetSize as u64, pmc.PeakWorkingSetSize as u64)
    } else {
        (0, 0)
    }
}

fn qpc_frequency() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: out pointer refers to a live local.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    })
}

/// Current value of the performance counter in 100 ns units. This is the time base of
/// the timestamps WASAPI attaches to captured packets.
pub fn qpc_now_hns() -> u64 {
    let mut c = 0i64;
    // SAFETY: out pointer refers to a live local.
    let _ = unsafe { QueryPerformanceCounter(&mut c) };
    (i128::from(c) * 10_000_000 / i128::from(qpc_frequency())) as u64
}

/// Number of logical processors.
pub fn logical_cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Operating system description.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OsInfo {
    /// Product name.
    pub name: String,
    /// Marketing version.
    pub version: String,
    /// Build (kernel) version.
    pub build: String,
    /// Long description.
    pub long_version: String,
}

/// Describe the operating system.
pub fn os_info() -> OsInfo {
    OsInfo {
        name: sysinfo::System::name().unwrap_or_default(),
        version: sysinfo::System::os_version().unwrap_or_default(),
        build: sysinfo::System::kernel_version().unwrap_or_default(),
        long_version: sysinfo::System::long_os_version().unwrap_or_default(),
    }
}

/// Milliseconds since the Unix epoch.
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
