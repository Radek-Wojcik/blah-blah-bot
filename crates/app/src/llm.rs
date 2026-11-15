//! The language-model backend of the extraction stages: settings, the per-meeting choice
//! with its persistence, and the confirmation a cloud backend needs.
//!
//! Everything here works on a `&Store` and plain values, so it is tested without Tauri. The
//! API keys never pass through this module: it only asks whether one is stored.

use std::path::{Path, PathBuf};

use queue::transcript::TranscriptFiles;
use queue::worker::{self, LlmOptions, Secret};
use serde::{Deserialize, Serialize};
use store::{LlmKind, Store};

use crate::ipc::{IpcError, IpcResult};
use crate::secrets;

/// Smallest context the worker accepts.
pub const MIN_CTX: u32 = 2048;
const MAX_CTX: u32 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManagedLlmSettings {
    /// Path to the GGUF file the worker starts `llama-server` with.
    pub model_path: String,
    /// Context size in tokens.
    pub ctx: u32,
}

impl Default for ManagedLlmSettings {
    fn default() -> Self {
        Self {
            model_path: String::new(),
            ctx: 16384,
        }
    }
}

/// An OpenAI-compatible endpoint (external server or hosted API).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EndpointSettings {
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmSettings {
    /// Backend used when a meeting is processed without an explicit choice. `managed` or
    /// `external`: the cloud is always chosen, and confirmed, for one meeting.
    pub default_kind: LlmKind,
    pub managed: ManagedLlmSettings,
    pub external: EndpointSettings,
    pub cloud: EndpointSettings,
    /// Filled when reading; ignored when saving. The keys themselves are never returned.
    #[serde(skip_deserializing)]
    pub external_key_set: bool,
    #[serde(skip_deserializing)]
    pub cloud_key_set: bool,
    /// Filled when reading: the external backend is the default and its server is not on
    /// this machine or in a private network, so every meeting of a cycle goes to it without
    /// a confirmation.
    #[serde(skip_deserializing)]
    pub external_host_is_remote: bool,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            default_kind: LlmKind::Managed,
            managed: ManagedLlmSettings::default(),
            external: EndpointSettings::default(),
            cloud: EndpointSettings::default(),
            external_key_set: false,
            cloud_key_set: false,
            external_host_is_remote: false,
        }
    }
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::new("invalid", message)
}

/// Whether `host` is this machine: `localhost` (and `*.localhost`), 127.0.0.0/8, `::1`.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Whether `host` is this machine or on a private network: loopback, private and
/// link-local addresses, and names that cannot be on the public internet (a name without a
/// dot, `.local`, `.lan`, `.home.arpa`, `.internal`).
pub fn is_local_host(host: &str) -> bool {
    if is_loopback_host(host) {
        return true;
    }
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => {
                let first = v6.segments()[0];
                // fc00::/7 unique local, fe80::/10 link-local.
                (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
            }
        };
    }
    !host.contains('.')
        || [".local", ".lan", ".home.arpa", ".internal"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// An endpoint URL: http or https, with a host and no credentials in it (a user name counts:
/// tokens are often put there). With `https_required` plain `http://` is only accepted for
/// this machine.
fn check_url(what: &str, url: &str, https_required: bool) -> IpcResult<()> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(());
    }
    let (rest, secure) = match url.strip_prefix("https://") {
        Some(rest) => (rest, true),
        None => (
            url.strip_prefix("http://").ok_or_else(|| {
                invalid(format!(
                    "{what}: the address must start with http:// or https://"
                ))
            })?,
            false,
        ),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(invalid(format!(
            "{what}: do not put a user name or key into the address; keys are stored separately"
        )));
    }
    let Some(host) = worker::url_host(url) else {
        return Err(invalid(format!("{what}: the address has no host")));
    };
    if https_required && !secure && !is_loopback_host(&host) {
        return Err(invalid(format!(
            "{what}: the address must start with https:// (plain http:// is only allowed for this machine)"
        )));
    }
    Ok(())
}

impl LlmSettings {
    /// Refuses values the worker or the confirmation could not use.
    pub fn validate(&self) -> IpcResult<()> {
        if self.default_kind == LlmKind::Cloud {
            return Err(invalid(
                "the default backend cannot be the cloud: it is chosen and confirmed per meeting",
            ));
        }
        if !(MIN_CTX..=MAX_CTX).contains(&self.managed.ctx) {
            return Err(invalid(format!(
                "the context size must be between {MIN_CTX} and {MAX_CTX} tokens"
            )));
        }
        check_url("external server", &self.external.base_url, false)?;
        check_url("cloud endpoint", &self.cloud.base_url, true)?;
        Ok(())
    }

    /// Host of the endpoint of `kind` in these settings.
    fn host_of(&self, kind: LlmKind) -> Option<String> {
        let endpoint = match kind {
            LlmKind::Managed => return None,
            LlmKind::External => &self.external,
            LlmKind::Cloud => &self.cloud,
        };
        worker::url_host(endpoint.base_url.trim())
    }

    /// Name of the credential entry that holds the key of `kind` for the endpoint set now;
    /// `None` for the managed backend or an address without a host.
    pub fn key_entry(&self, kind: LlmKind) -> Option<String> {
        let base = match kind {
            LlmKind::Managed => return None,
            LlmKind::External => secrets::EXTERNAL_KEY,
            LlmKind::Cloud => secrets::CLOUD_KEY,
        };
        Some(secrets::endpoint_key(base, &self.host_of(kind)?))
    }

    /// Whether the external backend is the default one and its server is not on this
    /// machine or in a private network.
    pub fn external_default_is_remote(&self) -> bool {
        self.default_kind == LlmKind::External
            && self
                .host_of(LlmKind::External)
                .is_some_and(|h| !is_local_host(&h))
    }

    /// The worker options for `kind`, or `llm_not_configured` when a value is missing.
    pub fn options_for(&self, kind: LlmKind) -> IpcResult<LlmOptions> {
        let missing = |what: &str| {
            IpcError::new(
                "llm_not_configured",
                format!(
                    "the {} language model has no {what} in the settings",
                    kind.as_str()
                ),
            )
        };
        let endpoint = |e: &EndpointSettings| -> IpcResult<(String, String)> {
            let url = e.base_url.trim();
            let model = e.model.trim();
            if url.is_empty() {
                return Err(missing("address"));
            }
            if model.is_empty() {
                return Err(missing("model name"));
            }
            check_url(kind.as_str(), url, kind == LlmKind::Cloud)?;
            Ok((url.to_string(), model.to_string()))
        };
        match kind {
            LlmKind::Managed => {
                let path = self.managed.model_path.trim();
                if path.is_empty() {
                    return Err(missing("model file"));
                }
                Ok(LlmOptions {
                    kind,
                    model_path: Some(path.to_string()),
                    ctx: Some(self.managed.ctx),
                    base_url: None,
                    model: None,
                    cloud_confirmed: false,
                })
            }
            LlmKind::External | LlmKind::Cloud => {
                let (base_url, model) = endpoint(if kind == LlmKind::External {
                    &self.external
                } else {
                    &self.cloud
                })?;
                Ok(LlmOptions {
                    kind,
                    model_path: None,
                    ctx: None,
                    base_url: Some(base_url),
                    model: Some(model),
                    cloud_confirmed: false,
                })
            }
        }
    }
}

/// Parses the kind the UI sends.
pub fn parse_kind(s: &str) -> IpcResult<LlmKind> {
    match s {
        "managed" => Ok(LlmKind::Managed),
        "external" => Ok(LlmKind::External),
        "cloud" => Ok(LlmKind::Cloud),
        _ => Err(invalid("unknown language model backend")),
    }
}

/// The key a stage of a job with this backend may see: the key saved for the host of the
/// job's own endpoint (external or cloud), never anything for the managed backend, for an
/// address without a host, or for another host than the one the key was saved for.
pub fn key_for(llm: &LlmOptions, get: impl Fn(&str) -> Option<String>) -> Option<Secret> {
    let base = match llm.kind {
        LlmKind::Managed => return None,
        LlmKind::External => secrets::EXTERNAL_KEY,
        LlmKind::Cloud => secrets::CLOUD_KEY,
    };
    let host = llm.base_url.as_deref().and_then(worker::url_host)?;
    get(&secrets::endpoint_key(base, &host)).map(Secret::new)
}

/// Decides the backend of a meeting that is processed, records it in the store and returns
/// the options to put into the job. `None` means the language-model stages are skipped.
///
/// * A meeting without a cycle has nothing to extract: `None`, nothing recorded.
/// * Without an explicit choice the default backend is used; when it is not configured the
///   stages are skipped (the transcript still gets made). An explicit choice that is not
///   configured is an error (`llm_not_configured`).
/// * The cloud needs its key (`cloud_key_missing`) and a confirmation given earlier with
///   [`confirm_cloud`] for this endpoint (`cloud_not_confirmed`). Choosing it never
///   confirms it.
pub fn choose_llm(
    store: &Store,
    meeting_id: &str,
    settings: &LlmSettings,
    requested: Option<LlmKind>,
    key_present: &dyn Fn(&str) -> bool,
) -> IpcResult<Option<LlmOptions>> {
    let report = store
        .get_report_settings(meeting_id)?
        .ok_or_else(|| IpcError::new("not_found", "meeting not found"))?;
    if report.cycle_id.is_none() {
        return Ok(None);
    }
    let kind = requested.unwrap_or(settings.default_kind);
    let options = match settings.options_for(kind) {
        Ok(o) => o,
        Err(e) if requested.is_none() && e.code == "llm_not_configured" => {
            store.clear_llm_choice(meeting_id)?;
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let key_saved = settings
        .key_entry(LlmKind::Cloud)
        .is_some_and(|entry| key_present(&entry));
    if kind == LlmKind::Cloud && !key_saved {
        return Err(IpcError::new(
            "cloud_key_missing",
            "store the API key of the cloud endpoint in the settings first",
        ));
    }
    let host = options.base_url.as_deref().and_then(worker::url_host);
    store.set_llm_choice(meeting_id, kind, host.as_deref())?;
    if kind == LlmKind::Cloud {
        let choice = store.get_llm_choice(meeting_id)?;
        if !worker::confirmed_for_run(&options, choice.as_ref()) {
            return Err(IpcError::new(
                "cloud_not_confirmed",
                "confirm sending this meeting to the cloud endpoint first",
            ));
        }
    }
    Ok(Some(options))
}

/// What the confirmation dialog of a cloud backend shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPreview {
    pub endpoint_host: String,
    pub model: String,
    /// Words of the transcript that would be sent: edited text, excluded passages left out.
    pub word_count: u64,
    pub screenshot_descriptions: u32,
    /// Items of the cycle's earlier meetings that go along with every request (their text,
    /// assignee and due date), whatever those meetings' own backend was.
    pub previous_item_count: u32,
    pub includes_audio: bool,
    pub includes_images: bool,
    pub summary_text: String,
    /// The meeting already has a confirmation for this endpoint.
    pub confirmed: bool,
    /// False while the meeting has no transcript yet (the word count is then 0).
    pub transcript_ready: bool,
}

fn cloud_endpoint(settings: &LlmSettings) -> IpcResult<(LlmOptions, String)> {
    let options = settings.options_for(LlmKind::Cloud)?;
    let host = options
        .base_url
        .as_deref()
        .and_then(worker::url_host)
        .ok_or_else(|| IpcError::new("llm_not_configured", "the cloud address has no host"))?;
    Ok((options, host))
}

fn count_descriptions(meeting_dir: &Path) -> u32 {
    std::fs::read(meeting_dir.join("work").join("descriptions.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("items").and_then(|i| i.as_array().map(Vec::len)))
        .unwrap_or(0) as u32
}

/// The most items of earlier meetings the prompts of the worker carry.
pub const MAX_PREVIOUS_ITEMS_SENT: u32 = 30;

fn items_in(file: &Path) -> Option<(Option<String>, usize)> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let items = value.get("items")?.as_array()?.len();
    let id = value
        .get("meeting_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some((id, items))
}

/// How many items of the cycle's earlier meetings go along with a request: the items of the
/// finished drafts of its other meetings (here and in `repo_previous_dirs`, the folders of
/// `meetings/<cycle>` of the repositories), at most [`MAX_PREVIOUS_ITEMS_SENT`]. These are
/// sent whichever backend those meetings used.
fn count_previous_items(
    store: &Store,
    meeting_dir: &Path,
    meeting_id: &str,
    repo_previous_dirs: &[PathBuf],
) -> IpcResult<u32> {
    let Some(details) = store.get_meeting(meeting_id)? else {
        return Ok(0);
    };
    let Some(cycle_id) = store
        .get_report_settings(meeting_id)?
        .and_then(|s| s.cycle_id)
    else {
        return Ok(0);
    };
    let depth = Path::new(&details.meeting.dir).components().count();
    let root = meeting_dir.ancestors().nth(depth);
    let mut seen = std::collections::HashSet::new();
    seen.insert(meeting_id.to_string());
    let mut total = 0usize;
    if let Some(root) = root {
        for m in store.list_meetings_by_cycle(&cycle_id)? {
            if m.id == meeting_id {
                continue;
            }
            let file = root.join(&m.dir).join("work").join("draft.json");
            if let Some((_, n)) = items_in(&file) {
                if seen.insert(m.id.clone()) {
                    total += n;
                }
            }
        }
    }
    for dir in repo_previous_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some((id, n)) = items_in(&entry.path().join("meeting.json")) {
                let unique = id.is_none_or(|id| seen.insert(id));
                if unique {
                    total += n;
                }
            }
        }
    }
    Ok(total.min(MAX_PREVIOUS_ITEMS_SENT as usize) as u32)
}

/// What would be sent to the cloud endpoint for this meeting. Reads only; nothing is
/// recorded and nothing is confirmed. `repo_previous_dirs` are the folders with the earlier
/// meetings of the cycle in its repositories.
pub fn cloud_preview(
    store: &Store,
    settings: &LlmSettings,
    meeting_dir: &Path,
    meeting_id: &str,
    repo_previous_dirs: &[PathBuf],
) -> IpcResult<CloudPreview> {
    if store.get_meeting(meeting_id)?.is_none() {
        return Err(IpcError::new("not_found", "meeting not found"));
    }
    let (options, host) = cloud_endpoint(settings)?;
    let model = options.model.clone().unwrap_or_default();
    let (word_count, transcript_ready) = match TranscriptFiles::new(meeting_dir).view() {
        Ok(view) => (
            view.segments
                .iter()
                .filter(|s| !s.excluded)
                .map(|s| s.text.split_whitespace().count() as u64)
                .sum(),
            true,
        ),
        Err(_) => (0, false),
    };
    let screenshot_descriptions = count_descriptions(meeting_dir);
    let previous_item_count =
        count_previous_items(store, meeting_dir, meeting_id, repo_previous_dirs)?;
    let confirmed = worker::confirmed_for_run(&options, store.get_llm_choice(meeting_id)?.as_ref());
    let amount = if transcript_ready {
        format!(
            "the text of the transcript ({word_count} words; passages you excluded are left out) \
             and {screenshot_descriptions} screenshot descriptions"
        )
    } else {
        "the text of the transcript and the screenshot descriptions (the amount is known once \
         the meeting is transcribed)"
            .to_string()
    };
    let previous = match previous_item_count {
        0 => "no items".to_string(),
        1 => "1 item".to_string(),
        n => format!("up to {n} items"),
    };
    let summary_text = format!(
        "This will send {amount} to {host} (model {model}), together with the names, glossary, \
         instruction and open topics of the cycle, and {previous} (text, assignee and due date) \
         from earlier meetings of the cycle, including meetings you processed only locally. \
         No audio and no images are sent. Nothing is sent without this confirmation."
    );
    Ok(CloudPreview {
        endpoint_host: host,
        model,
        word_count,
        screenshot_descriptions,
        previous_item_count,
        includes_audio: false,
        includes_images: false,
        summary_text,
        confirmed,
        transcript_ready,
    })
}

/// Records the user's confirmation that this meeting's text may be sent to the cloud
/// endpoint of the settings. Call it only from the explicit confirmation command.
pub fn confirm_cloud(
    store: &Store,
    settings: &LlmSettings,
    meeting_id: &str,
    now_ms: i64,
) -> IpcResult<()> {
    let (_, host) = cloud_endpoint(settings)?;
    if !store.set_llm_choice(meeting_id, LlmKind::Cloud, Some(&host))? {
        return Err(IpcError::new("not_found", "meeting not found"));
    }
    if !store.confirm_cloud(meeting_id, now_ms)? {
        return Err(IpcError::new("internal", "the confirmation was not stored"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{Meeting, MeetingState, ReportSettings};

    fn store_with(cycle: Option<&str>) -> Store {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&Meeting {
            id: "m1".into(),
            title: "t".into(),
            started_unix_ms: 1,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: "m1".into(),
        })
        .unwrap();
        if let Some(c) = cycle {
            s.set_report_settings(
                "m1",
                &ReportSettings {
                    cycle_id: Some(c.into()),
                    ..ReportSettings::default()
                },
            )
            .unwrap();
        }
        s
    }

    fn configured() -> LlmSettings {
        LlmSettings {
            managed: ManagedLlmSettings {
                model_path: "D:/models/gemma.gguf".into(),
                ctx: 16384,
            },
            external: EndpointSettings {
                base_url: "http://127.0.0.1:11434/v1".into(),
                model: "gemma".into(),
            },
            cloud: EndpointSettings {
                base_url: "https://api.example.com/v1".into(),
                model: "big-model".into(),
            },
            ..LlmSettings::default()
        }
    }

    const KEY_PRESENT: &dyn Fn(&str) -> bool = &|_| true;
    const NO_KEY: &dyn Fn(&str) -> bool = &|_| false;

    #[test]
    fn defaults_are_managed_with_a_16k_context_and_no_secrets() {
        let d = LlmSettings::default();
        assert_eq!(d.default_kind, LlmKind::Managed);
        assert_eq!(d.managed.ctx, 16384);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["defaultKind"], "managed");
        assert_eq!(json["managed"]["ctx"], 16384);
        assert!(json.get("cloudKeySet").is_some());
        // Key flags are never read back from a saved file.
        let back: LlmSettings =
            serde_json::from_str(r#"{"cloudKeySet":true,"externalKeySet":true}"#).unwrap();
        assert!(!back.cloud_key_set && !back.external_key_set);
    }

    #[test]
    fn validation_refuses_a_cloud_default_a_tiny_context_and_credentials_in_urls() {
        assert!(configured().validate().is_ok());
        let mut s = configured();
        s.default_kind = LlmKind::Cloud;
        assert_eq!(s.validate().unwrap_err().code, "invalid");
        let mut s = configured();
        s.managed.ctx = 512;
        assert_eq!(s.validate().unwrap_err().code, "invalid");
        let mut s = configured();
        s.cloud.base_url = "https://user:secret@api.example.com/v1".into();
        let e = s.validate().unwrap_err();
        assert_eq!(e.code, "invalid");
        assert!(!e.message.contains("secret"));
        let mut s = configured();
        s.external.base_url = "ftp://host".into();
        assert_eq!(s.validate().unwrap_err().code, "invalid");
    }

    #[test]
    fn a_key_or_user_name_in_any_address_is_refused() {
        for url in [
            "https://ghp_token@api.example.com/v1",
            "https://user:pass@api.example.com/v1",
            "https://@api.example.com/v1",
        ] {
            let mut s = configured();
            s.cloud.base_url = url.into();
            let e = s.validate().unwrap_err();
            assert_eq!(e.code, "invalid", "{url}");
            assert!(!e.message.contains("ghp_token") && !e.message.contains("pass"));
            let mut s = configured();
            s.external.base_url = url.into();
            assert_eq!(s.validate().unwrap_err().code, "invalid", "{url}");
            assert_eq!(
                s.options_for(LlmKind::External).unwrap_err().code,
                "invalid",
                "{url}"
            );
        }
    }

    #[test]
    fn the_cloud_needs_https_unless_it_is_this_machine() {
        let mut s = configured();
        s.cloud.base_url = "http://api.example.com/v1".into();
        let e = s.validate().unwrap_err();
        assert_eq!(e.code, "invalid");
        assert!(e.message.contains("https://"));
        // The options of a cloud meeting refuse it as well, whatever the settings file says.
        assert_eq!(s.options_for(LlmKind::Cloud).unwrap_err().code, "invalid");
        for ok in [
            "http://localhost:8080/v1",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:8080/v1",
            "https://api.example.com/v1",
        ] {
            s.cloud.base_url = ok.into();
            assert!(s.validate().is_ok(), "{ok}");
        }
        // The external server may use plain http on the network.
        s.external.base_url = "http://192.168.1.20:8000/v1".into();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn local_and_remote_hosts_are_told_apart() {
        for local in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "::1",
            "10.1.2.3",
            "172.16.0.5",
            "172.31.255.1",
            "192.168.0.10",
            "169.254.1.1",
            "fd12:3456::1",
            "fe80::1",
            "gpu-box",
            "nas.local",
            "box.lan",
        ] {
            assert!(is_local_host(local), "{local}");
        }
        for remote in [
            "api.openai.com",
            "8.8.8.8",
            "172.32.0.1",
            "172.15.0.1",
            "2001:db8::1",
            "example.com",
        ] {
            assert!(!is_local_host(remote), "{remote}");
        }
        assert!(is_loopback_host("127.0.0.2") && !is_loopback_host("192.168.0.1"));
    }

    #[test]
    fn a_remote_external_default_is_flagged_for_the_settings_screen() {
        let mut s = configured();
        assert!(!s.external_default_is_remote(), "managed default");
        s.default_kind = LlmKind::External;
        assert!(!s.external_default_is_remote(), "loopback server");
        s.external.base_url = "http://192.168.1.20:8000/v1".into();
        assert!(!s.external_default_is_remote(), "private network");
        s.external.base_url = "https://api.openai.com/v1".into();
        assert!(s.external_default_is_remote());
        s.default_kind = LlmKind::Managed;
        assert!(!s.external_default_is_remote());
        // The flag is filled when reading and never read back from a file.
        let back: LlmSettings = serde_json::from_str(r#"{"externalHostIsRemote":true}"#).unwrap();
        assert!(!back.external_host_is_remote);
        assert!(serde_json::to_value(&back)
            .unwrap()
            .get("externalHostIsRemote")
            .is_some());
    }

    #[test]
    fn options_come_from_the_settings_and_carry_no_key() {
        let s = configured();
        let m = s.options_for(LlmKind::Managed).unwrap();
        assert_eq!(m.model_path.as_deref(), Some("D:/models/gemma.gguf"));
        assert_eq!(m.ctx, Some(16384));
        let c = s.options_for(LlmKind::Cloud).unwrap();
        assert_eq!(c.base_url.as_deref(), Some("https://api.example.com/v1"));
        assert!(!c.cloud_confirmed);
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.to_lowercase().contains("key"));
        assert_eq!(
            LlmSettings::default()
                .options_for(LlmKind::Managed)
                .unwrap_err()
                .code,
            "llm_not_configured"
        );
    }

    #[test]
    fn a_meeting_without_a_cycle_gets_no_backend_and_nothing_is_recorded() {
        let s = store_with(None);
        assert_eq!(
            choose_llm(&s, "m1", &configured(), None, KEY_PRESENT).unwrap(),
            None
        );
        assert_eq!(s.get_llm_choice("m1").unwrap(), None);
    }

    #[test]
    fn the_default_backend_is_used_and_persisted() {
        let s = store_with(Some("NOMAD"));
        let o = choose_llm(&s, "m1", &configured(), None, NO_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(o.kind, LlmKind::Managed);
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert_eq!((c.kind, c.endpoint_host), (LlmKind::Managed, None));

        let o = choose_llm(&s, "m1", &configured(), Some(LlmKind::External), NO_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(o.kind, LlmKind::External);
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert_eq!(c.kind, LlmKind::External);
        assert_eq!(c.endpoint_host.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn an_unconfigured_default_skips_but_an_unconfigured_explicit_choice_fails() {
        let s = store_with(Some("NOMAD"));
        let empty = LlmSettings::default();
        assert_eq!(choose_llm(&s, "m1", &empty, None, NO_KEY).unwrap(), None);
        let e = choose_llm(&s, "m1", &empty, Some(LlmKind::External), NO_KEY).unwrap_err();
        assert_eq!(e.code, "llm_not_configured");
    }

    #[test]
    fn an_unknown_meeting_is_not_found() {
        let s = store_with(Some("NOMAD"));
        let e = choose_llm(&s, "nope", &configured(), None, NO_KEY).unwrap_err();
        assert_eq!(e.code, "not_found");
    }

    #[test]
    fn the_cloud_needs_its_key_and_an_explicit_confirmation() {
        let s = store_with(Some("NOMAD"));
        let cfg = configured();
        let e = choose_llm(&s, "m1", &cfg, Some(LlmKind::Cloud), NO_KEY).unwrap_err();
        assert_eq!(e.code, "cloud_key_missing");

        // Choosing it is recorded but never confirms it.
        let e = choose_llm(&s, "m1", &cfg, Some(LlmKind::Cloud), KEY_PRESENT).unwrap_err();
        assert_eq!(e.code, "cloud_not_confirmed");
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert_eq!(c.kind, LlmKind::Cloud);
        assert!(!c.cloud_confirmed);

        confirm_cloud(&s, &cfg, "m1", 42).unwrap();
        let o = choose_llm(&s, "m1", &cfg, Some(LlmKind::Cloud), KEY_PRESENT)
            .unwrap()
            .unwrap();
        assert_eq!(o.kind, LlmKind::Cloud);
        let c = s.get_llm_choice("m1").unwrap().unwrap();
        assert!(c.cloud_confirmed);
        assert_eq!(c.confirmed_unix_ms, Some(42));
        assert_eq!(c.endpoint_host.as_deref(), Some("api.example.com"));
    }

    #[test]
    fn a_confirmation_is_for_one_endpoint_and_a_backend_switch_withdraws_it() {
        let s = store_with(Some("NOMAD"));
        let mut cfg = configured();
        confirm_cloud(&s, &cfg, "m1", 1).unwrap();
        // The endpoint changed after the confirmation.
        cfg.cloud.base_url = "https://other.example.net/v1".into();
        let e = choose_llm(&s, "m1", &cfg, Some(LlmKind::Cloud), KEY_PRESENT).unwrap_err();
        assert_eq!(e.code, "cloud_not_confirmed");

        // Back at the confirmed endpoint the old confirmation is gone too.
        let cfg = configured();
        let e = choose_llm(&s, "m1", &cfg, Some(LlmKind::Cloud), KEY_PRESENT).unwrap_err();
        assert_eq!(e.code, "cloud_not_confirmed");

        confirm_cloud(&s, &cfg, "m1", 2).unwrap();
        choose_llm(&s, "m1", &cfg, Some(LlmKind::External), NO_KEY).unwrap();
        let e = choose_llm(&s, "m1", &cfg, Some(LlmKind::Cloud), KEY_PRESENT).unwrap_err();
        assert_eq!(e.code, "cloud_not_confirmed");
    }

    #[test]
    fn confirming_needs_a_cloud_endpoint_and_a_meeting() {
        let s = store_with(Some("NOMAD"));
        assert_eq!(
            confirm_cloud(&s, &LlmSettings::default(), "m1", 1)
                .unwrap_err()
                .code,
            "llm_not_configured"
        );
        assert_eq!(
            confirm_cloud(&s, &configured(), "nope", 1)
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn the_preview_counts_what_would_be_sent_and_confirms_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            work.join("transcript.json"),
            include_str!("../../../worker/tests/fixtures/transcript.json"),
        )
        .unwrap();
        std::fs::write(
            work.join("descriptions.json"),
            r#"{"items":[{"file":"img/a.png","offset_ms":1,"description":"x"},
                         {"file":"img/b.png","offset_ms":2,"description":"y"}]}"#,
        )
        .unwrap();
        let s = store_with(Some("NOMAD"));
        let cfg = configured();
        let p = cloud_preview(&s, &cfg, dir.path(), "m1", &[]).unwrap();
        assert_eq!(p.endpoint_host, "api.example.com");
        assert_eq!(p.model, "big-model");
        assert!(p.word_count > 0);
        assert_eq!(p.screenshot_descriptions, 2);
        assert!(!p.includes_audio && !p.includes_images);
        assert!(p.transcript_ready);
        assert!(!p.confirmed);
        assert!(p.summary_text.contains("api.example.com"));
        assert!(p.summary_text.contains(&p.word_count.to_string()));
        assert_eq!(s.get_llm_choice("m1").unwrap(), None);

        // Excluding a passage lowers the count.
        let edits = serde_json::json!({"segments": {"0": {"excluded": true}}});
        std::fs::write(work.join("edits.json"), edits.to_string()).unwrap();
        let after = cloud_preview(&s, &cfg, dir.path(), "m1", &[]).unwrap();
        assert!(after.word_count < p.word_count);

        confirm_cloud(&s, &cfg, "m1", 7).unwrap();
        assert!(
            cloud_preview(&s, &cfg, dir.path(), "m1", &[])
                .unwrap()
                .confirmed
        );
    }

    fn earlier_draft(root: &Path, folder: &str, meeting_id: &str, items: usize) {
        let dir = root.join(folder).join("work");
        std::fs::create_dir_all(&dir).unwrap();
        let items: Vec<_> = (0..items)
            .map(|i| serde_json::json!({"id": format!("i{i}"), "text": "x"}))
            .collect();
        std::fs::write(
            dir.join("draft.json"),
            serde_json::json!({"meeting_id": meeting_id, "items": items}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn the_preview_counts_the_items_of_earlier_meetings_that_go_along() {
        let root = tempfile::tempdir().unwrap();
        let s = store_with(Some("NOMAD"));
        let cfg = configured();
        let this = root.path().join("m1");
        std::fs::create_dir_all(&this).unwrap();
        let none = cloud_preview(&s, &cfg, &this, "m1", &[]).unwrap();
        assert_eq!(none.previous_item_count, 0);
        assert!(
            none.summary_text.contains("no items"),
            "{}",
            none.summary_text
        );

        // Two earlier meetings of the cycle (one processed locally only) and one of another.
        for (id, n, cycle) in [("m2", 4, "NOMAD"), ("m3", 5, "NOMAD"), ("m4", 9, "OTHER")] {
            s.create_meeting(&Meeting {
                id: id.into(),
                title: id.into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: id.into(),
            })
            .unwrap();
            s.set_report_settings(
                id,
                &ReportSettings {
                    cycle_id: Some(cycle.into()),
                    ..ReportSettings::default()
                },
            )
            .unwrap();
            earlier_draft(root.path(), id, id, n);
        }
        let p = cloud_preview(&s, &cfg, &this, "m1", &[]).unwrap();
        assert_eq!(p.previous_item_count, 9);
        assert!(
            p.summary_text.contains("up to 9 items"),
            "{}",
            p.summary_text
        );
        assert!(p.summary_text.contains("earlier meetings"));
        assert!(p.summary_text.contains("processed only locally"));

        // A copy of m2 in the repository is not counted twice; another meeting is.
        let repo = root.path().join("repo-meetings");
        for (folder, id, n) in [("2026-11-01", "m2", 4), ("2026-11-08", "m9", 40)] {
            let dir = repo.join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            let items: Vec<_> = (0..n).map(|i| serde_json::json!({"id": i})).collect();
            std::fs::write(
                dir.join("meeting.json"),
                serde_json::json!({"meeting_id": id, "items": items}).to_string(),
            )
            .unwrap();
        }
        let p = cloud_preview(&s, &cfg, &this, "m1", std::slice::from_ref(&repo)).unwrap();
        // 9 local + 40 from the repository, but the prompts carry at most 30.
        assert_eq!(p.previous_item_count, MAX_PREVIOUS_ITEMS_SENT);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["previousItemCount"], 30);
    }

    #[test]
    fn the_preview_without_a_transcript_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let s = store_with(Some("NOMAD"));
        let p = cloud_preview(&s, &configured(), dir.path(), "m1", &[]).unwrap();
        assert!(!p.transcript_ready);
        assert_eq!(p.word_count, 0);
    }

    fn options(kind: LlmKind, url: Option<&str>) -> LlmOptions {
        LlmOptions {
            kind,
            model_path: None,
            ctx: None,
            base_url: url.map(str::to_string),
            model: Some("m".into()),
            cloud_confirmed: false,
        }
    }

    #[test]
    fn only_remote_backends_get_a_key_and_only_the_one_saved_for_their_host() {
        let get = |name: &str| Some(format!("secret-of-{name}"));
        let managed = options(LlmKind::Managed, None);
        assert!(key_for(&managed, get).is_none());
        let ext = key_for(
            &options(LlmKind::External, Some("http://127.0.0.1:11434/v1")),
            get,
        )
        .unwrap();
        let cloud = key_for(
            &options(LlmKind::Cloud, Some("https://API.example.com:8443/v1")),
            get,
        )
        .unwrap();
        assert_eq!(
            ext.expose(),
            format!("secret-of-{}@127.0.0.1", secrets::EXTERNAL_KEY)
        );
        assert_eq!(
            cloud.expose(),
            format!("secret-of-{}@api.example.com", secrets::CLOUD_KEY)
        );
        // An absent key stays absent, and so does one for an address without a host.
        assert!(key_for(
            &options(LlmKind::Cloud, Some("https://a.example/v1")),
            |_| None
        )
        .is_none());
        assert!(key_for(&options(LlmKind::Cloud, None), get).is_none());
        assert!(key_for(&options(LlmKind::Cloud, Some("https:///v1")), get).is_none());
        // Debug output never shows it.
        assert!(!format!("{cloud:?}").contains("secret-of"));
    }

    #[test]
    fn a_key_saved_for_one_host_is_never_found_for_another() {
        // The user saved a key for provider A, then pointed the settings at provider B.
        let vault =
            |name: &str| (name == "llm-cloud-key@a.example.com").then(|| "key-a".to_string());
        let job_a = options(LlmKind::Cloud, Some("https://a.example.com/v1"));
        let job_b = options(LlmKind::Cloud, Some("https://b.example.net/v1"));
        assert_eq!(key_for(&job_a, vault).unwrap().expose(), "key-a");
        assert!(key_for(&job_b, vault).is_none(), "B must not get A's key");
        // The settings look for the key of the endpoint they hold now.
        let mut s = configured();
        s.cloud.base_url = "https://a.example.com/v1".into();
        assert_eq!(
            s.key_entry(LlmKind::Cloud).as_deref(),
            Some("llm-cloud-key@a.example.com")
        );
        s.cloud.base_url = "https://b.example.net/v1".into();
        let entry = s.key_entry(LlmKind::Cloud).unwrap();
        assert!(vault(&entry).is_none());
        assert_eq!(s.key_entry(LlmKind::Managed), None);
        // Choosing the cloud asks for the key of the current host.
        let st = store_with(Some("NOMAD"));
        let e =
            choose_llm(&st, "m1", &s, Some(LlmKind::Cloud), &|n| vault(n).is_some()).unwrap_err();
        assert_eq!(e.code, "cloud_key_missing");
        s.cloud.base_url = "https://a.example.com/v1".into();
        let e =
            choose_llm(&st, "m1", &s, Some(LlmKind::Cloud), &|n| vault(n).is_some()).unwrap_err();
        assert_eq!(e.code, "cloud_not_confirmed", "the key is there now");
    }
}
