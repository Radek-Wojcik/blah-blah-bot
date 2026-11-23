//! Secrets in the Windows Credential Manager. Configuration files hold no secret values,
//! and nothing here is ever logged or returned to the UI.

use crate::ipc::{IpcError, IpcResult};

const SERVICE: &str = "blah-blah-bot";
pub const HF_TOKEN: &str = "huggingface-token";
/// API key of the cloud language model endpoint (the entry name carries the host, see
/// [`endpoint_key`]).
pub const CLOUD_KEY: &str = "llm-cloud-key";
/// Optional API key of the external (self-hosted) language model server (entry name with the
/// host as well).
pub const EXTERNAL_KEY: &str = "llm-external-key";

/// Name of the credential entry of an endpoint key: the key's base name and the host it was
/// saved for. A key is only ever found again for that host, so changing the address in the
/// settings (or a job that keeps an older address) can never hand the key to another server.
pub fn endpoint_key(base: &str, host: &str) -> String {
    format!("{base}@{}", host.trim().to_ascii_lowercase())
}

/// Register the credential store. Call once at start; failure only disables secrets.
pub fn init() {
    #[cfg(windows)]
    match windows_native_keyring_store::Store::new() {
        Ok(store) => keyring_core::set_default_store(store),
        Err(e) => tracing::warn!(error = %e, "credential store not available"),
    }
}

fn entry(name: &str) -> IpcResult<keyring_core::Entry> {
    keyring_core::Entry::new(SERVICE, name)
        .map_err(|e| IpcError::new("secrets", format!("credential store: {e}")))
}

pub fn get(name: &str) -> Option<String> {
    entry(name).ok()?.get_password().ok()
}

pub fn set(name: &str, value: &str) -> IpcResult<()> {
    if value.trim().is_empty() {
        return Err(IpcError::new("invalid", "the value is empty"));
    }
    entry(name)?
        .set_password(value.trim())
        .map_err(|e| IpcError::new("secrets", format!("could not store the secret: {e}")))
}

pub fn clear(name: &str) -> IpcResult<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(e) => Err(IpcError::new(
            "secrets",
            format!("could not remove the secret: {e}"),
        )),
    }
}

// ----- publishing secrets ------------------------------------------------------------------

/// The GitHub token of one API host (the entry name carries the host, see
/// [`github_token_entry`]).
pub const GITHUB_TOKEN: &str = "github-token";

/// Entries of the secrets that cycle files refer to (`cred:discord/nomad`) start with this.
/// No entry of the application itself does, so a reference in a repository can only reach
/// secrets stored for that purpose, never the Hugging Face token or a model key.
pub const TARGET_PREFIX: &str = "target:";

/// Where secrets are kept. [`Credentials`] is the Windows Credential Manager; tests use an
/// in-memory store.
pub trait SecretStore {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str) -> IpcResult<()>;
    fn clear(&self, name: &str) -> IpcResult<()>;
}

/// The credential store of the machine.
pub struct Credentials;

impl SecretStore for Credentials {
    fn get(&self, name: &str) -> Option<String> {
        get(name)
    }

    fn set(&self, name: &str, value: &str) -> IpcResult<()> {
        set(name, value)
    }

    fn clear(&self, name: &str) -> IpcResult<()> {
        clear(name)
    }
}

/// The entry of the GitHub token for the API at `api_base`: the token is only found again
/// for that host, so a token saved for one server can never be sent to another.
pub fn github_token_entry(api_base: &str) -> IpcResult<String> {
    let base = publishers::github::normalize_api_base(api_base)
        .map_err(|why| IpcError::new("invalid", format!("GitHub API address: {why}")))?;
    let host = publishers::github::api_host(&base)
        .ok_or_else(|| IpcError::new("invalid", "GitHub API address has no host"))?;
    Ok(endpoint_key(GITHUB_TOKEN, &host))
}

/// A `cred:` reference as written in a cycle file: `cred:` and a name of letters, digits,
/// `_`, `.`, `/` and `-` that starts with a letter or digit (the rule of `cycle.schema.json`).
/// Returns the name.
pub fn reference_name(reference: &str) -> IpcResult<&str> {
    let invalid = || {
        IpcError::new(
            "invalid",
            "a secret reference looks like cred:discord/nomad",
        )
    };
    let name = reference.trim().strip_prefix("cred:").ok_or_else(invalid)?;
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'));
    if first_ok && rest_ok {
        Ok(name)
    } else {
        Err(invalid())
    }
}

/// The credential entry behind a `cred:` reference, in the [`TARGET_PREFIX`] namespace.
pub fn target_entry(reference: &str) -> IpcResult<String> {
    Ok(format!("{TARGET_PREFIX}{}", reference_name(reference)?))
}

/// The secret a `cred:` reference points to; `None` when the reference is malformed or
/// nothing is stored under it.
pub fn resolve_cred(store: &dyn SecretStore, reference: &str) -> Option<String> {
    store.get(&target_entry(reference).ok()?)
}

#[cfg(test)]
pub use memory::MemorySecrets;

#[cfg(test)]
mod memory {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::{IpcError, IpcResult, SecretStore};

    /// An in-memory [`SecretStore`] for tests.
    #[derive(Default)]
    pub struct MemorySecrets(Mutex<BTreeMap<String, String>>);

    impl MemorySecrets {
        pub fn names(&self) -> Vec<String> {
            self.0.lock().unwrap().keys().cloned().collect()
        }
    }

    impl SecretStore for MemorySecrets {
        fn get(&self, name: &str) -> Option<String> {
            self.0.lock().unwrap().get(name).cloned()
        }

        fn set(&self, name: &str, value: &str) -> IpcResult<()> {
            if value.trim().is_empty() {
                return Err(IpcError::new("invalid", "the value is empty"));
            }
            self.0
                .lock()
                .unwrap()
                .insert(name.to_string(), value.trim().to_string());
            Ok(())
        }

        fn clear(&self, name: &str) -> IpcResult<()> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_github_token_belongs_to_the_host_of_its_api() {
        assert_eq!(
            github_token_entry("https://api.github.com").unwrap(),
            "github-token@api.github.com"
        );
        assert_eq!(
            github_token_entry(" https://GHE.example.com/api/v3/ ").unwrap(),
            "github-token@ghe.example.com"
        );
        assert_eq!(
            github_token_entry("http://127.0.0.1:9000").unwrap(),
            "github-token@127.0.0.1"
        );
        for bad in [
            "http://api.github.com",
            "api.github.com",
            "",
            "https://u:p@x.example",
        ] {
            assert!(github_token_entry(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn references_follow_the_schema_rule() {
        assert_eq!(
            reference_name("cred:discord/nomad").unwrap(),
            "discord/nomad"
        );
        assert_eq!(reference_name(" cred:a.b_c-1 ").unwrap(), "a.b_c-1");
        for bad in [
            "discord/nomad",
            "cred:",
            "cred:/x",
            "cred:-x",
            "cred:a b",
            r"cred:a\b",
            "cred:a@b",
            "cred:ü",
            "CRED:x",
            "https://discord.com/api/webhooks/1/2",
        ] {
            assert!(reference_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_reference_can_never_reach_an_application_secret() {
        let store = MemorySecrets::default();
        store.set(HF_TOKEN, "hf-secret").unwrap();
        store
            .set(&endpoint_key(CLOUD_KEY, "api.example.com"), "cloud-secret")
            .unwrap();
        store
            .set(&endpoint_key(EXTERNAL_KEY, "127.0.0.1"), "ext-secret")
            .unwrap();
        store
            .set(
                &github_token_entry("https://api.github.com").unwrap(),
                "gh-secret",
            )
            .unwrap();
        for reference in [
            "cred:huggingface-token",
            "cred:llm-cloud-key",
            "cred:llm-cloud-key@api.example.com",
            "cred:llm-external-key",
            "cred:github-token",
            "cred:github-token/api.github.com",
            "cred:target:x",
        ] {
            assert_eq!(resolve_cred(&store, reference), None, "{reference}");
        }
        // The prefix keeps the namespaces apart whatever the names of the application are.
        for own in [HF_TOKEN, CLOUD_KEY, EXTERNAL_KEY, GITHUB_TOKEN] {
            assert!(!own.starts_with(TARGET_PREFIX));
        }
        store
            .set(&target_entry("cred:discord/nomad").unwrap(), "hook")
            .unwrap();
        assert_eq!(
            resolve_cred(&store, "cred:discord/nomad").as_deref(),
            Some("hook")
        );
        assert_eq!(resolve_cred(&store, "not a reference"), None);
    }
}
