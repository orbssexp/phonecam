//! Regression: wss:// must not panic for lack of a rustls crypto provider.
//! Needs network: `cargo test -p phonecam-transport --test wss_live -- --ignored`.

#[test]
#[ignore]
fn live_relay_tls_handshake() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let url = "wss://phonecam-relay.phonecam-relay.workers.dev/room/tlscheck?role=desktop";
    let r = rt.block_on(tokio_tungstenite::connect_async(url));
    // Any HTTP-level answer proves TLS worked; only a TLS error is a failure.
    if let Err(e) = &r {
        assert!(!format!("{e}").to_lowercase().contains("tls"), "tls error: {e}");
    }
}
