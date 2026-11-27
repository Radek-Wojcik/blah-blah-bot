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
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Streaming read/write chunk size (64 KiB).
const CHUNK_SIZE: usize = 64 * 1024;

/// Maximum number of HTTP redirects followed manually for a download.
const MAX_REDIRECTS: usize = 5;

/// Release tag of the pinned `llama.cpp` archives, used for the extraction folder name.
pub const LLAMA_SERVER_RELEASE: &str = "b12774";

/// Exact host allowlist for production pinned downloads (case-insensitive).
pub const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "huggingface.co",
    "cdn-lfs.huggingface.co",
    "cdn-lfs-us-1.hf.co",
];

/// Network policy controlling URL validation and TLS enforcement for downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadPolicy {
    /// Only enabled in local tests using an ephemeral `127.0.0.1` HTTP server.
    pub allow_loopback_http: bool,
}

impl DownloadPolicy {
    /// Strict production policy: HTTPS only and exact allowlisted hosts.
    pub const PRODUCTION: Self = Self {
        allow_loopback_http: false,
    };
}

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

/// Validates that `file_name` is a plain file name with no path traversal, drive/stream
/// separators, NUL bytes, or control characters.
fn validate_file_name(file_name: &str) -> Result<(), DownloadError> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains(['/', '\\', ':', '\0'])
        || file_name.chars().any(char::is_control)
    {
        return Err(DownloadError::Failed(
            "download target has an invalid file name".to_string(),
        ));
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    lower == "localhost"
        || lower.ends_with(".localhost")
        || lower
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn is_allowlisted_host(host: &str) -> bool {
    ALLOWED_DOWNLOAD_HOSTS
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
}

/// Extracts a safe host name from `url` (never includes credentials, path or query string) and
/// validates the scheme and host against `policy`. Returns `(host, is_https)`.
fn validate_url_host(url: &str, policy: DownloadPolicy) -> Result<(String, bool), DownloadError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| DownloadError::Failed("download address is not a valid URL".to_string()))?;
    let scheme_lower = scheme.to_ascii_lowercase();
    if scheme_lower != "https" && scheme_lower != "http" {
        return Err(DownloadError::Failed(
            "download address must use https://".to_string(),
        ));
    }
    let is_https = scheme_lower == "https";
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
    let host_lower = host.to_ascii_lowercase();
    let loopback = is_loopback_host(&host_lower);

    if policy.allow_loopback_http {
        if loopback {
            return Ok((host_lower, is_https));
        }
        if !is_https {
            return Err(DownloadError::Failed(
                "plain http:// downloads are only allowed for loopback addresses".to_string(),
            ));
        }
        if !is_allowlisted_host(&host_lower) {
            return Err(DownloadError::Failed(format!(
                "download host {host_lower} is not allowed"
            )));
        }
        return Ok((host_lower, true));
    }

    if !is_https {
        return Err(DownloadError::Failed(
            "download address must use https://".to_string(),
        ));
    }
    if loopback || !is_allowlisted_host(&host_lower) {
        return Err(DownloadError::Failed(format!(
            "download host {host_lower} is not allowed"
        )));
    }
    Ok((host_lower, true))
}

fn resolve_redirect_url(current_url: &str, location: &str) -> Result<String, DownloadError> {
    let loc = location.trim();
    if loc.contains("://") {
        return Ok(loc.to_string());
    }
    if loc.starts_with('/') {
        let (scheme, rest) = current_url.split_once("://").ok_or_else(|| {
            DownloadError::Failed("download address is not a valid URL".to_string())
        })?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() {
            return Err(DownloadError::Failed(
                "download address has no host".to_string(),
            ));
        }
        return Ok(format!("{scheme}://{authority}{loc}"));
    }
    Err(DownloadError::Failed(
        "download redirect target is not a valid URL".to_string(),
    ))
}

/// Validates a single redirect hop against `policy`, rejecting protocol downgrades
/// (`https://` -> `http://`) and hosts outside the policy allowlist.
fn validate_redirect_hop(
    current_url: &str,
    current_is_https: bool,
    location: &str,
    policy: DownloadPolicy,
) -> Result<(String, String, bool), DownloadError> {
    let next_url = resolve_redirect_url(current_url, location)?;
    let (next_host, next_is_https) = validate_url_host(&next_url, policy)?;
    if current_is_https && !next_is_https {
        return Err(DownloadError::Failed(
            "download redirect from https:// to http:// was refused".to_string(),
        ));
    }
    Ok((next_url, next_host, next_is_https))
}

/// Parses an HTTP `Content-Range: bytes <start>-<end>/<total|*>` header.
fn parse_content_range(header: &str) -> Option<(u64, Option<u64>)> {
    let trimmed = header.trim();
    let rest = trimmed
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bytes "))
        .map(|_| trimmed[6..].trim())?;
    let (range_part, total_part) = rest.split_once('/')?;
    let (start_str, end_str) = range_part.split_once('-')?;
    let start: u64 = start_str.trim().parse().ok()?;
    let end: u64 = end_str.trim().parse().ok()?;
    if start > end {
        return None;
    }
    let total_trimmed = total_part.trim();
    let total = if total_trimmed == "*" {
        None
    } else {
        let t: u64 = total_trimmed.parse().ok()?;
        if end >= t {
            return None;
        }
        Some(t)
    };
    Some((start, total))
}

fn download_agent(policy: DownloadPolicy) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(300)))
        .https_only(!policy.allow_loopback_http)
        .http_status_as_error(false)
        .max_redirects(0)
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

/// Downloads `pin` from its pinned URL into `download_dir` (under Local AppData) using the
/// strict production policy (HTTPS only, exact host allowlist).
pub fn download_pin(
    pin: &Pin,
    download_dir: &Path,
    cancel: &Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, DownloadError> {
    download_pin_from(
        pin.url,
        pin,
        download_dir,
        DownloadPolicy::PRODUCTION,
        cancel,
        on_progress,
    )
}

/// Downloads `pin` from `url` into `download_dir`, resuming from `<file_name>.part` when
/// possible, verifying exact byte size and SHA256, and renaming atomically on success.
pub fn download_pin_from(
    url: &str,
    pin: &Pin,
    download_dir: &Path,
    policy: DownloadPolicy,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, DownloadError> {
    validate_file_name(pin.file_name)?;
    let (mut host, mut current_is_https) = validate_url_host(url, policy)?;

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

    let agent = download_agent(policy);
    let mut current_url = url.to_string();
    let mut hops = 0usize;
    let response = loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let mut request = agent
            .get(&current_url)
            .header("User-Agent", "blah-blah-bot");
        if existing_len > 0 {
            request = request.header("Range", &format!("bytes={existing_len}-"));
        }
        let resp = request.call().map_err(|e| map_ureq_error(&e, &host))?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if hops >= MAX_REDIRECTS {
                return Err(DownloadError::Failed(format!(
                    "download from {host} exceeded the maximum number of redirects"
                )));
            }
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    DownloadError::Failed(format!(
                        "download from {host} returned a redirect without a target"
                    ))
                })?;
            let (next_url, next_host, next_is_https) =
                validate_redirect_hop(&current_url, current_is_https, location, policy)?;
            current_url = next_url;
            host = next_host;
            current_is_https = next_is_https;
            hops += 1;
            continue;
        }
        break resp;
    };

    let status = response.status().as_u16();
    let (mut file, mut hasher, mut downloaded) = match status {
        206 if existing_len > 0 => {
            let content_range = response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok());
            let range_ok =
                content_range
                    .and_then(parse_content_range)
                    .is_some_and(|(start, total)| {
                        start == existing_len && total.is_none_or(|t| t == pin.size)
                    });
            if !range_ok {
                remove_file_in_dir(download_dir, &part_path);
                return Err(DownloadError::Failed(
                    "server returned an unexpected partial range; the partial download was cleared and can be retried"
                        .to_string(),
                ));
            }
            let mut h = Sha256::new();
            let hashed = hash_existing_file(&part_path, &mut h, cancel)?;
            if hashed != existing_len {
                remove_file_in_dir(download_dir, &part_path);
                return Err(DownloadError::Failed(
                    "partial download changed during resume; the partial download was cleared and can be retried"
                        .to_string(),
                ));
            }
            let f = OpenOptions::new()
                .append(true)
                .open(&part_path)
                .map_err(|e| {
                    DownloadError::Failed(format!("could not open partial download file: {e}"))
                })?;
            (f, h, hashed)
        }
        206 => {
            remove_file_in_dir(download_dir, &part_path);
            return Err(DownloadError::Failed(
                "server returned partial content when a full download was requested; please retry"
                    .to_string(),
            ));
        }
        200 => {
            if cancel.load(Ordering::SeqCst) {
                return Err(DownloadError::Cancelled);
            }
            let f = File::create(&part_path).map_err(|e| {
                DownloadError::Failed(format!("could not create partial download file: {e}"))
            })?;
            (f, Sha256::new(), 0u64)
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

    file.sync_all().map_err(|e| {
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
    fs::rename(part_path, final_path).map_err(|e| {
        DownloadError::Failed(format!("could not save the verified download file: {e}"))
    })?;
    Ok(final_path.to_path_buf())
}

/// Extracts a ZIP archive into `target_dir`, failing closed on the first unsafe entry.
///
/// Every entry is validated before anything is written: absolute paths, drive prefixes,
/// `..` traversal components, NUL bytes, and symlinks are rejected so nothing can be
/// written outside `target_dir`.
pub fn extract_zip_safe(zip_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file = File::open(zip_path)
        .map_err(|e| format!("could not open archive {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("could not read archive {}: {e}", zip_path.display()))?;

    // Pass 1: validate every entry before creating directories or writing any files.
    let mut rel_paths: Vec<Option<PathBuf>> = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("could not read archive entry: {e}"))?;
        let raw_name: String = entry
            .name()
            .map_err(|_| "archive contains an entry with an invalid name".to_string())?
            .into_owned();
        if raw_name.is_empty()
            || raw_name.starts_with(['/', '\\'])
            || raw_name.contains([':', '\0'])
        {
            return Err(format!("archive contains an unsafe path: {raw_name}"));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| (mode & 0o170000) == 0o120000)
        {
            return Err(format!("archive contains a symlink entry: {raw_name}"));
        }
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("archive contains an unsafe path: {raw_name}"));
        };
        if rel.as_os_str().is_empty() {
            if entry.is_dir() {
                rel_paths.push(None);
                continue;
            }
            return Err(format!("archive contains an unsafe path: {raw_name}"));
        }
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!("archive contains an unsafe path: {raw_name}"));
        }
        rel_paths.push(Some(rel));
    }

    // Pass 2: extract validated entries into `target_dir`.
    fs::create_dir_all(target_dir)
        .map_err(|e| format!("could not create extraction folder: {e}"))?;
    for (i, rel_opt) in rel_paths.into_iter().enumerate() {
        let Some(rel) = rel_opt else {
            continue;
        };
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("could not read archive entry: {e}"))?;
        let dest = target_dir.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest)
                .map_err(|e| format!("could not create folder during extraction: {e}"))?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create folder during extraction: {e}"))?;
            }
            let mut out = File::create(&dest)
                .map_err(|e| format!("could not extract file from archive: {e}"))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| format!("could not extract file from archive: {e}"))?;
            out.sync_all()
                .map_err(|e| format!("could not flush extracted file: {e}"))?;
        }
    }
    Ok(())
}

/// Searches `root` for a file named `target_name` (case-insensitive) up to `max_depth`
/// directory levels deep without following symlinks.
pub fn find_file_in_tree(root: &Path, target_name: &str, max_depth: usize) -> Option<PathBuf> {
    fn walk(dir: &Path, target_name: &str, depth: usize, max_depth: usize) -> Option<PathBuf> {
        let Ok(read) = fs::read_dir(dir) else {
            return None;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for entry in entries {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(target_name)
            {
                return Some(path);
            }
            if ft.is_dir() && depth < max_depth {
                subdirs.push(path);
            }
        }
        for sub in subdirs {
            if let Some(found) = walk(&sub, target_name, depth + 1, max_depth) {
                return Some(found);
            }
        }
        None
    }
    walk(root, target_name, 0, max_depth)
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

    const TEST_POLICY: DownloadPolicy = DownloadPolicy {
        allow_loopback_http: true,
    };

    fn spawn_raw_response_server(response: String) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line) {
                if n == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://{addr}/artifact.bin"), handle)
    }

    #[test]
    fn download_fresh_success() {
        let dir = TestDir::new("dl-fresh");
        let payload = make_payload(10_000);
        let pin = pin_for("fresh.bin", &payload);
        let (url, server) = spawn_file_server(payload.clone(), RangeMode::Support206);

        let cancel = Arc::new(AtomicBool::new(false));
        let mut phases = Vec::new();
        let out = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |p| {
            phases.push(p.phase);
        })
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
        let out = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |p| {
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
        let out = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
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
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
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
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, move |p| {
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
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
            .expect_err("truncated file must fail");
        server.join().expect("server thread");

        match err {
            DownloadError::Failed(msg) => assert!(msg.contains("expected size"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!dir.path().join("badsize.bin").exists());
        assert!(!dir.path().join("badsize.bin.part").exists());
    }

    #[test]
    fn redirect_to_non_allowlisted_host_is_rejected() {
        let dir = TestDir::new("dl-redir-host");
        let payload = make_payload(512);
        let pin = pin_for("redir.bin", &payload);
        let (url, server) = spawn_raw_response_server(
            "HTTP/1.1 302 Found\r\nLocation: https://evil.example.invalid/file.bin\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
            .expect_err("redirect to non-allowlisted host must fail");
        server.join().expect("server thread");

        match err {
            DownloadError::Failed(msg) => assert!(msg.contains("not allowed"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn https_to_http_downgrade_redirect_is_rejected() {
        let prod_err = validate_redirect_hop(
            "https://github.com/org/repo/releases/download/v1/a.zip",
            true,
            "http://objects.githubusercontent.com/a.zip",
            DownloadPolicy::PRODUCTION,
        )
        .expect_err("https -> http downgrade must fail in production");
        match prod_err {
            DownloadError::Failed(msg) => assert!(msg.contains("https://"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }

        let test_err = validate_redirect_hop(
            "https://127.0.0.1:8443/a.zip",
            true,
            "http://127.0.0.1:8080/a.zip",
            TEST_POLICY,
        )
        .expect_err("https -> http downgrade must fail even on loopback");
        match test_err {
            DownloadError::Failed(msg) => assert!(msg.contains("http://"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn redirect_to_loopback_rejected_in_production_policy() {
        for target in [
            "https://127.0.0.1/file.bin",
            "http://127.0.0.1/file.bin",
            "https://localhost/file.bin",
            "https://[::1]/file.bin",
        ] {
            let err = validate_redirect_hop(
                "https://github.com/org/repo/file.bin",
                true,
                target,
                DownloadPolicy::PRODUCTION,
            )
            .expect_err("loopback redirect must fail in production policy");
            assert!(matches!(err, DownloadError::Failed(_)), "{target}: {err:?}");
        }
    }

    #[test]
    fn url_and_file_name_validation_rejects_unsafe_inputs() {
        for bad_url in [
            "https://user:pass@github.com/file.bin",
            "https://token@huggingface.co/file.bin",
            "ftp://github.com/file.bin",
            "file:///C:/Windows/System32/cmd.exe",
            "javascript:alert(1)",
            "https:///no-host",
        ] {
            assert!(
                validate_url_host(bad_url, DownloadPolicy::PRODUCTION).is_err(),
                "expected URL rejection for {bad_url}"
            );
            assert!(
                validate_url_host(bad_url, TEST_POLICY).is_err(),
                "expected URL rejection in test policy for {bad_url}"
            );
        }

        for bad_name in [
            "",
            ".",
            "..",
            "../evil.bin",
            "sub/file.bin",
            "sub\\file.bin",
            "C:evil.bin",
            "ads:stream.bin",
            "nul\0byte.bin",
            "ctrl\nchar.bin",
            "ctrl\rchar.bin",
        ] {
            assert!(
                validate_file_name(bad_name).is_err(),
                "expected file name rejection for {bad_name:?}"
            );
        }
        assert!(validate_file_name("gemma-4-12B-it-Q4_K_M.gguf").is_ok());
    }

    #[test]
    fn http_416_and_5xx_fail_with_plain_error_and_keep_part() {
        let payload = make_payload(4_096);
        let pin = pin_for("keep-part.bin", &payload);
        let initial_part = payload[..1_024].to_vec();

        for (status_line, code_str) in [
            ("HTTP/1.1 416 Range Not Satisfiable", "416"),
            ("HTTP/1.1 503 Service Unavailable", "503"),
        ] {
            let dir = TestDir::new(&format!("dl-http-{code_str}"));
            let part = dir.path().join("keep-part.bin.part");
            fs::write(&part, &initial_part).unwrap();

            let (url, server) = spawn_raw_response_server(format!(
                "{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ));
            let cancel = Arc::new(AtomicBool::new(false));
            let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
                .expect_err("HTTP error must fail");
            server.join().expect("server thread");

            match err {
                DownloadError::Failed(msg) => assert!(msg.contains(code_str), "{msg}"),
                other => panic!("expected Failed for {code_str}, got {other:?}"),
            }
            assert!(part.is_file(), ".part must be kept on HTTP {code_str}");
            assert_eq!(fs::read(&part).unwrap(), initial_part);
        }
    }

    #[test]
    fn oversize_stream_fails_and_deletes_part() {
        let dir = TestDir::new("dl-oversize");
        let expected = make_payload(4_000);
        let oversized = make_payload(8_000);
        let pin = pin_for("oversize.bin", &expected);

        let (url, server) = spawn_file_server(oversized, RangeMode::Ignore200);
        let cancel = Arc::new(AtomicBool::new(false));
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
            .expect_err("oversize stream must fail");
        server.join().expect("server thread");

        match err {
            DownloadError::Failed(msg) => assert!(msg.contains("exceeded"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!dir.path().join("oversize.bin").exists());
        assert!(!dir.path().join("oversize.bin.part").exists());
    }

    #[test]
    fn resume_with_changed_part_fails_closed() {
        let dir = TestDir::new("dl-changed-part");
        let payload = make_payload(10_000);
        let pin = pin_for("changed.bin", &payload);
        let part = dir.path().join("changed.bin.part");
        let mut tampered_prefix = payload[..5_000].to_vec();
        tampered_prefix[42] ^= 0xFF;
        fs::write(&part, &tampered_prefix).unwrap();

        let (url, server) = spawn_file_server(payload.clone(), RangeMode::Support206);
        let cancel = Arc::new(AtomicBool::new(false));
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
            .expect_err("resuming with a modified .part must fail closed");
        server.join().expect("server thread");

        assert_eq!(
            err,
            DownloadError::Failed(CHECKSUM_MISMATCH_MSG.to_string())
        );
        assert!(!dir.path().join("changed.bin").exists());
        assert!(!part.exists());
    }

    #[test]
    fn resume_with_mismatched_content_range_fails_closed_and_deletes_part() {
        let dir = TestDir::new("dl-bad-range");
        let payload = make_payload(4_000);
        let pin = pin_for("badrange.bin", &payload);
        let part = dir.path().join("badrange.bin.part");
        fs::write(&part, &payload[..1_000]).unwrap();

        // Server claims Content-Range starts at 0 instead of 1000.
        let (url, server) = spawn_raw_response_server(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3999/4000\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest"
                .to_string(),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let err = download_pin_from(&url, &pin, dir.path(), TEST_POLICY, &cancel, |_| {})
            .expect_err("mismatched Content-Range must fail closed");
        server.join().expect("server thread");

        match err {
            DownloadError::Failed(msg) => assert!(msg.contains("partial range"), "{msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!part.exists(), ".part must be deleted on bad 206");
    }

    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).expect("create test zip");
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, opts).expect("start zip entry");
            writer.write_all(data).expect("write zip entry");
        }
        writer.finish().expect("finish test zip");
    }

    #[test]
    fn extract_zip_safe_rejects_zip_slip_without_writing_outside() {
        let dir = TestDir::new("dl-zip-slip");
        let zip_path = dir.path().join("evil.zip");
        let target = dir.path().join("extract");
        write_test_zip(
            &zip_path,
            &[("benign.txt", b"ok"), ("../evil.txt", b"escaped")],
        );

        let err =
            extract_zip_safe(&zip_path, &target).expect_err("zip-slip archive must be rejected");
        assert!(err.contains("unsafe path"), "{err}");
        assert!(!dir.path().join("evil.txt").exists());
        assert!(!target.join("benign.txt").exists());
    }

    #[test]
    fn extract_zip_safe_extracts_valid_zip_and_finds_llama_server() {
        let dir = TestDir::new("dl-zip-ok");
        let zip_path = dir.path().join("llama.zip");
        let target = dir.path().join("extract");
        write_test_zip(
            &zip_path,
            &[
                ("nested/bin/llama-server.exe", b"MZ fake llama"),
                ("nested/bin/cudart64_12.dll", b"MZ fake cudart"),
            ],
        );

        extract_zip_safe(&zip_path, &target).expect("valid zip should extract");
        let found = find_file_in_tree(&target, "llama-server.exe", 3)
            .expect("llama-server.exe should be found");
        assert_eq!(
            found,
            target.join("nested").join("bin").join("llama-server.exe")
        );
        assert_eq!(fs::read(&found).unwrap(), b"MZ fake llama");
    }
}
