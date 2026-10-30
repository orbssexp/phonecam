//! Mini signaling relay for tests: same wire behaviour as spike/relay (two roles per room,
//! `{"t":"peer","up":bool}` presence), optionally answering the desktop's challenge as a paired
//! phone would (for the Edge page, which predates pairing).
#![allow(dead_code, clippy::result_large_err)]
use futures_util::{SinkExt, StreamExt};
use phonecam_core::pairing;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message as M;

pub const SECRET: &[u8] = b"0123456789abcdef";
pub const ROOM: &str = "e2eRoomIdeRoomIdeRoomId_1";

#[derive(Default)]
struct Room {
    desktop: Option<UnboundedSender<String>>,
    phone: Option<UnboundedSender<String>>,
}

#[derive(Default, Clone)]
pub struct Seen {
    pub offer_has_rtx: Arc<AtomicBool>,
    pub offers: Arc<Mutex<Vec<String>>>,
}

pub async fn serve_relay(listener: TcpListener, seen: Seen, auto_hello: bool) {
    let room = Arc::new(Mutex::new(Room::default()));
    loop {
        let Ok((tcp, _)) = listener.accept().await else { return };
        let (room, seen) = (room.clone(), seen.clone());
        tokio::spawn(async move {
            let mut role = String::new();
            let cb = |req: &Request, resp: Response| {
                role = req.uri().query().unwrap_or("").split('&').find_map(|kv| kv.strip_prefix("role=")).unwrap_or("").to_string();
                Ok(resp)
            };
            let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else { return };
            let (mut sink, mut stream) = ws.split();
            let (tx, mut rx) = unbounded_channel::<String>();
            tokio::spawn(async move {
                while let Some(t) = rx.recv().await {
                    if sink.send(M::Text(t.into())).await.is_err() {
                        return;
                    }
                }
            });
            let other_of = |room: &Room, role: &str| if role == "desktop" { room.phone.clone() } else { room.desktop.clone() };
            {
                let mut r = room.lock().unwrap();
                let other = other_of(&r, &role);
                let _ = tx.send(format!(r#"{{"t":"peer","up":{}}}"#, other.is_some()));
                if let Some(o) = other {
                    let _ = o.send(r#"{"t":"peer","up":true}"#.into());
                }
                if role == "desktop" { r.desktop = Some(tx.clone()) } else { r.phone = Some(tx.clone()) }
            }
            while let Some(Ok(M::Text(t))) = stream.next().await {
                let t = t.to_string();
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                let other = other_of(&room.lock().unwrap(), &role);
                if role == "phone" && v["type"] == "offer" {
                    let sdp = v["sdp"].as_str().unwrap_or("");
                    seen.offer_has_rtx.store(sdp.contains("rtx/90000"), Ordering::Relaxed);
                    seen.offers.lock().unwrap().push(sdp.lines().find(|l| l.starts_with("m=video")).unwrap_or("").to_string());
                }
                // The Edge page predates pairing: answer the desktop's challenge as the phone would.
                if auto_hello && role == "desktop" && v["type"] == "challenge" {
                    let nonce = v["nonce"].as_str().unwrap_or("");
                    let hello = serde_json::json!({"type":"hello","token":null,"role":"phone","version":1,"proof":pairing::proof_hex(SECRET, nonce.as_bytes())});
                    let d = room.lock().unwrap().desktop.clone();
                    if let Some(d) = d {
                        let _ = d.send(hello.to_string());
                    }
                    continue;
                }
                if let Some(o) = other {
                    let _ = o.send(t);
                }
            }
            let mut r = room.lock().unwrap();
            let other = other_of(&r, &role);
            if role == "desktop" { r.desktop = None } else { r.phone = None }
            if let Some(o) = other {
                let _ = o.send(r#"{"t":"peer","up":false}"#.into());
            }
        });
    }
}

