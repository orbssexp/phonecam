//! Pairing proof (spec §14a): the desktop sends a nonce; the phone answers with
//! HMAC-SHA256(secret, DOMAIN || nonce). The secret only travels in the QR URL fragment.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const DOMAIN: &[u8] = b"phonecam-pair-v1";

fn mac(secret: &[u8], nonce: &[u8]) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    m.update(DOMAIN);
    m.update(nonce);
    m
}

pub fn proof_hex(secret: &[u8], nonce: &[u8]) -> String {
    mac(secret, nonce).finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time check. Returns false for any malformed input.
pub fn verify_hex(secret: &[u8], nonce: &[u8], candidate: &str) -> bool {
    match decode_hex(candidate) {
        Some(bytes) => mac(secret, nonce).verify_slice(&bytes).is_ok(),
        None => false,
    }
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: &[u8] = b"0123456789abcdef";
    const N: &[u8] = b"nonce-1";

    #[test]
    fn proof_verifies_with_same_secret_and_nonce() {
        let p = proof_hex(K, N);
        assert_eq!(p.len(), 64);
        assert!(verify_hex(K, N, &p));
    }

    #[test]
    fn wrong_secret_or_nonce_fails() {
        let p = proof_hex(K, N);
        assert!(!verify_hex(b"fedcba9876543210", N, &p));
        assert!(!verify_hex(K, b"nonce-2", &p));
    }

    #[test]
    fn malformed_or_truncated_proof_fails() {
        let p = proof_hex(K, N);
        assert!(!verify_hex(K, N, &p[..62]));
        assert!(!verify_hex(K, N, "zz"));
        assert!(!verify_hex(K, N, ""));
    }

    #[test]
    fn non_hex_characters_are_rejected() {
        // u8::from_str_radix accepts a leading '+'; a proof must have one canonical encoding.
        let p = proof_hex(K, N);
        let plus = format!("+{}", &p[1..]);
        assert!(!verify_hex(K, N, &plus));
    }

    #[test]
    fn proof_is_deterministic() {
        assert_eq!(proof_hex(K, N), proof_hex(K, N));
    }
}
