//! Pairing: one-time codes shown in the application and the client secrets that replace them.
//!
//! The user starts pairing in the application, which shows an 8-character code (Crockford
//! base32, 40 bits). The code is valid for five minutes, works once and is locked after five
//! wrong attempts. The extension that presents it receives a client id and a 32-byte random
//! secret; the application keeps only the SHA-256 of the secret. Later connections
//! authenticate with the id and the secret. Unpairing removes the client.
//!
//! Time is passed in by the caller, so the state machine is tested without waiting.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Characters of a pairing code (Crockford base32: no I, L, O, U).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Characters in a pairing code.
pub const CODE_LEN: usize = 8;
/// How long a code is valid.
pub const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// Wrong attempts after which a code stops working.
pub const MAX_ATTEMPTS: u32 = 5;
/// Bytes of a client secret.
pub const SECRET_BYTES: usize = 32;

/// The operating system could not provide random numbers.
#[derive(Debug, thiserror::Error)]
#[error("no random numbers available: {0}")]
pub struct RandomError(String);

/// `N` random bytes from the operating system.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|e| RandomError(e.to_string()))?;
    Ok(out)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new random pairing code of [`CODE_LEN`] characters.
pub fn new_code() -> Result<String, RandomError> {
    let bytes = random_bytes::<5>()?;
    let mut bits: u64 = 0;
    for b in bytes {
        bits = (bits << 8) | u64::from(b);
    }
    Ok((0..CODE_LEN)
        .rev()
        .map(|i| ALPHABET[((bits >> (i * 5)) & 31) as usize] as char)
        .collect())
}

/// The code as typed by a person, in canonical form: case and separators (`-`, spaces) are
/// ignored and the letters Crockford base32 treats as look-alikes (I, L, O) are mapped to
/// digits. `None` when it cannot be a code.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' => continue,
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// The code split in two halves for display: `7K3M-9QXD`.
pub fn display_code(code: &str) -> String {
    if code.len() == CODE_LEN {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code.to_string()
    }
}

/// Comparison whose duration does not depend on where the inputs differ.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A new client secret: [`SECRET_BYTES`] random bytes as lowercase hex.
pub fn new_secret() -> Result<String, RandomError> {
    Ok(to_hex(&random_bytes::<SECRET_BYTES>()?))
}

/// A new client id: `c-` and 16 hex characters.
pub fn new_client_id() -> Result<String, RandomError> {
    Ok(format!("c-{}", to_hex(&random_bytes::<8>()?)))
}

/// SHA-256 of a secret as lowercase hex: what the application stores.
pub fn hash_secret(secret: &str) -> String {
    to_hex(&Sha256::digest(secret.as_bytes()))
}

/// True when `secret` hashes to `stored_hash`.
pub fn secret_matches(secret: &str, stored_hash: &str) -> bool {
    same_bytes(hash_secret(secret).as_bytes(), stored_hash.as_bytes())
}

/// A paired extension as stored in the application settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedClient {
    pub id: String,
    /// What the client called itself at pairing, for example `Chrome 136`.
    pub name: String,
    pub ext_version: String,
    /// SHA-256 of the secret, lowercase hex. The secret itself is never stored.
    pub secret_sha256: String,
    pub paired_unix_ms: u64,
}

/// Why a code was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    /// Wrong code, or no pairing was started.
    #[error("the pairing code is not correct")]
    BadCode,
    /// The code expired or was locked after too many wrong attempts.
    #[error("the pairing code expired")]
    Expired,
}

#[derive(Debug, Clone)]
struct Pending {
    code: String,
    expires: Instant,
    failures: u32,
    locked: bool,
}

/// The pairing code currently offered, if any.
#[derive(Debug, Default)]
pub struct Pairing {
    pending: Option<Pending>,
}

impl Pairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer a new random code (a previous one stops working).
    pub fn start(&mut self, now: Instant) -> Result<String, RandomError> {
        let code = new_code()?;
        self.start_with(code.clone(), now);
        Ok(code)
    }

    /// Offer a given code. Used by the test receiver and tests.
    pub fn start_with(&mut self, code: String, now: Instant) {
        self.pending = Some(Pending {
            code,
            expires: now + CODE_TTL,
            failures: 0,
            locked: false,
        });
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// The code that can be used now, with its expiry.
    pub fn active(&self, now: Instant) -> Option<(&str, Instant)> {
        self.pending
            .as_ref()
            .filter(|p| !p.locked && now < p.expires)
            .map(|p| (p.code.as_str(), p.expires))
    }

    /// True when a code was offered but can no longer be used (expired or locked).
    pub fn is_stale(&self, now: Instant) -> bool {
        self.pending.is_some() && self.active(now).is_none()
    }

    /// Check a code presented by an extension. A correct code is used up.
    pub fn claim(&mut self, input: &str, now: Instant) -> Result<(), ClaimError> {
        let Some(p) = self.pending.as_mut() else {
            return Err(ClaimError::BadCode);
        };
        if p.locked || now >= p.expires {
            return Err(ClaimError::Expired);
        }
        let ok = normalize_code(input).is_some_and(|c| same_bytes(c.as_bytes(), p.code.as_bytes()));
        if ok {
            self.pending = None;
            return Ok(());
        }
        p.failures += 1;
        if p.failures >= MAX_ATTEMPTS {
            p.locked = true;
        }
        Err(ClaimError::BadCode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_eight_crockford_characters() {
        for _ in 0..50 {
            let c = new_code().unwrap();
            assert_eq!(c.len(), CODE_LEN);
            assert!(c.bytes().all(|b| ALPHABET.contains(&b)), "{c}");
            assert_eq!(normalize_code(&c).as_deref(), Some(c.as_str()));
        }
        assert_ne!(new_code().unwrap(), new_code().unwrap());
    }

    #[test]
    fn typed_codes_are_normalized() {
        assert_eq!(normalize_code("7k3m-9qxd").as_deref(), Some("7K3M9QXD"));
        assert_eq!(normalize_code(" 7K3M 9QXD ").as_deref(), Some("7K3M9QXD"));
        assert_eq!(normalize_code("IL0O-0000").as_deref(), Some("11000000"));
        assert_eq!(normalize_code("7K3M9QX"), None);
        assert_eq!(normalize_code("7K3M9QXU"), None);
        assert_eq!(normalize_code("7K3M9QXDD"), None);
        assert_eq!(display_code("7K3M9QXD"), "7K3M-9QXD");
    }

    #[test]
    fn a_correct_code_works_once() {
        let t0 = Instant::now();
        let mut p = Pairing::new();
        p.start_with("7K3M9QXD".into(), t0);
        assert_eq!(p.active(t0).map(|(c, _)| c), Some("7K3M9QXD"));
        assert_eq!(p.claim("7k3m-9qxd", t0), Ok(()));
        assert!(p.active(t0).is_none());
        assert_eq!(p.claim("7K3M9QXD", t0), Err(ClaimError::BadCode));
    }

    #[test]
    fn a_code_expires_after_five_minutes() {
        let t0 = Instant::now();
        let mut p = Pairing::new();
        p.start_with("7K3M9QXD".into(), t0);
        let almost = t0 + CODE_TTL - Duration::from_millis(1);
        assert!(p.active(almost).is_some());
        assert!(!p.is_stale(almost));
        let late = t0 + CODE_TTL;
        assert!(p.active(late).is_none());
        assert!(p.is_stale(late));
        assert_eq!(p.claim("7K3M9QXD", late), Err(ClaimError::Expired));
    }

    #[test]
    fn five_wrong_attempts_lock_the_code() {
        let t0 = Instant::now();
        let mut p = Pairing::new();
        p.start_with("7K3M9QXD".into(), t0);
        for _ in 0..MAX_ATTEMPTS - 1 {
            assert_eq!(p.claim("00000000", t0), Err(ClaimError::BadCode));
        }
        // Still usable after four wrong attempts.
        assert!(p.active(t0).is_some());
        assert_eq!(p.claim("garbage", t0), Err(ClaimError::BadCode));
        // The fifth wrong attempt locked it: even the right code is refused now.
        assert!(p.active(t0).is_none());
        assert_eq!(p.claim("7K3M9QXD", t0), Err(ClaimError::Expired));
        // A new pairing starts from scratch.
        p.start_with("AAAABBBB".into(), t0);
        assert_eq!(p.claim("AAAABBBB", t0), Ok(()));
    }

    #[test]
    fn without_a_pairing_every_code_is_wrong() {
        let mut p = Pairing::new();
        assert_eq!(
            p.claim("7K3M9QXD", Instant::now()),
            Err(ClaimError::BadCode)
        );
        p.start_with("7K3M9QXD".into(), Instant::now());
        p.cancel();
        assert_eq!(
            p.claim("7K3M9QXD", Instant::now()),
            Err(ClaimError::BadCode)
        );
    }

    #[test]
    fn a_new_code_replaces_the_old_one() {
        let t0 = Instant::now();
        let mut p = Pairing::new();
        let first = p.start(t0).unwrap();
        let second = p.start(t0).unwrap();
        if first != second {
            assert_eq!(p.claim(&first, t0), Err(ClaimError::BadCode));
        }
        assert_eq!(p.claim(&second, t0), Ok(()));
    }

    #[test]
    fn secrets_are_stored_as_hashes() {
        let s = new_secret().unwrap();
        assert_eq!(s.len(), SECRET_BYTES * 2);
        let h = hash_secret(&s);
        assert_eq!(h.len(), 64);
        assert_ne!(h, s);
        assert!(secret_matches(&s, &h));
        assert!(!secret_matches(&new_secret().unwrap(), &h));
        assert!(!secret_matches(&s, ""));
        // A known vector: SHA-256 of "abc".
        assert_eq!(
            hash_secret("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let id = new_client_id().unwrap();
        assert!(id.starts_with("c-") && id.len() == 18 && crate::protocol::valid_id(&id));
    }
}
