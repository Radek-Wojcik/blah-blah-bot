//! Secrets in the Windows Credential Manager. Configuration files hold no secret values,
//! and nothing here is ever logged or returned to the UI.

use crate::ipc::{IpcError, IpcResult};

const SERVICE: &str = "blah-blah-bot";
pub const HF_TOKEN: &str = "huggingface-token";

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
