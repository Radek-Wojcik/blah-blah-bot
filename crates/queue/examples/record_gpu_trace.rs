//! Records GPU readings to a JSON trace: `cargo run -p queue --example record_gpu_trace -- <seconds> <out.json>`.
//! Process ids are replaced by their order so traces carry nothing machine specific.

use queue::gpu::{GpuProcess, GpuSampler, NvmlSampler};

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let out = args.next().unwrap_or_else(|| "trace.json".into());
    let mut sampler = NvmlSampler::new();
    let mut samples = Vec::new();
    for _ in 0..=seconds / 2 {
        match sampler.sample() {
            Some(mut s) => {
                for (i, p) in s.processes.iter_mut().enumerate() {
                    *p = GpuProcess {
                        pid: i as u32 + 1,
                        used_mib: p.used_mib,
                    };
                }
                samples.push(s);
            }
            None => {
                eprintln!("no NVML readings");
                std::process::exit(1);
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    let json = serde_json::json!({ "samples": samples });
    std::fs::write(&out, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    eprintln!("wrote {} readings to {out}", samples.len());
}
