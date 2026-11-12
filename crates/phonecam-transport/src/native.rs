//! Native iPhone sender source (contracts "Round H", wire protocol native v1).
//!
//! The phone listens and advertises Bonjour `_phonecam._tcp`; this module browses mDNS, picks the
//! service whose TXT `id` matches the pairing secret, connects over plain TCP, runs the HMAC
//! handshake and then turns frames into the same [`TransportEvent`]s the WebRTC engine emits, so
//! the media pipeline and the UI do not care which source is live. One worker thread owns the
//! connection state; one short-lived reader thread per connection parses frames.
use crate::{Done, SharedStats, Stats, TransportEvent};
use mdns_sd::{RecvTimeoutError as MdnsTimeout, ServiceDaemon, ServiceEvent};
use phonecam_core::heartbeat::{HbAction, Heartbeat};
use phonecam_core::native_auth as auth;
use phonecam_core::path::PathKind;
use phonecam_core::{Action, Conn, ConnState, Ctl, Event as ConnEvent};
use serde_json::Value;
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tmpsc;

const SERVICE_TYPE: &str = "_phonecam._tcp.local.";
/// Frame body (type + payload) limit from the protocol.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
pub const T_CONTROL: u8 = 0x01;
pub const T_VIDEO: u8 = 0x03;
const RECV_BUF: usize = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// After auth, ask for a keyframe when none arrived within this long, and again every this long.
const KEY_RETRY: Duration = Duration::from_millis(1500);
/// Window over which the received bitrate and the lag estimate are refreshed.
const STATS_EVERY: Duration = Duration::from_secs(1);
const LOG_EVERY: Duration = Duration::from_secs(10);
/// Capacity of the events channel to the app: small, so a slow consumer shows up as drops (and a
/// keyframe resync) right away instead of as seconds of queued video.
const EVENT_QUEUE: usize = 16;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const DISCOVER_FOR: Duration = Duration::from_secs(4);
const HB_INTERVAL_MS: u64 = 2_000;
const HB_MISS_LIMIT: u32 = 5;
/// While the phone says it is paused (backgrounded) iOS may freeze its timers; do not call that a
/// lost link for this long.
const PAUSE_GRACE: Duration = Duration::from_secs(60);
const KEYFRAME_REQ: &str = r#"{"type":"keyframe"}"#;

// ---------- frame codec ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// UTF-8 JSON control message.
    Control(String),
    /// One H.264 access unit (Annex B). `rot` is x90 deg clockwise for an upright picture.
    Video { pts_us: u64, keyframe: bool, rot: u8, data: Vec<u8> },
    /// A frame type this version does not know; skipped.
    Other(u8),
}

/// `u32 BE len` (type + payload) | `u8 type` | payload.
pub fn encode_frame(ty: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + payload.len());
    v.extend_from_slice(&(payload.len() as u32 + 1).to_be_bytes());
    v.push(ty);
    v.extend_from_slice(payload);
    v
}

pub fn encode_control(json: &str) -> Vec<u8> {
    encode_frame(T_CONTROL, json.as_bytes())
}

pub fn encode_video(pts_us: u64, keyframe: bool, rot: u8, annexb: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(10 + annexb.len());
    p.extend_from_slice(&pts_us.to_be_bytes());
    p.push(u8::from(keyframe));
    p.push(rot);
    p.extend_from_slice(annexb);
    encode_frame(T_VIDEO, &p)
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(bad("frame length out of range"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    match body[0] {
        T_CONTROL => String::from_utf8(body.split_off(1)).map(Frame::Control).map_err(|_| bad("control frame is not UTF-8")),
        T_VIDEO => {
            if body.len() < 11 {
                return Err(bad("video frame too short"));
            }
            let pts_us = u64::from_be_bytes(body[1..9].try_into().expect("8 bytes"));
            let (keyframe, rot) = (body[9] & 1 != 0, body[10] & 3);
            body.drain(..11);
            Ok(Frame::Video { pts_us, keyframe, rot, data: body })
        }
        t => Ok(Frame::Other(t)),
    }
}

// ---------- handshake ----------

fn hex_nonce() -> String {
    let mut n = [0u8; 16];
    getrandom::getrandom(&mut n).expect("os rng");
    n.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_nonce(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// PC side of the handshake over any stream. `Err` means: close the connection.
pub(crate) fn client_handshake<S: Read + Write>(s: &mut S, k: &[u8]) -> Result<(), String> {
    let pc = hex_nonce();
    let hello = serde_json::json!({ "t": "hello", "v": 1, "nonce": pc }).to_string();
    s.write_all(&encode_control(&hello)).map_err(|e| format!("send hello: {e}"))?;
    let json = match read_frame(s).map_err(|e| format!("read auth: {e}"))? {
        Frame::Control(j) => j,
        _ => return Err("first frame was not a control frame".into()),
    };
    let v: Value = serde_json::from_str(&json).map_err(|e| format!("auth is not JSON: {e}"))?;
    if v["t"] != "auth" {
        return Err("expected auth".into());
    }
    let (ph, proof) = (v["nonce"].as_str().unwrap_or(""), v["proof"].as_str().unwrap_or(""));
    if !is_nonce(ph) {
        return Err("bad phone nonce".into());
    }
    if !auth::verify_phone_proof(k, &pc, ph, proof) {
        return Err("bad proof".into());
    }
    let ok = serde_json::json!({ "t": "auth_ok", "proof": auth::desktop_proof(k, ph, &pc) }).to_string();
    s.write_all(&encode_control(&ok)).map_err(|e| format!("send auth_ok: {e}"))
}

// ---------- public API ----------

#[derive(Debug, Clone)]
pub struct NativeConfig {
    /// Pairing secret `k` (32 raw bytes). Never logged.
    pub secret: Vec<u8>,
    /// Skip mDNS and connect to this address (tests, manual override).
    pub addr: Option<SocketAddr>,
}

enum Cmd {
    SendCtl(Ctl),
    Keyframe,
    Shutdown,
}

enum Msg {
    Cmd(Cmd),
    /// `(session id, frame, arrival time)` from a reader thread; stale ids are ignored.
    Frame(u64, Frame, Instant),
    Closed(u64, String),
}

#[derive(Clone)]
pub struct NativeHandle {
    tx: mpsc::Sender<Msg>,
    stats: Arc<SharedStats>,
    done: Arc<Done>,
}

impl NativeHandle {
    /// Send a control message to the phone (dropped when not connected).
    pub fn send_ctl(&self, c: Ctl) {
        let _ = self.tx.send(Msg::Cmd(Cmd::SendCtl(c)));
    }
    /// Ask the phone for a keyframe (`{"type":"keyframe"}`, the native PLI).
    pub fn request_keyframe(&self) {
        let _ = self.tx.send(Msg::Cmd(Cmd::Keyframe));
    }
    pub fn stats(&self) -> Stats {
        self.stats.snapshot()
    }
    /// Stop the worker and wait (at most [`crate::SHUTDOWN_WAIT`]) for it to exit. Idempotent;
    /// events end after this. Blocks: call it outside locks and off the async executor.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Cmd(Cmd::Shutdown));
        if !self.done.wait(crate::SHUTDOWN_WAIT) {
            tracing::warn!("[native] worker did not stop within {} s", crate::SHUTDOWN_WAIT.as_secs());
        }
    }
}

pub struct NativeReceiver;

impl NativeReceiver {
    /// Browse mDNS for the paired phone and keep (re)connecting to it.
    pub fn start(secret: Vec<u8>) -> (NativeHandle, tmpsc::Receiver<TransportEvent>) {
        Self::start_with(NativeConfig { secret, addr: None })
    }

    /// Same, but connect to an explicit address and skip mDNS.
    pub fn start_at(addr: SocketAddr, secret: Vec<u8>) -> (NativeHandle, tmpsc::Receiver<TransportEvent>) {
        Self::start_with(NativeConfig { secret, addr: Some(addr) })
    }

    pub fn start_with(cfg: NativeConfig) -> (NativeHandle, tmpsc::Receiver<TransportEvent>) {
        let (ev_tx, ev_rx) = tmpsc::channel(EVENT_QUEUE);
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(SharedStats::default());
        let done = Arc::new(Done::default());
        let mut worker = Worker::new(cfg, tx.clone(), rx, ev_tx, stats.clone());
        let done2 = done.clone();
        std::thread::Builder::new()
            .name("phonecam-native".into())
            .spawn(move || {
                worker.run();
                tracing::info!("[native] worker stopped");
                done2.set();
            })
            .expect("spawn native thread");
        (NativeHandle { tx, stats, done }, ev_rx)
    }
}

// ---------- throughput and lag measurement ----------

// ---------- throughput and lag measurement ----------

/// End-to-end lag estimate: how much later AUs arrive, relative to the phone's capture clock
/// (`pts`), than the best case seen in the last 30 s. A growing value means frames are queuing
/// somewhere between the phone's encoder and our socket (the phone's send queue, the network).
struct LagEstimator {
    first_pts_ms: Option<f64>,
    last_pts_ms: f64,
    /// `(second index, smallest arrival-minus-pts in that second)`, last 30 s.
    mins: std::collections::VecDeque<(u64, f64)>,
    sum: f64,
    n: u64,
}

impl LagEstimator {
    fn new() -> Self {
        Self { first_pts_ms: None, last_pts_ms: 0.0, mins: Default::default(), sum: 0.0, n: 0 }
    }

    fn observe(&mut self, arrival_ms: f64, pts_ms: f64) {
        // The phone restarted its capture clock: start over.
        if self.first_pts_ms.is_some() && pts_ms + 1000.0 < self.last_pts_ms {
            *self = Self::new();
        }
        let first = *self.first_pts_ms.get_or_insert(pts_ms);
        self.last_pts_ms = pts_ms;
        let d = arrival_ms - (pts_ms - first);
        let sec = (arrival_ms / 1000.0) as u64;
        match self.mins.back_mut() {
            Some((s, m)) if *s == sec => *m = m.min(d),
            _ => self.mins.push_back((sec, d)),
        }
        while self.mins.front().is_some_and(|(s, _)| *s + 30 < sec) {
            self.mins.pop_front();
        }
        self.sum += d;
        self.n += 1;
    }

    /// Lag averaged over the AUs seen since the last call, in ms; `None` when there were none.
    fn take(&mut self) -> Option<u32> {
        if self.n == 0 {
            return None;
        }
        let avg = self.sum / self.n as f64;
        let base = self.mins.iter().map(|(_, m)| *m).fold(f64::INFINITY, f64::min);
        self.sum = 0.0;
        self.n = 0;
        Some((avg - base).max(0.0).round() as u32)
    }
}

/// Per-connection counters behind the `[native]` stats line and the UI's bitrate / delay.
struct Flow {
    lag: LagEstimator,
    win_start: Instant,
    bytes: u64,
    log_start: Instant,
    bytes10: u64,
    aus10: u64,
    keys10: u64,
    lag_max10: u32,
    lag_last: u32,
}

impl Flow {
    fn new() -> Self {
        let now = Instant::now();
        Self { lag: LagEstimator::new(), win_start: now, bytes: 0, log_start: now, bytes10: 0, aus10: 0, keys10: 0, lag_max10: 0, lag_last: 0 }
    }

    fn on_au(&mut self, len: usize, keyframe: bool, arrival: Duration, pts_us: u64) {
        self.bytes += len as u64;
        self.bytes10 += len as u64;
        self.aus10 += 1;
        self.keys10 += u64::from(keyframe);
        self.lag.observe(arrival.as_secs_f64() * 1000.0, pts_us as f64 / 1000.0);
    }
}

/// Addresses to try, IPv4 first (then global / ULA IPv6; link-local needs a scope id we lack).
fn order_addrs(ips: impl Iterator<Item = IpAddr>, port: u16) -> Vec<SocketAddr> {
    let (mut v4, mut v6) = (vec![], vec![]);
    for ip in ips {
        match ip {
            IpAddr::V4(a) => v4.push(SocketAddr::new(IpAddr::V4(a), port)),
            IpAddr::V6(a) if (a.segments()[0] & 0xffc0) != 0xfe80 => v6.push(SocketAddr::new(IpAddr::V6(a), port)),
            IpAddr::V6(_) => {}
        }
    }
    v4.sort();
    v4.dedup();
    v6.sort();
    v6.dedup();
    v4.extend(v6);
    v4
}

/// Delay before the next try while the service is visible: 0.5 s, 1 s, then 2 s.
fn retry_delay_ms(failures: u32) -> u64 {
    match failures {
        0 | 1 => 500,
        2 => 1_000,
        _ => 2_000,
    }
}

/// Plain-words reason for a failed TCP connect.
fn describe_connect_error(e: &io::Error) -> String {
    use io::ErrorKind as K;
    match e.kind() {
        K::TimedOut | K::WouldBlock => format!("connect timeout after {} ms", CONNECT_TIMEOUT.as_millis()),
        K::ConnectionRefused => "refused (nothing listening on that port)".into(),
        K::ConnectionReset | K::ConnectionAborted => "connection reset during connect".into(),
        K::HostUnreachable | K::NetworkUnreachable => "no route to host".into(),
        _ => format!("{e}"),
    }
}

// ---------- worker ----------

struct Worker {
    cfg: NativeConfig,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    events: tmpsc::Sender<TransportEvent>,
    stats: Arc<SharedStats>,
    conn: Conn,
    daemon: Option<ServiceDaemon>,
    next_sid: u64,
    done: bool,
}

/// State of one authenticated TCP connection.
struct Session {
    sid: u64,
    stream: TcpStream,
    hb: Heartbeat,
    /// AUs are withheld until a keyframe arrives (first one, or after a dropped AU).
    need_key: bool,
    paused_since: Option<Instant>,
    last_rtt: u32,
    started: Instant,
    flow: Flow,
    /// When a keyframe was last requested (or the session began): drives the 1.5 s re-request.
    key_asked: Instant,
}

impl Worker {
    fn new(
        cfg: NativeConfig,
        tx: mpsc::Sender<Msg>,
        rx: mpsc::Receiver<Msg>,
        events: tmpsc::Sender<TransportEvent>,
        stats: Arc<SharedStats>,
    ) -> Self {
        Self { cfg, tx, rx, events, stats, conn: Conn::new(), daemon: None, next_sid: 0, done: false }
    }

    fn run(&mut self) {
        tracing::info!("[native] started ({})", if self.cfg.addr.is_some() { "explicit address" } else { "mDNS browse" });
        let mut failures = 0u32;
        while !self.done {
            let addrs = match self.cfg.addr {
                Some(a) => vec![a],
                None => self.discover(),
            };
            let mut visible = false;
            if !self.done && !addrs.is_empty() {
                visible = true;
                match self.connect(&addrs) {
                    Some(s) => {
                        failures = 0;
                        self.session(s);
                        // The phone just dropped us (or the app is quitting): look again right away.
                        continue;
                    }
                    None => failures += 1,
                }
            }
            if !self.done {
                // Service visible: retry quickly. Not visible: the browse above already waited.
                let d = if visible { retry_delay_ms(failures) } else { 250 };
                self.wait(Duration::from_millis(d));
            }
        }
        if let Some(d) = self.daemon.take() {
            let _ = d.shutdown();
        }
    }

    /// Once a second: refresh the received bitrate and lag the UI reads; every 10 s: one log line.
    fn flow_tick(&mut self, s: &mut Session) {
        let f = &mut s.flow;
        let el = f.win_start.elapsed();
        if el >= STATS_EVERY {
            let kbps = (f.bytes as f64 * 8.0 / 1000.0 / el.as_secs_f64()).round() as u32;
            self.stats.rx_kbps.store(kbps, Ordering::Relaxed);
            if let Some(lag) = f.lag.take() {
                f.lag_last = lag;
                f.lag_max10 = f.lag_max10.max(lag);
            }
            self.stats.lag_ms.store(f.lag_last, Ordering::Relaxed);
            f.bytes = 0;
            f.win_start = Instant::now();
        }
        let el = f.log_start.elapsed();
        if el >= LOG_EVERY {
            let secs = el.as_secs_f64();
            tracing::info!(
                "[native] stats last {:.0}s: received {:.0} kbps, {} AUs ({:.1}/s), average AU {} bytes, {} keyframes, lag {} ms (max {} ms)",
                secs,
                f.bytes10 as f64 * 8.0 / 1000.0 / secs,
                f.aus10,
                f.aus10 as f64 / secs,
                f.bytes10.checked_div(f.aus10).unwrap_or(0),
                f.keys10,
                f.lag_last,
                f.lag_max10,
            );
            f.bytes10 = 0;
            f.aus10 = 0;
            f.keys10 = 0;
            f.lag_max10 = f.lag_last;
            f.log_start = Instant::now();
        }
    }

    /// Sleep, still honouring `Shutdown`. Other commands are dropped (not connected).
    fn wait(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while !self.done {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.rx.recv_timeout(left) {
                Ok(Msg::Cmd(Cmd::Shutdown)) | Err(RecvTimeoutError::Disconnected) => self.done = true,
                _ => {}
            }
        }
    }

    fn drain_cmds(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            if matches!(m, Msg::Cmd(Cmd::Shutdown)) {
                self.done = true;
            }
        }
    }

    /// Browse for up to `DISCOVER_FOR`; returns the IPv4 addresses of the matching service.
    fn discover(&mut self) -> Vec<SocketAddr> {
        if self.daemon.is_none() {
            match ServiceDaemon::new() {
                Ok(d) => self.daemon = Some(d),
                Err(e) => {
                    tracing::warn!("[native] mDNS daemon failed: {e}");
                    return vec![];
                }
            }
        }
        let daemon = self.daemon.clone().expect("daemon set above");
        let browse = match daemon.browse(SERVICE_TYPE) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("[native] mDNS browse failed: {e}");
                return vec![];
            }
        };
        let want = auth::service_id(&self.cfg.secret);
        let end = Instant::now() + DISCOVER_FOR;
        let mut found = vec![];
        while found.is_empty() && Instant::now() < end {
            self.drain_cmds();
            if self.done {
                break;
            }
            match browse.recv_timeout(Duration::from_millis(250)) {
                Ok(ServiceEvent::ServiceResolved(info)) if info.get_property_val_str("id") == Some(want.as_str()) => {
                    found = order_addrs(info.addresses.iter().map(|a| a.to_ip_addr()), info.port);
                    tracing::info!(
                        "[native] found paired phone '{}' ({} address(es))",
                        info.get_property_val_str("name").unwrap_or("?"),
                        found.len()
                    );
                }
                Ok(_) | Err(MdnsTimeout::Timeout) => {}
                Err(MdnsTimeout::Disconnected) => break,
            }
        }
        let _ = daemon.stop_browse(SERVICE_TYPE);
        found
    }

    /// Tries every address in order (IPv4 first); logs exactly why each one failed. The attempt
    /// runs on its own thread so a shutdown does not wait for a dead address.
    fn connect(&mut self, addrs: &[SocketAddr]) -> Option<TcpStream> {
        for a in addrs {
            if self.done {
                return None;
            }
            let t0 = Instant::now();
            let secret = self.cfg.secret.clone();
            let addr = *a;
            let slot: Arc<Mutex<Option<TcpStream>>> = Arc::default();
            let slot2 = slot.clone();
            let (rtx, rrx) = mpsc::channel::<Result<TcpStream, String>>();
            let spawned = std::thread::Builder::new().name("phonecam-native-conn".into()).spawn(move || {
                let r = match open_stream(&addr) {
                    Ok(mut s) => {
                        if let Ok(c) = s.try_clone() {
                            *slot2.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
                        }
                        let _ = s.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
                        let _ = s.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
                        match client_handshake(&mut s, &secret) {
                            Ok(()) => Ok(s),
                            Err(e) => Err(format!("handshake with {addr} failed: {e}")),
                        }
                    }
                    Err(e) => Err(format!("connect to {addr} failed: {}", describe_connect_error(&e))),
                };
                let _ = rtx.send(r);
            });
            if spawned.is_err() {
                continue;
            }
            let res = loop {
                match rrx.recv_timeout(Duration::from_millis(50)) {
                    Ok(r) => break Some(r),
                    Err(RecvTimeoutError::Timeout) => {
                        self.drain_cmds();
                        if self.done {
                            // Cancel: closing the socket makes the handshake read fail at once.
                            if let Some(c) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                                let _ = c.shutdown(std::net::Shutdown::Both);
                            }
                            break None;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break None,
                }
            };
            match res {
                Some(Ok(s)) => {
                    tracing::info!("[native] authenticated with {a} (connect+handshake {} ms)", t0.elapsed().as_millis());
                    return Some(s);
                }
                Some(Err(e)) => tracing::warn!("[native] {e}"),
                None => return None,
            }
        }
        None
    }

    // ---------- one connection ----------

    fn session(&mut self, stream: TcpStream) {
        self.next_sid += 1;
        let sid = self.next_sid;
        let _ = stream.set_read_timeout(None);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let reader = match stream.try_clone() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[native] clone socket: {e}");
                return;
            }
        };
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("phonecam-native-rx".into())
            .spawn(move || reader_loop(sid, reader, tx))
            .expect("spawn native reader");

        let mut s = Session {
            sid,
            stream,
            hb: Heartbeat::new(HB_INTERVAL_MS, HB_MISS_LIMIT, 0),
            need_key: true,
            paused_since: None,
            last_rtt: 0,
            started: Instant::now(),
            flow: Flow::new(),
            key_asked: Instant::now(),
        };
        self.emit(TransportEvent::PhoneJoined);
        self.conn_event(ConnEvent::PhoneJoined);
        self.emit(TransportEvent::PathChanged { kind: PathKind::Lan, rtt_ms: 0 });

        let reason = loop {
            if self.done {
                break "shutdown".to_string();
            }
            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Msg::Frame(id, f, at)) if id == s.sid => {
                    s.hb.alive();
                    if let Err(e) = self.on_frame(&mut s, f, at) {
                        break e;
                    }
                }
                Ok(Msg::Closed(id, why)) if id == s.sid => break why,
                Ok(Msg::Cmd(Cmd::Shutdown)) | Err(RecvTimeoutError::Disconnected) => self.done = true,
                Ok(Msg::Cmd(Cmd::SendCtl(c))) => {
                    if let Err(e) = send_json(&mut s.stream, &crate::signal::to_json(&c)) {
                        break format!("send: {e}");
                    }
                }
                Ok(Msg::Cmd(Cmd::Keyframe)) => {
                    s.key_asked = Instant::now();
                    if let Err(e) = send_json(&mut s.stream, KEYFRAME_REQ) {
                        break format!("send: {e}");
                    }
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            }
            if let Err(e) = self.tick(&mut s) {
                break e;
            }
        };
        let _ = s.stream.shutdown(std::net::Shutdown::Both);
        self.stats.rx_kbps.store(0, Ordering::Relaxed);
        self.stats.lag_ms.store(0, Ordering::Relaxed);
        tracing::info!("[native] disconnected after {}s: {reason}", s.started.elapsed().as_secs());
        self.emit(TransportEvent::PhoneLeft);
        self.conn_event(ConnEvent::PhoneLeft);
    }

    /// Heartbeat: ping every interval, give up after the miss limit (not while the phone is paused).
    fn tick(&mut self, s: &mut Session) -> Result<(), String> {
        self.flow_tick(s);
        // No keyframe yet (or lost sync): ask again every KEY_RETRY until one arrives.
        if s.need_key && s.key_asked.elapsed() >= KEY_RETRY {
            s.key_asked = Instant::now();
            self.stats.plis.fetch_add(1, Ordering::Relaxed);
            tracing::info!("[native] no keyframe yet, requesting one");
            send_json(&mut s.stream, KEYFRAME_REQ).map_err(|e| format!("send keyframe request: {e}"))?;
        }
        let ms = s.started.elapsed().as_millis() as u64;
        if s.paused_since.is_some_and(|t| t.elapsed() < PAUSE_GRACE) {
            s.hb.alive();
        }
        match s.hb.tick(ms) {
            Some(HbAction::SendPing { t }) => {
                send_json(&mut s.stream, &crate::signal::to_json(&Ctl::Ping { t })).map_err(|e| format!("send ping: {e}"))
            }
            Some(HbAction::Lost) => Err("heartbeat lost".into()),
            None => Ok(()),
        }
    }

    fn on_frame(&mut self, s: &mut Session, f: Frame, at: Instant) -> Result<(), String> {
        match f {
            Frame::Control(json) => {
                match serde_json::from_str::<Ctl>(&json) {
                    Ok(c) => self.on_ctl(s, c)?,
                    // Unknown control types (newer app) are not fatal.
                    Err(e) => tracing::debug!("[native] ignoring control frame: {e}"),
                }
                Ok(())
            }
            Frame::Video { keyframe, rot, data, pts_us } => {
                s.flow.on_au(data.len(), keyframe, at.saturating_duration_since(s.started), pts_us);
                if s.need_key && !keyframe {
                    return Ok(());
                }
                if keyframe {
                    s.need_key = false;
                }
                self.stats.access_units.fetch_add(1, Ordering::Relaxed);
                let ev = TransportEvent::AccessUnit { data, keyframe, rtp_ts: 0, rotation: Some(u16::from(rot) * 90) };
                if self.events.try_send(ev).is_err() {
                    // Consumer too slow (or gone): drop and resync on a keyframe.
                    self.stats.dropped_aus.fetch_add(1, Ordering::Relaxed);
                    self.stats.plis.fetch_add(1, Ordering::Relaxed);
                    s.need_key = true;
                    s.key_asked = Instant::now();
                    send_json(&mut s.stream, KEYFRAME_REQ).map_err(|e| format!("send: {e}"))?;
                } else if keyframe && matches!(self.conn.state(), ConnState::Connecting | ConnState::Reconnecting { .. }) {
                    self.conn_event(ConnEvent::MediaConnected);
                }
                Ok(())
            }
            Frame::Other(t) => {
                tracing::debug!("[native] skipping frame type {t:#04x}");
                Ok(())
            }
        }
    }

    fn on_ctl(&mut self, s: &mut Session, c: Ctl) -> Result<(), String> {
        match &c {
            Ctl::Ping { t } => {
                let pong = Ctl::Pong { t: *t, battery: None, charging: None };
                return send_json(&mut s.stream, &crate::signal::to_json(&pong)).map_err(|e| format!("send pong: {e}"));
            }
            Ctl::Pong { t, .. } => {
                s.hb.pong(*t, s.started.elapsed().as_millis() as u64);
                let rtt = s.hb.rtt_ms().map_or(0, |r| (r as u32).max(1));
                if rtt != s.last_rtt {
                    s.last_rtt = rtt;
                    self.emit(TransportEvent::PathChanged { kind: PathKind::Lan, rtt_ms: rtt });
                }
            }
            Ctl::Paused { reason } => {
                tracing::info!("[native] camera paused: {reason:?}");
                s.paused_since = Some(Instant::now());
                self.conn_event(ConnEvent::CameraPaused(*reason));
            }
            Ctl::Resumed => {
                tracing::info!("[native] camera resumed");
                s.paused_since = None;
                self.conn_event(ConnEvent::CameraResumed);
                // The phone sends a keyframe on resume, but make sure the decoder resyncs.
                s.need_key = true;
                s.key_asked = Instant::now();
                send_json(&mut s.stream, KEYFRAME_REQ).map_err(|e| format!("send: {e}"))?;
            }
            Ctl::Caps { width, height, fps, rotation, current, .. } => {
                tracing::info!("[native] caps: {width:?}x{height:?} @ {fps:?} fps, rotation hint {rotation:?}, lens {current}");
            }
            Ctl::Set { .. } => return Ok(()),
        }
        self.emit(TransportEvent::Ctl(c));
        Ok(())
    }

    fn conn_event(&mut self, ev: ConnEvent) {
        let before = self.conn.state();
        for a in self.conn.handle(ev) {
            // No ICE here: a lost link is a reconnect (the worker loop), so only the vcam matters.
            if let Action::SetVcam(f) = a {
                self.emit(TransportEvent::Vcam(f));
            }
        }
        if matches!(self.conn.state(), ConnState::Live) && !matches!(before, ConnState::Live | ConnState::Paused(_)) {
            tracing::info!("[native] live");
            self.emit(TransportEvent::MediaConnected);
        }
    }

    fn emit(&mut self, ev: TransportEvent) {
        if let Err(tmpsc::error::TrySendError::Full(e)) = self.events.try_send(ev) {
            tracing::warn!("[native] event queue full, dropping {e:?}");
        }
    }
}

fn open_stream(addr: &SocketAddr) -> io::Result<TcpStream> {
    let sock = Socket::new(Domain::for_address(*addr), Type::STREAM, Some(Protocol::TCP))?;
    sock.set_tcp_nodelay(true)?;
    let _ = sock.set_recv_buffer_size(RECV_BUF);
    sock.connect_timeout(&(*addr).into(), CONNECT_TIMEOUT)?;
    Ok(sock.into())
}

fn send_json(s: &mut TcpStream, json: &str) -> io::Result<()> {
    s.write_all(&encode_control(json))
}

fn reader_loop(sid: u64, mut r: TcpStream, tx: mpsc::Sender<Msg>) {
    loop {
        match read_frame(&mut r) {
            Ok(f) => {
                if tx.send(Msg::Frame(sid, f, Instant::now())).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Msg::Closed(sid, e.to_string()));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;

    fn k() -> Vec<u8> {
        (0u8..32).collect()
    }

    #[test]
    fn frame_codec_round_trips() {
        let c = encode_control(r#"{"type":"resumed"}"#);
        assert_eq!(&c[..5], &[0, 0, 0, 19, 1]);
        assert_eq!(read_frame(&mut Cursor::new(c)).unwrap(), Frame::Control(r#"{"type":"resumed"}"#.into()));

        let au = [0, 0, 0, 1, 0x65, 1, 2, 3];
        let v = encode_video(0x0102_0304_0506_0708, true, 3, &au);
        assert_eq!(&v[..4], &(1u32 + 10 + au.len() as u32).to_be_bytes());
        assert_eq!(v[4], T_VIDEO);
        assert_eq!(&v[5..13], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&v[13..15], &[1, 3]);
        let got = read_frame(&mut Cursor::new(v)).unwrap();
        assert_eq!(got, Frame::Video { pts_us: 0x0102_0304_0506_0708, keyframe: true, rot: 3, data: au.to_vec() });

        let d = encode_video(7, false, 0, &au);
        assert!(matches!(read_frame(&mut Cursor::new(d)).unwrap(), Frame::Video { keyframe: false, rot: 0, .. }));
        assert_eq!(read_frame(&mut Cursor::new(encode_frame(0x7f, b"x"))).unwrap(), Frame::Other(0x7f));
    }

    #[test]
    fn frames_parse_back_to_back_and_reject_garbage() {
        let mut two = encode_control("{}");
        two.extend(encode_video(1, true, 0, &[0, 0, 1, 9]));
        let mut c = Cursor::new(two);
        assert!(matches!(read_frame(&mut c).unwrap(), Frame::Control(_)));
        assert!(matches!(read_frame(&mut c).unwrap(), Frame::Video { .. }));
        assert!(read_frame(&mut c).is_err());

        assert!(read_frame(&mut Cursor::new(vec![0, 0, 0, 0])).is_err());
        assert!(read_frame(&mut Cursor::new(((MAX_FRAME as u32) + 1).to_be_bytes().to_vec())).is_err());
        assert!(read_frame(&mut Cursor::new(encode_frame(T_VIDEO, &[0; 5]))).is_err());
        assert!(read_frame(&mut Cursor::new(encode_frame(T_CONTROL, &[0xff, 0xfe]))).is_err());
        // Truncated body.
        let mut t = encode_control("{}");
        t.pop();
        assert!(read_frame(&mut Cursor::new(t)).is_err());
    }

    /// Fake phone: accepts one connection, answers `hello` with `proof_k`'s proof.
    /// Returns the stream and whether the PC's `auth_ok` proof verified.
    fn phone_handshake(l: &TcpListener, secret: &[u8], proof_k: &[u8]) -> (TcpStream, bool) {
        let (mut s, _) = l.accept().unwrap();
        let hello = match read_frame(&mut s).unwrap() {
            Frame::Control(j) => serde_json::from_str::<Value>(&j).unwrap(),
            f => panic!("{f:?}"),
        };
        assert_eq!((hello["t"].as_str(), hello["v"].as_u64()), (Some("hello"), Some(1)));
        let pc = hello["nonce"].as_str().unwrap().to_string();
        assert!(is_nonce(&pc));
        let ph = "f0e1d2c3b4a5968778695a4b3c2d1e0f";
        let a = serde_json::json!({ "t": "auth", "nonce": ph, "proof": auth::phone_proof(proof_k, &pc, ph) });
        s.write_all(&encode_control(&a.to_string())).unwrap();
        let ok = match read_frame(&mut s) {
            Ok(Frame::Control(j)) => {
                let v: Value = serde_json::from_str(&j).unwrap();
                v["t"] == "auth_ok" && auth::verify_desktop_proof(secret, ph, &pc, v["proof"].as_str().unwrap_or(""))
            }
            _ => false,
        };
        (s, ok)
    }

    #[test]
    fn handshake_accepts_good_proof_and_authenticates_the_pc() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || phone_handshake(&l, &k(), &k()).1);
        let mut s = TcpStream::connect(addr).unwrap();
        assert_eq!(client_handshake(&mut s, &k()), Ok(()));
        assert!(t.join().unwrap(), "phone could not verify the desktop proof");
    }

    #[test]
    fn handshake_rejects_bad_proof() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || phone_handshake(&l, &k(), &[7u8; 32]).1);
        let mut s = TcpStream::connect(addr).unwrap();
        assert_eq!(client_handshake(&mut s, &k()), Err("bad proof".into()));
        drop(s);
        assert!(!t.join().unwrap(), "PC must not send auth_ok after a bad proof");
    }

    #[test]
    fn addresses_are_ipv4_first_and_link_local_is_skipped() {
        let ips = ["fe80::1", "2001:db8::1", "192.168.1.9", "10.0.0.2", "192.168.1.9"].iter().map(|s| s.parse::<IpAddr>().unwrap());
        let got: Vec<String> = order_addrs(ips, 9).iter().map(|a| a.to_string()).collect();
        assert_eq!(got, ["10.0.0.2:9", "192.168.1.9:9", "[2001:db8::1]:9"]);
    }

    #[test]
    fn retry_delays_are_half_one_two_seconds() {
        assert_eq!([1, 2, 3, 9].map(retry_delay_ms), [500, 1000, 2000, 2000]);
    }

    #[test]
    fn refused_connection_is_described() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let e = open_stream(&addr).unwrap_err();
        assert!(describe_connect_error(&e).contains("refused") || describe_connect_error(&e).contains("timeout"), "{e}");
    }

    #[test]
    fn shutdown_waits_for_the_worker() {
        let (h, _rx) = NativeReceiver::start_at("127.0.0.1:9".parse().unwrap(), k());
        let t = Instant::now();
        h.shutdown();
        assert!(t.elapsed() < Duration::from_secs(3));
        assert!(h.done.wait(Duration::from_millis(1)));
    }

    fn next_event(rx: &mut tmpsc::Receiver<TransportEvent>) -> TransportEvent {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(e) => return e,
                Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => panic!("no event: {e}"),
            }
        }
    }

    #[test]
    fn bad_proof_never_reaches_the_event_stream() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || phone_handshake(&l, &k(), &[7u8; 32]).1);
        let (h, mut rx) = NativeReceiver::start_at(addr, k());
        assert!(!t.join().unwrap());
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err());
        h.shutdown();
    }

    #[test]
    fn fake_phone_streams_h264_and_receiver_goes_live() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let (got_tx, got_rx) = mpsc::channel::<String>();
        let phone = std::thread::spawn(move || {
            let (mut s, ok) = phone_handshake(&l, &k(), &k());
            assert!(ok);
            let caps = r#"{"type":"caps","lenses":[{"id":"b","label":"Back"}],"current":"b","zoom_min":1.0,"zoom_max":5.0,"torch":true,"rotation":90,"width":3840,"height":2160,"fps":30.0}"#;
            s.write_all(&encode_control(caps)).unwrap();
            // A delta before the first keyframe must be withheld.
            s.write_all(&encode_video(0, false, 1, &[0, 0, 0, 1, 0x41, 9])).unwrap();
            let idr = [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 3, 3, 3];
            s.write_all(&encode_video(33_333, true, 1, &idr)).unwrap();
            for i in 1..4u64 {
                s.write_all(&encode_video(33_333 * (i + 1), false, 1, &[0, 0, 0, 1, 0x41, i as u8])).unwrap();
            }
            s.write_all(&encode_control(r#"{"type":"paused","reason":"call"}"#)).unwrap();
            // Forward whatever the PC sends (keyframe request, set, ...).
            while let Ok(Frame::Control(j)) = read_frame(&mut s) {
                if !j.contains("\"ping\"") {
                    let _ = got_tx.send(j);
                }
            }
        });

        let (h, mut rx) = NativeReceiver::start_at(addr, k());
        let mut evs = vec![];
        while !evs.iter().any(|e| matches!(e, TransportEvent::Ctl(Ctl::Paused { .. }))) {
            evs.push(next_event(&mut rx));
        }
        assert_eq!(evs[0], TransportEvent::PhoneJoined);
        assert!(evs.contains(&TransportEvent::PathChanged { kind: PathKind::Lan, rtt_ms: 0 }));
        assert!(evs.contains(&TransportEvent::Vcam(phonecam_core::VcamFrame::Camera)));
        assert!(evs.contains(&TransportEvent::MediaConnected));
        assert!(evs.iter().any(|e| matches!(e, TransportEvent::Ctl(Ctl::Caps { width: Some(3840), .. }))));
        let aus: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                TransportEvent::AccessUnit { data, keyframe, rotation, .. } => Some((data.len(), *keyframe, *rotation)),
                _ => None,
            })
            .collect();
        assert_eq!(aus, vec![(20, true, Some(90)), (6, false, Some(90)), (6, false, Some(90)), (6, false, Some(90))]);
        assert_eq!(h.stats().access_units, 4);

        h.request_keyframe();
        assert_eq!(got_rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"{"type":"keyframe"}"#);
        h.send_ctl(Ctl::Set { profile: Some(phonecam_core::Profile::Smooth), zoom: None, torch: None, lens: None });
        assert_eq!(got_rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"{"type":"set","profile":"smooth"}"#);
        h.shutdown();
        phone.join().unwrap();
    }
}
