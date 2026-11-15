//! The language-model backend of the extraction stages: settings, the per-meeting choice
//! with its persistence, and the confirmation a cloud backend needs.
//!
//! Everything here works on a `&Store` and plain values, so it is tested without Tauri. The
//! API keys never pass through this module: it only asks whether one is stored.

use std::path::Path;

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
        }
    }
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::new("invalid", message)
}

/// An endpoint URL: http or https, with a host and no credentials in it.
fn check_url(what: &str, url: &str) -> IpcResult<()> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(());
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| {
            invalid(format!(
                "{what}: the address must start with http:// or https://"
            ))
        })?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(invalid(format!(
            "{what}: do not put a user name or key into the address; keys are stored separately"
        )));
    }
    if worker::url_host(url).is_none() {
        return Err(invalid(format!("{what}: the address has no host")));
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
        check_url("external server", &self.external.base_url)?;
        check_url("cloud endpoint", &self.cloud.base_url)?;
        Ok(())
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
            check_url(kind.as_str(), url)?;
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

/// The key a stage of a job with this backend may see: the external or the cloud key from
/// the credential store, never anything for the managed backend or for no backend.
pub fn key_for(kind: Option<LlmKind>, get: impl Fn(&str) -> Option<String>) -> Option<Secret> {
    let name = match kind? {
        LlmKind::Managed => return None,
        LlmKind::External => secrets::EXTERNAL_KEY,
        LlmKind::Cloud => secrets::CLOUD_KEY,
    };
    get(name).map(Secret::new)
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
    if kind == LlmKind::Cloud && !key_present(secrets::CLOUD_KEY) {
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

/// What would be sent to the cloud endpoint for this meeting. Reads only; nothing is
/// recorded and nothing is confirmed.
pub fn cloud_preview(
    store: &Store,
    settings: &LlmSettings,
    meeting_dir: &Path,
    meeting_id: &str,
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
    let summary_text = format!(
        "This will send {amount} to {host} (model {model}), together with the names, glossary, \
         instruction and open topics of the cycle. No audio and no images are sent. Nothing is \
         sent without this confirmation."
    );
    Ok(CloudPreview {
        endpoint_host: host,
        model,
        word_count,
        screenshot_descriptions,
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
        let p = cloud_preview(&s, &cfg, dir.path(), "m1").unwrap();
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
        let after = cloud_preview(&s, &cfg, dir.path(), "m1").unwrap();
        assert!(after.word_count < p.word_count);

        confirm_cloud(&s, &cfg, "m1", 7).unwrap();
        assert!(cloud_preview(&s, &cfg, dir.path(), "m1").unwrap().confirmed);
    }

    #[test]
    fn the_preview_without_a_transcript_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let s = store_with(Some("NOMAD"));
        let p = cloud_preview(&s, &configured(), dir.path(), "m1").unwrap();
        assert!(!p.transcript_ready);
        assert_eq!(p.word_count, 0);
    }

    #[test]
    fn only_remote_backends_get_a_key_and_only_their_own() {
        let get = |name: &str| Some(format!("secret-of-{name}"));
        assert!(key_for(None, get).is_none());
        assert!(key_for(Some(LlmKind::Managed), get).is_none());
        let ext = key_for(Some(LlmKind::External), get).unwrap();
        let cloud = key_for(Some(LlmKind::Cloud), get).unwrap();
        assert_eq!(ext.expose(), format!("secret-of-{}", secrets::EXTERNAL_KEY));
        assert_eq!(cloud.expose(), format!("secret-of-{}", secrets::CLOUD_KEY));
        // An absent key stays absent.
        assert!(key_for(Some(LlmKind::Cloud), |_| None).is_none());
        // Debug output never shows it.
        assert!(!format!("{cloud:?}").contains("secret-of"));
    }
}
