//! Test signal player that renders to a *named* output device.
//!
//! The tests need sound sources in separate processes. Playing them on the default device
//! is audible, so this player refuses the default device: a device name is mandatory
//! (for example a virtual cable that nobody monitors).

use std::time::{Duration, Instant};

use wasapi::{
    initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WasapiError, WaveFormat,
};

/// What to play.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// Continuous sine wave of the given frequency.
    Sine(f32),
    /// A short 2 kHz burst once per second, half a second into every second.
    Click,
}

const RATE: usize = 48_000;

/// One mono sample of the signal at frame index `n`.
pub fn sample_at(signal: Signal, n: u64, amplitude: f32) -> f32 {
    match signal {
        Signal::Sine(freq) => {
            let phase = (n as f64 * f64::from(freq) / RATE as f64).fract();
            amplitude * (phase * std::f64::consts::TAU).sin() as f32
        }
        Signal::Click => {
            let burst = RATE / 100; // 10 ms
            let pos = (n % RATE as u64) as usize;
            let start = RATE / 2;
            if pos < start || pos >= start + burst {
                return 0.0;
            }
            let k = (pos - start) as f64;
            let hann = 0.5 - 0.5 * (std::f64::consts::TAU * k / burst as f64).cos();
            let carrier = (std::f64::consts::TAU * 2000.0 * k / RATE as f64).sin();
            amplitude * (hann * carrier) as f32
        }
    }
}

/// Names of all active render devices.
pub fn list_render_devices() -> Result<Vec<String>, String> {
    let _ = initialize_mta();
    let devices = DeviceEnumerator::new()
        .and_then(|e| e.get_device_collection(Direction::Render))
        .map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for d in &devices {
        names.push(
            d.and_then(|d| d.get_friendlyname())
                .map_err(|e| e.to_string())?,
        );
    }
    Ok(names)
}

/// Play `signal` on the render device whose name contains `device` (case-insensitive) for
/// `seconds`.
pub fn play(device: &str, signal: Signal, amplitude: f32, seconds: f64) -> Result<(), String> {
    let _ = initialize_mta();
    let wanted = device.to_lowercase();
    let devices = DeviceEnumerator::new()
        .and_then(|e| e.get_device_collection(Direction::Render))
        .map_err(|e| e.to_string())?;
    let mut found = None;
    for d in &devices {
        let d = d.map_err(|e| e.to_string())?;
        if d.get_friendlyname()
            .is_ok_and(|n| n.to_lowercase().contains(&wanted))
        {
            found = Some(d);
            break;
        }
    }
    let device = found.ok_or_else(|| format!("no active render device matches `{device}`"))?;
    let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let format = WaveFormat::new(32, 32, SampleType::Float, RATE, 2, None);
    client
        .initialize_client(
            &format,
            Direction::Render,
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 0,
            },
            None,
        )
        .map_err(|e| e.to_string())?;
    let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
    let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
    client.start_stream().map_err(|e| e.to_string())?;

    let started = Instant::now();
    let mut frame = 0u64;
    let end = Duration::from_secs_f64(seconds);
    while started.elapsed() < end {
        match event.wait_for_event(500) {
            Ok(()) => {}
            Err(WasapiError::EventTimeout) => continue,
            Err(e) => return Err(e.to_string()),
        }
        let free = client
            .get_available_space_in_frames()
            .map_err(|e| e.to_string())? as usize;
        let mut bytes = Vec::with_capacity(free * 8);
        for _ in 0..free {
            let s = sample_at(signal, frame, amplitude).to_le_bytes();
            bytes.extend_from_slice(&s);
            bytes.extend_from_slice(&s);
            frame += 1;
        }
        render
            .write_to_device(&bytes, None)
            .map_err(|e| e.to_string())?;
    }
    let _ = client.stop_stream();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_has_expected_period() {
        // 1 kHz at 48 kHz: 48 samples per period.
        let a = sample_at(Signal::Sine(1000.0), 0, 0.5);
        let b = sample_at(Signal::Sine(1000.0), 48, 0.5);
        assert!(a.abs() < 1e-6 && b.abs() < 1e-4);
        let peak = sample_at(Signal::Sine(1000.0), 12, 0.5);
        assert!((peak - 0.5).abs() < 1e-4);
    }

    #[test]
    fn click_is_silent_outside_the_burst() {
        assert_eq!(sample_at(Signal::Click, 0, 0.5), 0.0);
        assert_eq!(sample_at(Signal::Click, 24_000 + 480, 0.5), 0.0);
        let inside = (24_000..24_480)
            .map(|n| sample_at(Signal::Click, n, 0.5).abs())
            .fold(0.0, f32::max);
        assert!(inside > 0.1);
        // The pattern repeats every second.
        assert_eq!(
            sample_at(Signal::Click, 24_100, 0.5),
            sample_at(Signal::Click, 48_000 + 24_100, 0.5)
        );
    }
}
