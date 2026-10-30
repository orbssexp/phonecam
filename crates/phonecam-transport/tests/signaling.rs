//! Pairing, signaling and state-machine events against a fake phone on a local mini relay.
//! No browser. Needs no LAN interface for the assertions below (the answer lists whatever is bound).
mod common;
use common::{serve_relay, Seen, ROOM, SECRET};
use futures_util::{SinkExt, StreamExt};
use phonecam_core::{pairing, VcamFrame};
use phonecam_transport::{Config, Receiver, TransportEvent};
use std::time::Duration;
use str0m::media::{Direction, MediaKind};
use str0m::Rtc;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as M;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_json(ws: &mut Ws, ms: u64) -> Option<serde_json::Value> {
    loop {
        match tokio::time::timeout(Duration::from_millis(ms), ws.next()).await {
            Ok(Some(Ok(M::Text(t)))) => return serde_json::from_str(&t).ok(),
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

async fn next_event(ev: &mut tokio::sync::mpsc::Receiver<TransportEvent>, ms: u64) -> Option<TransportEvent> {
    tokio::time::timeout(Duration::from_millis(ms), ev.recv()).await.ok().flatten()
}

fn browser_like_offer() -> String {
    str0m::crypto::from_feature_flags().install_process_default();
    let mut a = Rtc::builder().build(std::time::Instant::now());
    a.add_local_candidate(str0m::Candidate::host("10.9.9.9:4000".parse().unwrap(), "udp").unwrap());
    let mut api = a.sdp_api();
    api.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    api.add_channel("ctl".into());
    api.apply().unwrap().0.to_sdp_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_gates_the_offer_and_events_follow_the_phone() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(serve_relay(listener, Seen::default(), false));
    let cfg = Config { relay_url: format!("ws://127.0.0.1:{port}"), page_url: String::new(), room_id: ROOM.into(), secret: SECRET.to_vec() };
    let (h, mut ev) = Receiver::start(cfg);
    assert_eq!(next_event(&mut ev, 2000).await, Some(TransportEvent::Vcam(VcamFrame::Placeholder)));

    // Give the desktop a moment to reach the relay, then join as the phone.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/room/{ROOM}?role=phone")).await.unwrap();
    assert_eq!(next_event(&mut ev, 3000).await, Some(TransportEvent::PhoneJoined));

    // presence, then the challenge
    let mut nonce = String::new();
    for _ in 0..4 {
        match next_json(&mut ws, 2000).await {
            Some(v) if v["type"] == "challenge" => {
                nonce = v["nonce"].as_str().unwrap().to_string();
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert_eq!(nonce.len(), 32, "no challenge received");

    // Wrong proof -> bye, and the following offer is ignored.
    let bad = serde_json::json!({"type":"hello","token":null,"role":"phone","version":1,"proof":pairing::proof_hex(b"wrong-secret-123", nonce.as_bytes())});
    ws.send(M::Text(bad.to_string().into())).await.unwrap();
    let bye = next_json(&mut ws, 2000).await.expect("bye after a bad proof");
    assert_eq!(bye["type"], "bye");
    let offer = serde_json::json!({"type":"offer","sdp":browser_like_offer()});
    ws.send(M::Text(offer.to_string().into())).await.unwrap();
    assert!(next_json(&mut ws, 800).await.is_none(), "unpaired phone must not get an answer");

    // Right proof -> offer is answered.
    let good = serde_json::json!({"type":"hello","token":null,"role":"phone","version":1,"proof":pairing::proof_hex(SECRET, nonce.as_bytes())});
    ws.send(M::Text(good.to_string().into())).await.unwrap();
    ws.send(M::Text(offer.to_string().into())).await.unwrap();
    let ans = next_json(&mut ws, 3000).await.expect("answer after a good proof");
    assert_eq!(ans["type"], "answer");
    let sdp = ans["sdp"].as_str().unwrap();
    assert!(sdp.contains("a=ice-lite"), "desktop must be ICE-lite");
    assert!(sdp.contains("rtx/90000") && sdp.contains("a=rtcp-fb:") && sdp.contains(" nack"), "answer must keep NACK + RTX");

    // restart_ice goes through the relay while no DataChannel is open.
    h.restart_ice();
    let r = next_json(&mut ws, 2000).await.expect("restart_ice on the relay");
    assert_eq!(r["type"], "restart_ice");

    // Phone leaves with no media flowing -> vcam shows Reconnecting.
    ws.close(None).await.unwrap();
    let mut saw_left = false;
    let mut saw_reconnecting = false;
    for _ in 0..8 {
        match next_event(&mut ev, 2000).await {
            Some(TransportEvent::PhoneLeft) => saw_left = true,
            Some(TransportEvent::Vcam(VcamFrame::Reconnecting)) => saw_reconnecting = true,
            Some(_) => {}
            None => break,
        }
        if saw_left && saw_reconnecting {
            break;
        }
    }
    assert!(saw_left && saw_reconnecting, "left={saw_left} reconnecting={saw_reconnecting}");
    h.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_reconnects_to_a_relay_that_starts_late() {
    // Reserve a port, start the receiver first, bring the relay up 1.5 s later.
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let cfg = Config { relay_url: format!("ws://127.0.0.1:{port}"), page_url: String::new(), room_id: ROOM.into(), secret: SECRET.to_vec() };
    let (h, mut ev) = Receiver::start(cfg);
    assert_eq!(next_event(&mut ev, 2000).await, Some(TransportEvent::Vcam(VcamFrame::Placeholder)));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    tokio::spawn(serve_relay(listener, Seen::default(), false));
    // Backoff is 1 s, then 2 s: the phone must be able to meet the desktop within a few seconds.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (_ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/room/{ROOM}?role=phone")).await.unwrap();
    assert_eq!(next_event(&mut ev, 4000).await, Some(TransportEvent::PhoneJoined));
    h.shutdown();
}
