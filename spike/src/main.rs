mod certs;
mod decode;
mod metrics;
mod rtc;
mod y4m;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

struct App {
    tx: mpsc::Sender<rtc::Msg>,
    sid: AtomicU64,
    loss: Arc<AtomicU32>,
    ips: Vec<Ipv4Addr>,
    mobileconfig: String,
    ca_pem: String,
}

fn private_ips() -> Vec<Ipv4Addr> {
    let mut v = vec![];
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if let std::net::IpAddr::V4(ip) = i.ip() {
            if i.is_loopback() || ip.is_link_local() {
                continue;
            }
            if ip.is_private() {
                println!("[net] interface {} -> {ip}", i.name);
                v.push(ip);
            }
        }
    }
    v
}

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|x| x == name)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Some(p) = arg("--gen-y4m") {
        y4m::generate(&p, 90)?;
        println!("wrote {p}");
        return Ok(());
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    str0m::crypto::from_feature_flags().install_process_default();

    let decoder = arg("--decoder").unwrap_or_else(|| "auto".into());
    let vcam = !flag("--no-vcam");
    let metrics_path = arg("--metrics").unwrap_or_else(|| "metrics.csv".into());
    let label = arg("--label").unwrap_or_else(|| decoder.clone());
    let run_secs: Option<u64> = arg("--secs").and_then(|s| s.parse().ok());
    let loss_pm: u32 = arg("--loss-permille").and_then(|s| s.parse().ok()).unwrap_or(0);
    let http_port: u16 = arg("--http-port").and_then(|s| s.parse().ok()).unwrap_or(8080);
    let https_port: u16 = arg("--https-port").and_then(|s| s.parse().ok()).unwrap_or(8443);

    let mut ips = private_ips();
    if let Some(only) = arg("--only-ip") {
        let only: Ipv4Addr = only.parse()?;
        ips.retain(|i| *i == only);
    }
    anyhow::ensure!(!ips.is_empty(), "no private IPv4 interfaces");
    let c = certs::make(&ips)?;
    let stats = Arc::new(metrics::Stats::default());
    metrics::spawn(metrics_path, label, stats.clone());

    // decoder thread
    let (dec_tx, dec_rx) = mpsc::channel();
    {
        let (st, d) = (stats.clone(), decoder.clone());
        std::thread::spawn(move || decode::run(dec_rx, d, vcam, st));
    }

    // rtc thread
    let loss = Arc::new(AtomicU32::new(loss_pm));
    let (tx, rx) = mpsc::channel();
    let socks = rtc::bind_sockets(&ips, tx.clone(), loss.clone());
    {
        let cfg = rtc::Config { ips: ips.clone(), loss_pct: loss.clone() };
        std::thread::spawn(move || rtc::run(cfg, rx, socks, dec_tx));
    }

    let app = Arc::new(App {
        tx,
        sid: AtomicU64::new(1),
        loss,
        ips: ips.clone(),
        mobileconfig: certs::mobileconfig(&c.ca_der),
        ca_pem: c.ca_pem.clone(),
    });
    let router = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/ws", get(ws_handler))
        .route("/ca.mobileconfig", get(mobileconfig))
        .route("/ca.pem", get(capem))
        .route("/debug/pli", get(debug_pli))
        .route("/debug/loss", get(debug_loss))
        .with_state(app.clone());

    // HTTPS
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(c.leaf_chain_pem.clone().into_bytes(), c.leaf_key_pem.clone().into_bytes()).await?;
    let https_addr = SocketAddr::from(([0, 0, 0, 0], https_port));
    let r1 = router.clone();
    tokio::spawn(async move {
        axum_server::bind_rustls(https_addr, tls).serve(r1.into_make_service()).await.unwrap();
    });
    // HTTP (setup + localhost secure-context testing)
    let http_addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let r2 = router.clone();
    tokio::spawn(async move {
        axum_server::bind(http_addr).serve(r2.into_make_service()).await.unwrap();
    });

    if let Some(relay) = arg("--relay") {
        let page = arg("--page").unwrap_or_else(|| "http://127.0.0.1:8000/".into());
        let relay = relay.trim_end_matches('/').to_string();
        let id = room_id();
        let phone_url = format!("{page}#room={id}&relay={relay}");
        let ws_url = format!("{relay}/room/{id}?role=desktop");
        println!("
Phone URL (relay mode): {phone_url}
{}", qr(&phone_url));
        tokio::spawn(relay_loop(app.clone(), ws_url));
    } else {
    for ip in &ips {
        let u1 = format!("http://{ip}:{http_port}/ca.mobileconfig");
        let u2 = format!("https://{ip}:{https_port}/");
        println!("\n=== {ip} ===\nQR#1 CA profile (HTTP): {u1}\n{}", qr(&u1));
        println!("QR#2 camera page (HTTPS): {u2}\n{}", qr(&u2));
    }
    }
    println!("[main] localhost test page: http://localhost:{http_port}/?auto=1   decoder={decoder} vcam={vcam}");

    match run_secs {
        Some(s) => tokio::time::sleep(std::time::Duration::from_secs(s)).await,
        None => std::future::pending::<()>().await,
    }
    println!("[main] --secs elapsed, exiting");
    Ok(())
}

fn qr(s: &str) -> String {
    use qrcode::render::unicode::Dense1x2;
    qrcode::QrCode::new(s.as_bytes()).map(|c| c.render::<Dense1x2>().quiet_zone(true).build()).unwrap_or_default()
}

async fn mobileconfig(State(a): State<Arc<App>>) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/x-apple-aspen-config")], a.mobileconfig.clone())
}
async fn capem(State(a): State<Arc<App>>) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/x-pem-file")], a.ca_pem.clone())
}
async fn debug_pli(State(a): State<Arc<App>>) -> &'static str {
    let _ = a.tx.send(rtc::Msg::Cmd(rtc::Cmd::Pli));
    "pli sent"
}
async fn debug_loss(State(a): State<Arc<App>>, Query(q): Query<HashMap<String, String>>) -> String {
    let p: u32 = q.get("permille").and_then(|s| s.parse().ok()).unwrap_or(0);
    a.loss.store(p, Ordering::Relaxed);
    println!("[debug] simulated RTP loss = {p} per mille");
    format!("loss={p} permille")
}

async fn ws_handler(ws: WebSocketUpgrade, State(a): State<Arc<App>>) -> impl IntoResponse {
    ws.on_upgrade(move |s| ws_loop(s, a))
}

async fn ws_loop(mut s: WebSocket, a: Arc<App>) {
    let sid = a.sid.fetch_add(1, Ordering::Relaxed);
    println!("[ws] client connected, session {sid}");
    let addrs: Vec<String> = a.ips.iter().map(|i| i.to_string()).collect();
    let _ = s.send(Message::Text(serde_json::json!({"type":"hello","addresses":addrs}).to_string().into())).await;
    while let Some(Ok(msg)) = s.recv().await {
        let Message::Text(t) = msg else { continue };
        if let Some(out) = handle_signal(&a, sid, &t).await {
            let _ = s.send(Message::Text(out.into())).await;
        }
    }
    println!("[ws] client {sid} disconnected (peer connection kept alive)");
}

/// Shared signaling handler (local WSS and relay). Returns an optional reply message.
async fn handle_signal(a: &Arc<App>, sid: u64, t: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(t).ok()?;
    match v["type"].as_str() {
        Some("log") => {
            println!("[page] {}", v["msg"].as_str().unwrap_or(""));
            None
        }
        Some("offer") => {
            let (rtx, rrx) = tokio::sync::oneshot::channel();
            let sdp = v["sdp"].as_str().unwrap_or("").to_string();
            let _ = a.tx.send(rtc::Msg::Cmd(rtc::Cmd::Offer { sid, sdp, reply: rtx }));
            let out = match rrx.await {
                Ok(Ok(ans)) => serde_json::json!({"type":"answer","sdp":ans}),
                Ok(Err(e)) => {
                    println!("[ws] offer error: {e}");
                    serde_json::json!({"type":"error","msg":e})
                }
                Err(_) => serde_json::json!({"type":"error","msg":"rtc thread gone"}),
            };
            Some(out.to_string())
        }
        _ => None,
    }
}

fn room_id() -> String {
    use base64::Engine;
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("rng");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Connect to the relay as `desktop`, bridge messages into handle_signal, reconnect with backoff 1..30 s.
async fn relay_loop(a: Arc<App>, url: String) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as M;
    let mut backoff = 1u64;
    loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                println!("[relay] connected");
                backoff = 1;
                let (mut sink, mut stream) = ws.split();
                while let Some(m) = stream.next().await {
                    let t = match m {
                        Ok(M::Text(t)) => t.to_string(),
                        Ok(M::Close(_)) | Err(_) => break,
                        _ => continue,
                    };
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                        if v["t"] == "peer" {
                            println!("[relay] phone up={}", v["up"]);
                            continue;
                        }
                    }
                    if let Some(out) = handle_signal(&a, 1, &t).await {
                        if sink.send(M::Text(out.into())).await.is_err() {
                            break;
                        }
                    }
                }
                println!("[relay] disconnected");
            }
            Err(e) => println!("[relay] connect failed: {e}"),
        }
        println!("[relay] retry in {backoff}s");
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}
