//! Relay WebSocket client (ported from the spike's `relay_loop`). Connects as `role=desktop`,
//! forwards inbound text to the engine, sends whatever the engine queues, and reconnects with
//! the core backoff (1 s .. 30 s) forever until told to stop (spec 7.1, 7.2).
use crate::engine::{Msg, RelayIn};
use futures_util::{SinkExt, StreamExt};
use phonecam_core::backoff::Backoff;
use std::sync::mpsc::Sender;
use std::time::Duration;
use tokio::sync::{mpsc::UnboundedReceiver, watch};
use tokio_tungstenite::tungstenite::Message as M;

pub(crate) fn ws_url(relay_url: &str, room_id: &str) -> String {
    format!("{}/room/{}?role=desktop", relay_url.trim_end_matches('/'), room_id)
}

pub(crate) fn spawn(
    url: String,
    to_engine: Sender<Msg>,
    out_rx: UnboundedReceiver<String>,
    stop: watch::Receiver<bool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // wss:// needs a process-wide rustls crypto provider; without it the first TLS connect panics.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("[relay] cannot start runtime: {e}");
                return;
            }
        };
        rt.block_on(run(url, to_engine, out_rx, stop));
    })
}

async fn run(url: String, to_engine: Sender<Msg>, mut out_rx: UnboundedReceiver<String>, mut stop: watch::Receiver<bool>) {
    let mut backoff = Backoff::new(1_000, 30_000);
    loop {
        if *stop.borrow() {
            return;
        }
        let conn = tokio::select! {
            r = tokio_tungstenite::connect_async(url.as_str()) => r,
            _ = stop.changed() => return,
        };
        match conn {
            Ok((ws, _)) => {
                tracing::info!("[relay] connected");
                backoff.reset();
                // Anything queued while we were offline is stale (e.g. a challenge for a peer that left).
                while out_rx.try_recv().is_ok() {}
                if to_engine.send(Msg::Relay(RelayIn::Up)).is_err() {
                    return;
                }
                let (mut sink, mut stream) = ws.split();
                loop {
                    tokio::select! {
                        m = stream.next() => match m {
                            Some(Ok(M::Text(t))) => {
                                if to_engine.send(Msg::Relay(RelayIn::Text(t.to_string()))).is_err() {
                                    return;
                                }
                            }
                            Some(Ok(M::Close(_))) | Some(Err(_)) | None => break,
                            Some(Ok(_)) => {}
                        },
                        o = out_rx.recv() => match o {
                            Some(t) => {
                                if sink.send(M::Text(t.into())).await.is_err() {
                                    break;
                                }
                            }
                            None => return,
                        },
                        _ = stop.changed() => {
                            let _ = sink.close().await;
                            return;
                        }
                    }
                }
                tracing::info!("[relay] disconnected");
                if to_engine.send(Msg::Relay(RelayIn::Down)).is_err() {
                    return;
                }
            }
            Err(e) => tracing::warn!("[relay] connect failed: {e}"),
        }
        let wait = backoff.next_delay_ms();
        tracing::info!("[relay] retry in {wait} ms");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
            _ = stop.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_has_room_and_desktop_role() {
        assert_eq!(ws_url("wss://r.example/", "abc"), "wss://r.example/room/abc?role=desktop");
        assert_eq!(ws_url("ws://127.0.0.1:8787", "abc"), "ws://127.0.0.1:8787/room/abc?role=desktop");
    }
}
