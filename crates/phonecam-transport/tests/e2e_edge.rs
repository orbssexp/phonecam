//! End-to-end: headless Edge (throwaway profile, fake camera) -> local mini relay -> Receiver.
//! Ignored by default (needs Edge and a private LAN interface). Run with:
//!   cargo test -p phonecam-transport --test e2e_edge -- --ignored --nocapture --test-threads=1
//! Never opens a visible browser or the user's profile; only the Edge it started is killed.
mod common;
use common::{serve_relay, Seen, ROOM, SECRET};
use phonecam_transport::{Config, Receiver, TransportEvent};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

const EDGE: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";

struct EdgeGuard(Child);
impl Drop for EdgeGuard {
    fn drop(&mut self) {
        let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &self.0.id().to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn page_path(with_rtx: bool) -> std::path::PathBuf {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spike/web/index.html");
    let html = std::fs::read_to_string(src).unwrap();
    let html = if with_rtx {
        let needle = "return c.mimeType.toLowerCase()==='video/h264';";
        assert!(html.contains(needle), "spike page changed; update the test");
        html.replace(needle, "var m=c.mimeType.toLowerCase();return m==='video/h264'||m==='video/rtx';")
    } else {
        html
    };
    let p = std::env::temp_dir().join(format!("phonecam-e2e-page-{}.html", with_rtx));
    std::fs::write(&p, html).unwrap();
    p
}

async fn run(with_rtx: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Seen::default();
    tokio::spawn(serve_relay(listener, seen.clone(), true));

    let cfg = Config { relay_url: format!("ws://127.0.0.1:{port}"), page_url: String::new(), room_id: ROOM.into(), secret: SECRET.to_vec() };
    let (h, mut ev) = Receiver::start(cfg);

    let profile = std::env::temp_dir().join(format!("phonecam-e2e-edge-{}-{}", std::process::id(), with_rtx));
    let url = format!(
        "file:///{}?auto=1&res=1280x720@30#room={ROOM}&relay=ws://127.0.0.1:{port}",
        page_path(with_rtx).to_string_lossy().replace('\\', "/")
    );
    let _edge = EdgeGuard(
        Command::new(EDGE)
            .args(["--headless=new", &format!("--user-data-dir={}", profile.display()), "--no-first-run", "--no-default-browser-check", "--disable-sync"])
            .args(["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream", "--autoplay-policy=no-user-gesture-required", &url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start headless edge"),
    );

    let (mut joined, mut connected, mut first_key) = (false, false, None::<Instant>);
    let t0 = Instant::now();
    let mut phase_end = None::<Instant>;
    let mut loss_on = false;
    let mut snap_clean = None;
    let (mut aus, mut aus_loss) = (0u64, 0u64);
    loop {
        if t0.elapsed() > Duration::from_secs(90) {
            panic!("timeout: joined={joined} connected={connected} first_key={first_key:?} stats={:?}", h.stats());
        }
        if let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(200), ev.recv()).await {
            match e {
                TransportEvent::PhoneJoined => joined = true,
                TransportEvent::MediaConnected => connected = true,
                TransportEvent::AccessUnit { keyframe, .. } => {
                    aus += 1;
                    if loss_on {
                        aus_loss += 1;
                    }
                    if keyframe && first_key.is_none() {
                        first_key = Some(Instant::now());
                        phase_end = Some(Instant::now() + Duration::from_secs(8));
                    }
                }
                TransportEvent::PathChanged { kind, rtt_ms } => println!("path {kind:?} rtt={rtt_ms}"),
                other => println!("event {other:?}"),
            }
        }
        if let Some(end) = phase_end {
            if Instant::now() >= end {
                if !loss_on {
                    snap_clean = Some(h.stats());
                    println!("clean phase: aus={aus} stats={:?}", snap_clean.unwrap());
                    h.set_rx_loss_permille(30);
                    loss_on = true;
                    phase_end = Some(Instant::now() + Duration::from_secs(20));
                } else {
                    break;
                }
            }
        }
    }
    let end = h.stats();
    let clean = snap_clean.unwrap();
    println!("offer m=video: {:?}  has_rtx={}", seen.offers.lock().unwrap(), seen.offer_has_rtx.load(Ordering::Relaxed));
    println!("loss phase: aus={aus_loss} nacks={} gaps={} plis={} (clean gaps={} plis={})", end.nacks - clean.nacks, end.gaps - clean.gaps, end.plis - clean.plis, clean.gaps, clean.plis);
    h.shutdown();
    assert!(joined && connected);
    let clean_rate = (aus - aus_loss) as f64 / 8.0;
    assert!(aus_loss as f64 > clean_rate * 20.0 * 0.7, "video stalled under loss: {aus_loss} AUs in 20 s vs {clean_rate:.1}/s clean");
    assert!(end.nacks > clean.nacks, "receiver sent no NACK under 3% loss");
    // With NACK working, most holes are filled before the 100 ms reorder timeout.
    assert!(end.gaps - clean.gaps <= (end.nacks - clean.nacks) / 4 + 2, "too many unrecovered losses");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn edge_offer_without_rtx_still_nacks() {
    run(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn edge_offer_with_rtx_nacks() {
    run(true).await;
}
