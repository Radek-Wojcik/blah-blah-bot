//! Pinned binary and model downloads with HTTP Range resume, streaming SHA256 and exact-size
//! verification, and cooperative cancellation.
//!
//! Files are streamed into `<file_name>.part` inside the caller-supplied download folder under
//! Local AppData (never `%TEMP%`). Only after both the byte size and SHA256 match the pinned
//! values is the `.part` file atomically renamed to its final name. On checksum or size
//! mismatch the `.part` file inside the download folder is deleted and the call fails closed.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Streaming read/write chunk size (64 KiB).
const CHUNK_SIZE: usize = 64 * 1024;

/// A pinned remote artifact verified by exact byte size and SHA256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub url: &'static str,
    pub file_name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// pinned, not verified by an actual download; the app fails closed if the hash differs
pub const LLAMA_CPP_ZIP: Pin = Pin {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b12774/llama-b12774-bin-win-cuda-12.4-x64.zip",
    file_name: "llama-b12774-bin-win-cuda-12.4-x64.zip",
    size: 357_985_904,
    sha256: "7c2f8e99f7ffe244e5cba92e6c2b65f1d04fbabe8d4eb304d3fa200778ccf8f3",
};

/// pinned, not verified by an actual download; the app fails closed if the hash differs
pub const CUDART_ZIP: Pin = Pin {
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b12774/cudart-llama-bin-win-cuda-12.4-x64.zip",
    file_name: "cudart-llama-bin-win-cuda-12.4-x64.zip",
    size: 391_443_627,
    sha256: "8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6",
};

/// pinned, not verified by an actual download; the app fails closed if the hash differs
pub const GEMMA_12B_Q4_K_M: Pin = Pin {
    url: "https://huggingface.co/bartowski/gemma-4-12B-it-GGUF/resolve/2ae7d41be21ca62de00a2d320ee9cec50daa3aa6/gemma-4-12B-it-Q4_K_M.gguf",
    file_name: "gemma-4-12B-it-Q4_K_M.gguf",
    size: 7_662_533_088,
    sha256: "3962624dcd25b947d889dc9ae1bf275b61db6cd4dbe694057f34fffef1671509",
};

/// Phase of an active download reported to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPhase {
    Progress,
    Verifying,
}

/// Snapshot emitted during a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub phase: DownloadPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub elapsed_secs: u64,
}

/// Outcome error of [`download_pin`] and [`download_pin_from`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    Failed(String),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("download was cancelled"),
            Self::Failed(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for DownloadError {}

const CHECKSUM_MISMATCH_MSG: &str =
    "downloaded file does not match the expected checksum; the download was discarded and can be retried";

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Validates that `file_name` is a plain file name with no path traversal.
fn validate_file_name(file_name: &str) -> Result<(), DownloadError> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains(['/', '\\'])
    {
        return Err(DownloadError::Failed(
            "download target has an invalid file name".to_string(),
        ));
    }
    Ok(())
}

/// Extracts a safe host name from `url` (never includes credentials, path or query string) and
/// rejects non-HTTPS URLs except loopback HTTP for local tests.
fn validate_url_host(url: &str) -> Result<String, DownloadError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| DownloadError::Failed("download address is not a valid URL".to_string()))?;
    let scheme_lower = scheme.to_ascii_lowercase();
    if scheme_lower != "https" && scheme_lower != "http" {
        return Err(DownloadError::Failed(
            "download address must use https://".to_string(),
        ));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(DownloadError::Failed(
            "download address must not contain credentials".to_string(),
        ));
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(DownloadError::Failed(
            "download address has no host".to_string(),
        ));
    }
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if scheme_lower == "http" && !is_loopback {
        return Err(DownloadError::Failed(
            "plain http:// downloads are only allowed for loopback addresses".to_string(),
        ));
    }
    Ok(host.to_string())
}

fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .max_redirects(10)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

fn map_ureq_error(err: &ureq::Error, host: &str) -> DownloadError {
    let msg = match err {
        ureq::Error::Timeout(_) => format!("download from {host} timed out; you can retry"),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            format!("could not reach {host}; check your internet connection and retry")
        }
        ureq::Error::Tls(_) => {
            format!("secure connection to {host} failed; check your network or system clock")
        }
        ureq::Error::Io(_) => {
            format!("connection to {host} was interrupted; you can retry to resume")
        }
        _ => format!("download from {host} failed; you can retry"),
    };
    DownloadError::Failed(msg)
}

/// Deletes `target` only when it is a direct child of `download_dir`.
fn remove_file_in_dir(download_dir: &Path, target: &Path) {
    if target.parent() == Some(download_dir) {
        let _ = fs::remove_file(target);
    }
}

/// Streams an existing file into `hasher`, checking `cancel` between chunks.
fn hash_existing_file(
    path: &Path,
    hasher: &mut Sha256,
    cancel: &Arc<AtomicBool>,
) -> Result<u64, DownloadError> {
    let mut file = File::open(path)
        .map_err(|e| DownloadError::Failed(format!("could not read partial download file: {e}")))?;
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(DownloadError::Failed(format!(
                    "could not read partial download file: {e}"
                )));
            }
        };
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok(total)
}

/// Verifies whether `path` already has the exact size and SHA256 of `pin`.
fn file_matches_pin(
    path: &Path,
    pin: &Pin,
    cancel: &Arc<AtomicBool>,
) -> Result<bool, DownloadError> {
    let Ok(meta) = fs::metadata(path) else {
        return Ok(false);
    };
    if !meta.is_file() || meta.len() != pin.size {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let len = hash_existing_file(path, &mut hasher, cancel)?;
    if len != pin.size {
        return Ok(false);
    }
    Ok(hex_lower(&hasher.finalize()) == pin.sha256.to_ascii_lowercase())
}

/// Downloads `pin` from its pinned URL into `download_dir` (under Local AppData).
pub fn download_pin(
    pin: &Pin,
    download_dir: &Path,
    cancel: &Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, DownloadError> {
    download_pin_from(pin.url, pin, download_dir, cancel, on_progress)
}

/// Downloads `pin` from `url` into `download_dir`, resuming from `<file_name>.part` when
/// possible, verifying exact byte size and SHA256, and renaming atomically on success.
pub fn download_pin_from(
    url: &str,
    pin: &Pin,
    download_dir: &Path,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, DownloadError> {
    validate_file_name(pin.file_name)?;
    let host = validate_url_host(url)?;

    if cancel.load(Ordering::SeqCst) {
        return Err(DownloadError::Cancelled);
    }

    fs::create_dir_all(download_dir)
        .map_err(|e| DownloadError::Failed(format!("could not create the download folder: {e}")))?;

    let final_path = download_dir.join(pin.file_name);
    let part_path = download_dir.join(format!("{}.part", pin.file_name));
    let started = Instant::now();

    if file_matches_pin(&final_path, pin, cancel)? {
        on_progress(DownloadProgress {
            phase: DownloadPhase::Progress,
            downloaded_bytes: pin.size,
            total_bytes: Some(pin.size),
            elapsed_secs: started.elapsed().as_secs(),
        });
        return Ok(final_path);
    }

    let mut existing_len = match fs::metadata(&part_path) {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => 0,
    };

    // A partial file larger than the expected size cannot be resumed.
    if existing_len > pin.size {
        remove_file_in_dir(download_dir, &part_path);
        existing_len = 0;
    }

    // If the partial file already reached the full size, verify it directly without a request.
    if existing_len == pin.size && pin.size > 0 {
        on_progress(DownloadProgress {
            phase: DownloadPhase::Verifying,
            downloaded_bytes: existing_len,
            total_bytes: Some(pin.size),
            elapsed_secs: started.elapsed().as_secs(),
        });
        let mut hasher = Sha256::new();
        let len = hash_existing_file(&part_path, &mut hasher, cancel)?;
        return finalize_download(download_dir, &part_path, &final_path, pin, len, hasher);
    }

    let agent = download_agent();
    let mut request = agent.get(url).header("User-Agent", "blah-blah-bot");
    if existing_len > 0 {
        request = request.header("Range", &format!("bytes={existing_len}-"));
    }

    let response = request.call().map_err(|e| map_ureq_error(&e, &host))?;
    let status = response.status().as_u16();

    let (mut file, mut hasher, mut downloaded) = match status {
        206 if existing_len > 0 => {
            let mut h = Sha256::new();
            let hashed = hash_existing_file(&part_path, &mut h, cancel)?;
            let f = OpenOptions::new()
                .append(true)
                .open(&part_path)
                .map_err(|e| {
                    DownloadError::Failed(format!("could not open partial download file: {e}"))
                })?;
            (f, h, hashed)
        }
        200 | 206 => {
            let f = File::create(&part_path).map_err(|e| {
                DownloadError::Failed(format!("could not create partial download file: {e}"))
            })?;
            (f, Sha256::new(), 0u64)
        }
        416 if existing_len > 0 => {
            remove_file_in_dir(download_dir, &part_path);
            return Err(DownloadError::Failed(
                "partial download could not be resumed and was cleared; please retry".to_string(),
            ));
        }
        _ => {
            return Err(DownloadError::Failed(format!(
                "download from {host} failed (HTTP {status})"
            )));
        }
    };

    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = file.flush();
            return Err(DownloadError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                let _ = file.flush();
                return Err(DownloadError::Failed(format!(
                    "download from {host} was interrupted; you can retry to resume"
                )));
            }
        };
        file.write_all(&buf[..n]).map_err(|e| {
            DownloadError::Failed(format!("could not write to the download folder: {e}"))
        })?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;

        if downloaded > pin.size {
            let _ = file.flush();
            drop(file);
            remove_file_in_dir(download_dir, &part_path);
            return Err(DownloadError::Failed(format!(
                "downloaded file exceeded the expected size ({} bytes); the download was discarded and can be retried",
                pin.size
            )));
        }

        on_progress(DownloadProgress {
            phase: DownloadPhase::Progress,
            downloaded_bytes: downloaded,
            total_bytes: Some(pin.size),
            elapsed_secs: started.elapsed().as_secs(),
        });
    }

    file.flush().map_err(|e| {
        DownloadError::Failed(format!("could not finish writing the download file: {e}"))
    })?;
    drop(file);

    if cancel.load(Ordering::SeqCst) {
        return Err(DownloadError::Cancelled);
    }

    on_progress(DownloadProgress {
        phase: DownloadPhase::Verifying,
        downloaded_bytes: downloaded,
        total_bytes: Some(pin.size),
        elapsed_secs: started.elapsed().as_secs(),
    });

    finalize_download(
        download_dir,
        &part_path,
        &final_path,
        pin,
        downloaded,
        hasher,
    )
}

fn finalize_download(
    download_dir: &Path,
    part_path: &Path,
    final_path: &Path,
    pin: &Pin,
    downloaded: u64,
    hasher: Sha256,
) -> Result<PathBuf, DownloadError> {
    if downloaded != pin.size {
        remove_file_in_dir(download_dir, part_path);
        return Err(DownloadError::Failed(format!(
            "downloaded file size ({downloaded} bytes) does not match the expected size ({} bytes); the download was discarded and can be retried",
            pin.size
        )));
    }
    let actual_hex = hex_lower(&hasher.finalize());
    if actual_hex != pin.sha256.to_ascii_lowercase() {
        remove_file_in_dir(download_dir, part_path);
        return Err(DownloadError::Failed(CHECKSUM_MISMATCH_MSG.to_string()));
    }
    if final_path.exists() {
        remove_file_in_dir(download_dir, final_path);
    }
    fs::rename(part_path, final_path).map_err(|e| {
        DownloadError::Failed(format!("could not save the verified download file: {e}"))
    })?;
    Ok(final_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use test_tmp::TestDir;

    #[derive(Clone, Copy)]
    enum RangeMode {
        Support206,
        Ignore200,
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex_lower(&h.finalize())
    }

    fn spawn_file_server(
        payload: Vec<u8>,
        mode: RangeMode,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut range_start: Option<usize> = None;
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line) {
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("range") {
                        let v = v.trim();
                        if let Some(rest) = v.strip_prefix("bytes=") {
                            if let Some((start_s, _)) = rest.split_once('-') {
                                range_start = start_s.parse::<usize>().ok();
                            }
                        }
                    }
                }
                line.clear();
            }

            let total = payload.len();
            match (mode, range_start) {
                (RangeMode::Support206, Some(start)) if start < total => {
                    let slice = &payload[start..];
                    let end = total - 1;
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(slice);
                }
                _ => {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&payload);
                }
            }
            let _ = stream.flush();
        });
        (format!("http://{addr}/artifact.bin"), handle)
    }

    fn make_payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + 17) % 251) as u8).collect()
    }

    fn leak_str(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    fn pin_for(name: &'static str, payload: &[u8]) -> Pin {
        Pin {
            url: "https://example.invalid/unused",
            file_name: name,
            size: payload.len() as u64,
            sha256: leak_str(sha256_hex(payload)),
        }
    }

    #[test]
    fn download_fresh_success() {
        let dir = TestDir::new("dl-fresh");
        let payload = make_payload(10_000);
        let pin = pin_for("fresh.bin", &payload);
        let (url, server) = spawn_file_server(payload.clone(), RangeMode::Support206);

        let cancel = Arc::new(AtomicBool::new(false));
        let mut phases = Vec::new();
        let out = download_pin_from(&url, &pin, dir.path(), &cancel, |p| phases.push(p.phase))
            .expect("fresh download should succeed");
        server.join().expect("server thread");

        assert_eq!(out, dir.path().join("fresh.bin"));
        assert_eq!(fs::read(&out).unwrap(), payload);
        assert!(!dir.path().join("fresh.bin.part").exists());
        assert!(phases.contains(&DownloadPhase::Progress));
        assert!(phases.contains(&DownloadPhase::Verifying));
    }

    #[test]
    fn download_resumes_from_partial_with_206() {
        let dir = TestDir::new("dl-resume-206");
        let payload = make_payload(12_000);
        let pin = pin_for("resume.bin", &payload);
        let part = dir.path().join("resume.bin.part");
        fs::write(&part, &payload[..5_000]).unwrap();

        let (url, server) = spawn_file_server(payload.clone(), RangeMode::Support206);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut max_seen = 0u64;
        let out = download_pin_from(&url, &pin, dir.path(), &cancel, |p| {
            max_seen = max_seen.max(p.downloaded_bytes);
        })
        .expect("resumed download should succeed");
        server.join().expect("server thread");

        assert_eq!(out, dir.path().join("resume.bin"));
        assert_eq!(fs::read(&out).unwrap(), payload);
        assert!(!part.exists());
        assert_eq!(max_seen, payload.len() as u64);
    }

    #[test]
    fn download_restarts_when_server_ignores_range_with_200() {
        let dir = TestDir::new("dl-restart-200");
        let payload = make_payload(9_000);
        let pin = pin_for("restart.bin", &payload);
        let part = dir.path().join("restart.bin.part");
        // Write stale partial bytes; server returns 200 so downloader must restart from 0.
        fs::write(&part, vec![0xAA; 3_000]).unwrap();

        let (url, server) = spawn_file_server(payload.clone(), RangeMode::Ignore200);
        let cancel = Arc::new(AtomicBool::new(false));
        let out = download_pin_from(&url, &pin, dir.path(), &cancel, |_| {})
            .expect("restart download should succeed");
        server.join().expect("server thread");

        assert_eq!(out, dir.path().join("restart.bin"));
        assert_eq!(fs::read(&out).unwrap(), payload);
        assert!(!part.exists());
    }

    #[test]
    fn download_checksum_mismatch_deletes_part_and_fails_closed() {
        let dir = TestDir::new("dl-bad-sha");
        let expected = make_payload(8_192);
        let mut corrupted = expected.clone();
        corrupted[100] ^= 0xFF;
        let pin = pin_for("badsha.bin", &expected);

        let (url, server) = spawn_file_server(corrupted, RangeMode::Support206);
        let cancel = Arc::new(AtomicBool::new(false));
        let err = download_pin_from(&url, &pin, dir.path(), &cancel, |_| {})
            .expect_err("checksum mismatch must fail");
        server.join().expect("server thread");

        assert_eq!(
            err,
            DownloadError::Failed(CHECKSUM_MISMATCH_MSG.to_string())
        );
        assert!(!dir.path().join("badsha.bin").exists());
        assert!(!dir.path().join("badsha.bin.part").exists());
    }

    #[test]
    fn download_cancel_midway_keeps_part_for_resume() {
        let dir = TestDir::new("dl-cancel");
        // Multiple 64 KiB chunks so cancelling after the first chunk leaves a partial .part file.
        let payload = make_payload(CHUNK_SIZE * 4);
        let pin = pin_for("cancel.bin", &payload);

        let (url, server) = spawn_file_server(payload.clone(), RangeMode::Support206);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_cb = cancel.clone();
        let err = download_pin_from(&url, &pin, dir.path(), &cancel, move |p| {
            if p.downloaded_bytes > 0 {
                cancel_cb.store(true, Ordering::SeqCst);
            }
        })
        .expect_err("cancelled download must return Cancelled");
        let _ = server.join();

        assert_eq!(err, DownloadError::Cancelled);
        let part = dir.path().join("cancel.bin.part");
        assert!(part.is_file(), ".part must remain for resume");
        let part_len = fs::metadata(&part).unwrap().len();
        assert!(
            part_len > 0 && part_len < payload.len() as u64,
            "{part_len}"
        );
        assert!(!dir.path().join("cancel.bin").exists());
    }

    #[test]
    fn download_size_mismatch_deletes_part_and_fails_closed() {
        let dir = TestDir::new("dl-bad-size");
        let expected = make_payload(8_000);
        let truncated = expected[..4_000].to_vec();
        let pin = pin_for("badsize.bin", &expected);

        let (url, server) = spawn_file_server(truncated, RangeMode::Support206);
        let cancel = Arc::new(AtomicBool::new(false));
        let err = download_pin_from(&url, &pin, dir.path(), &cancel, |_| {})
            .expect_err("truncated file must fail");
        server.join().expect("server thread");

        match err {
            DownloadError::Failed(msg) => assert!(msg.contains("expected size"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!dir.path().join("badsize.bin").exists());
        assert!(!dir.path().join("badsize.bin.part").exists());
    }
}
