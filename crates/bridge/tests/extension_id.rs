//! The extension id the application accepts follows from the public key in the extension's
//! manifest: Chrome takes the first 16 bytes of the SHA-256 of the DER key and writes every
//! nibble as a letter from `a` to `p`. If the key or the constant changes alone, the browser
//! and the application stop agreeing and nothing can pair.

use sha2::{Digest, Sha256};

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a') + 26,
        b'0'..=b'9' => u32::from(c - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {}", c as char),
    };
    let digits: Vec<u32> = text.bytes().filter(|c| *c != b'=').map(value).collect();
    let mut out = Vec::new();
    for chunk in digits.chunks(4) {
        let mut acc = 0u32;
        for (i, d) in chunk.iter().enumerate() {
            acc |= d << (18 - 6 * i);
        }
        let bytes = chunk.len() * 6 / 8;
        out.extend_from_slice(&acc.to_be_bytes()[1..1 + bytes]);
    }
    out
}

#[test]
fn extension_id_matches_the_manifest_key() {
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../extension/manifest.json"
    ))
    .expect("extension/manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(&manifest).expect("valid JSON");
    let key = manifest["key"].as_str().expect("manifest has a key");
    let digest = Sha256::digest(base64_decode(key));
    let id: String = digest[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 15])
        .map(|n| char::from(b'a' + n))
        .collect();
    assert_eq!(id, bridge::server::EXTENSION_ID);
}
