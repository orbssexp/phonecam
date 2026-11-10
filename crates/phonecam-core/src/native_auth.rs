//! Native sender handshake (contracts "Round H", wire protocol native v1). Pure functions: the
//! Windows receiver and the iOS app both derive the same service id and HMAC proofs from `k`.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const ID_DOMAIN: &[u8] = b"phonecam-id-v1";
const PHONE_PREFIX: &str = "phonecam-native-v1|phone|";
const DESKTOP_PREFIX: &str = "phonecam-native-v1|desktop|";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn mac(k: &[u8], prefix: &str, first: &str, second: &str) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(k).expect("HMAC accepts any key length");
    m.update(prefix.as_bytes());
    m.update(first.as_bytes());
    m.update(second.as_bytes());
    m
}

/// mDNS TXT `id`: first 16 hex chars of SHA-256("phonecam-id-v1" || k).
pub fn service_id(k: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(ID_DOMAIN);
    h.update(k);
    hex(&h.finalize()[..8])
}

/// Phone's proof: HMAC(k, "phonecam-native-v1|phone|" + pc_nonce + phone_nonce), hex.
pub fn phone_proof(k: &[u8], pc_nonce: &str, phone_nonce: &str) -> String {
    hex(&mac(k, PHONE_PREFIX, pc_nonce, phone_nonce).finalize().into_bytes())
}

/// Desktop's proof: HMAC(k, "phonecam-native-v1|desktop|" + phone_nonce + pc_nonce), hex.
pub fn desktop_proof(k: &[u8], phone_nonce: &str, pc_nonce: &str) -> String {
    hex(&mac(k, DESKTOP_PREFIX, phone_nonce, pc_nonce).finalize().into_bytes())
}

/// Constant-time check of the phone's proof. False for any malformed input.
pub fn verify_phone_proof(k: &[u8], pc_nonce: &str, phone_nonce: &str, candidate: &str) -> bool {
    decode_hex(candidate).is_some_and(|b| mac(k, PHONE_PREFIX, pc_nonce, phone_nonce).verify_slice(&b).is_ok())
}

/// Constant-time check of the desktop's proof (phone side; kept here so tests cover both).
pub fn verify_desktop_proof(k: &[u8], phone_nonce: &str, pc_nonce: &str, candidate: &str) -> bool {
    decode_hex(candidate).is_some_and(|b| mac(k, DESKTOP_PREFIX, phone_nonce, pc_nonce).verify_slice(&b).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// k = 00 01 02 ... 1f
    fn k() -> Vec<u8> {
        (0u8..32).collect()
    }
    const PC: &str = "000102030405060708090a0b0c0d0e0f";
    const PH: &str = "f0e1d2c3b4a5968778695a4b3c2d1e0f";

    #[test]
    fn fixed_vectors() {
        assert_eq!(service_id(&k()), "2bbbe2d8a20e5e1a");
        assert_eq!(phone_proof(&k(), PC, PH), "4b6cd30259cdb4f6b96a3cf1182ea0bccc8490990d88ba3be733d781aab4d4e1");
        assert_eq!(desktop_proof(&k(), PH, PC), "f51ff1c09a94749e1864debc1047e5fc55e7c5109055c954df8c1b41e87f818f");
    }

    #[test]
    fn proofs_verify_and_are_not_interchangeable() {
        let p = phone_proof(&k(), PC, PH);
        assert!(verify_phone_proof(&k(), PC, PH, &p));
        assert!(!verify_desktop_proof(&k(), PH, PC, &p));
        assert!(!verify_phone_proof(&k(), PH, PC, &p));
        assert!(!verify_phone_proof(&[9u8; 32], PC, PH, &p));
    }

    #[test]
    fn malformed_proofs_fail() {
        let p = phone_proof(&k(), PC, PH);
        assert!(!verify_phone_proof(&k(), PC, PH, &p[..62]));
        assert!(!verify_phone_proof(&k(), PC, PH, "zz"));
        assert!(!verify_phone_proof(&k(), PC, PH, ""));
        let mut bad = p.clone();
        bad.replace_range(0..1, if p.starts_with('0') { "1" } else { "0" });
        assert!(!verify_phone_proof(&k(), PC, PH, &bad));
    }
}
