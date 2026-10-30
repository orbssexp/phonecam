//! Parsing of relay / DataChannel text and the pairing check (spec 14, 14a). Pure functions.
use phonecam_core::{pairing, Ctl, Role, Signal, PROTOCOL_VERSION};

#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// Relay presence: `{"t":"peer","up":bool}`.
    Peer { up: bool },
    Signal(Signal),
    Ctl(Ctl),
    /// Unknown or malformed (e.g. the spike page's `log` and `caps` messages). Ignored.
    Ignored,
}

pub fn parse_relay(text: &str) -> Incoming {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Incoming::Ignored };
    if v.get("t").and_then(|t| t.as_str()) == Some("peer") {
        return Incoming::Peer { up: v.get("up").and_then(|u| u.as_bool()).unwrap_or(false) };
    }
    if let Ok(s) = serde_json::from_value::<Signal>(v) {
        return Incoming::Signal(s);
    }
    Incoming::Ignored
}

/// DataChannel text is either a control message or a signaling message.
pub fn parse_channel(text: &str) -> Incoming {
    if let Ok(c) = serde_json::from_str::<Ctl>(text) {
        return Incoming::Ctl(c);
    }
    if let Ok(s) = serde_json::from_str::<Signal>(text) {
        return Incoming::Signal(s);
    }
    Incoming::Ignored
}

/// True only for a phone `hello` of the right protocol version whose proof matches `nonce`.
pub fn hello_ok(secret: &[u8], nonce: &str, msg: &Signal) -> bool {
    match msg {
        Signal::Hello { role: Role::Phone, version, proof, .. } if *version == PROTOCOL_VERSION => {
            pairing::verify_hex(secret, nonce.as_bytes(), proof)
        }
        _ => false,
    }
}

pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("protocol types always serialize")
}

pub fn new_nonce() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("os rng");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `a=fingerprint:` value of an SDP, used to tell "same peer re-offering" from "new page load".
pub fn fingerprint(sdp: &str) -> Option<String> {
    sdp.lines().find_map(|l| l.trim().strip_prefix("a=fingerprint:").map(|f| f.trim().to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: &[u8] = b"0123456789abcdef";

    #[test]
    fn relay_presence_and_signals_are_told_apart() {
        assert_eq!(parse_relay(r#"{"t":"peer","up":true}"#), Incoming::Peer { up: true });
        assert_eq!(parse_relay(r#"{"t":"peer","up":false}"#), Incoming::Peer { up: false });
        assert_eq!(parse_relay(r#"{"type":"offer","sdp":"v=0"}"#), Incoming::Signal(Signal::Offer { sdp: "v=0".into() }));
        assert_eq!(parse_relay(r#"{"type":"log","msg":"x"}"#), Incoming::Ignored);
        assert_eq!(parse_relay("not json"), Incoming::Ignored);
    }

    #[test]
    fn channel_text_is_ctl_first_then_signal() {
        assert_eq!(parse_channel(r#"{"type":"ping","t":3}"#), Incoming::Ctl(Ctl::Ping { t: 3 }));
        assert_eq!(parse_channel(r#"{"type":"restart_ice"}"#), Incoming::Signal(Signal::RestartIce));
        assert_eq!(parse_channel(r#"{"type":"caps","caps":{}}"#), Incoming::Ignored);
    }

    fn hello(proof: String, role: Role, version: u32) -> Signal {
        Signal::Hello { token: None, role, version, proof }
    }

    #[test]
    fn good_proof_is_accepted() {
        let p = pairing::proof_hex(K, b"n1");
        assert!(hello_ok(K, "n1", &hello(p, Role::Phone, PROTOCOL_VERSION)));
    }

    #[test]
    fn bad_proof_wrong_nonce_role_or_version_is_rejected() {
        let p = pairing::proof_hex(K, b"n1");
        assert!(!hello_ok(K, "n2", &hello(p.clone(), Role::Phone, PROTOCOL_VERSION)));
        assert!(!hello_ok(b"another-secret!!", "n1", &hello(p.clone(), Role::Phone, PROTOCOL_VERSION)));
        assert!(!hello_ok(K, "n1", &hello(p.clone(), Role::Desktop, PROTOCOL_VERSION)));
        assert!(!hello_ok(K, "n1", &hello(p, Role::Phone, PROTOCOL_VERSION + 1)));
        assert!(!hello_ok(K, "n1", &Signal::RestartIce));
    }

    #[test]
    fn fingerprint_is_extracted_case_insensitively() {
        assert_eq!(fingerprint("v=0\r\na=fingerprint:sha-256 AB:CD\r\n").as_deref(), Some("sha-256 ab:cd"));
        assert_eq!(fingerprint("v=0\r\n"), None);
    }

    #[test]
    fn nonces_differ() {
        assert_ne!(new_nonce(), new_nonce());
        assert_eq!(new_nonce().len(), 32);
    }
}
