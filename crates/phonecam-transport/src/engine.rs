//! The engine thread: owns the str0m session, the interface sockets, the core state machine
//! (`Conn`, `Heartbeat`) and all timers. Ported from the spike's `rtc::run` and extended with
//! pairing, heartbeat-driven ICE restart, the interface watch and NACK.
use crate::netwatch::{self, Sockets};
use crate::rtc_config::{force_nack, new_rtc};
use crate::signal::{self, Incoming};
use crate::{Config, SharedStats, TransportEvent};
use phonecam_core::heartbeat::{HbAction, Heartbeat};
use phonecam_core::path::{classify, PathKind};
use phonecam_core::{Action, Conn, ConnState, Ctl, Event as ConnEvent, Signal};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver as StdRx, RecvTimeoutError, Sender as StdTx};
use std::sync::Arc;
use std::time::{Duration, Instant};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::media::{KeyframeRequestKind, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::sync::{mpsc::Sender as TxSender, watch};

const HB_INTERVAL_MS: u64 = 2_000;
/// 5 missed pongs (about 10 s) and no media. The phone re-acquires its camera on a lens/profile
/// switch, which on iOS can take more than 5 s without media; 6 s was too tight for that.
const HB_MISS_LIMIT: u32 = 5;
/// After we send a camera-affecting `set`, the heartbeat is not allowed to fire for this long.
const SET_GRACE: Duration = Duration::from_secs(12);
/// While the phone says it is paused (backgrounded/locked) iOS may freeze its timers and
/// DataChannel; do not call that a lost link for this long.
const PAUSED_GRACE: Duration = Duration::from_secs(60);
/// Media silent for longer than this and then resuming: ask for a keyframe (the decoder may have
/// been released meanwhile and the encoder restarted).
const RESUME_GAP: Duration = Duration::from_millis(1_500);
const STATS_LOG_EVERY: Duration = Duration::from_secs(10);
const IFACE_POLL: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PLI_RETRY: Duration = Duration::from_secs(1);
const GAP_PLI_MIN_GAP: Duration = Duration::from_millis(300);
const FAILBACK_AFTER: Duration = Duration::from_secs(30);

/// Counters for the 10 s `[stats]` log line.
struct StatWindow {
    start: Instant,
    aus: u32,
    frames: u32,
    keys: u32,
    last_ts: Option<u64>,
    prev: crate::Stats,
}

impl StatWindow {
    fn new(now: Instant) -> Self {
        Self { start: now, aus: 0, frames: 0, keys: 0, last_ts: None, prev: crate::Stats::default() }
    }
}

pub(crate) enum Msg {
    Net { local: SocketAddr, src: SocketAddr, data: Vec<u8>, at: Instant },
    Relay(RelayIn),
    Cmd(Cmd),
}

pub(crate) enum RelayIn {
    Text(String),
    Up,
    Down,
}

pub(crate) enum Cmd {
    SendCtl(Ctl),
    RestartIce,
    Keyframe,
    Shutdown,
}

struct Session {
    rtc: Rtc,
    fingerprint: Option<String>,
    video_mid: Option<Mid>,
    dc: Option<ChannelId>,
    need_key: bool,
    pli_at: Option<Instant>,
    first_au: bool,
    last_pair: Option<(SocketAddr, SocketAddr)>,
}

impl Session {
    fn new(fingerprint: Option<String>, now: Instant) -> Self {
        Self {
            rtc: new_rtc(now),
            fingerprint,
            video_mid: None,
            dc: None,
            need_key: true,
            pli_at: None,
            first_au: true,
            last_pair: None,
        }
    }
}

struct Engine {
    cfg: Config,
    start: Instant,
    events: TxSender<TransportEvent>,
    relay_out: tokio::sync::mpsc::UnboundedSender<String>,
    stats: Arc<SharedStats>,
    socks: Sockets,
    session: Option<Session>,
    conn: Conn,
    hb: Option<Heartbeat>,
    // pairing
    phone_up: bool,
    nonce: Option<String>,
    verified: bool,
    // timers
    retry_at: Option<Instant>,
    connect_deadline: Option<Instant>,
    next_iface_poll: Instant,
    last_media_rx: Option<Instant>,
    grace_until: Option<Instant>,
    paused_since: Option<Instant>,
    // diagnostics
    win: StatWindow,
    last_cvo: Option<Option<u16>>,
    // path
    current_path: Option<PathKind>,
    last_path_event: Option<(PathKind, u32)>,
    last_path_at: Instant,
    lan_since: Option<Instant>,
    failback_sent: bool,
    done: bool,
}

pub(crate) fn spawn(
    cfg: Config,
    tx: StdTx<Msg>,
    rx: StdRx<Msg>,
    events: TxSender<TransportEvent>,
    stats: Arc<SharedStats>,
    loss: Arc<AtomicU32>,
    done: Arc<crate::Done>,
) {
    // str0m needs a process-wide crypto provider; installing twice is harmless.
    str0m::crypto::from_feature_flags().install_process_default();
    let (relay_out, out_rx) = tokio::sync::mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = watch::channel(false);
    let url = crate::relay::ws_url(&cfg.relay_url, &cfg.room_id);
    let relay_thread = crate::relay::spawn(url, tx.clone(), out_rx, stop_rx);
    let now = Instant::now();
    let eng = Engine {
        cfg,
        start: now,
        events,
        relay_out,
        stats,
        socks: Sockets::new(tx, loss),
        session: None,
        conn: Conn::new(),
        hb: None,
        phone_up: false,
        nonce: None,
        verified: false,
        retry_at: None,
        connect_deadline: None,
        next_iface_poll: now,
        last_media_rx: None,
        grace_until: None,
        paused_since: None,
        win: StatWindow::new(now),
        last_cvo: None,
        current_path: None,
        last_path_event: None,
        last_path_at: now,
        lan_since: None,
        failback_sent: false,
        done: false,
    };
    std::thread::Builder::new()
        .name("phonecam-transport".into())
        .spawn(move || {
            let mut eng = eng;
            eng.run(rx);
            eng.socks.stop_all();
            let _ = stop_tx.send(true);
            let _ = relay_thread.join();
            done.set();
        })
        .expect("spawn transport thread");
}

impl Engine {
    fn ms(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.start).as_millis() as u64
    }

    fn run(&mut self, rx: StdRx<Msg>) {
        self.emit(TransportEvent::Vcam(Conn::initial_vcam()));
        while !self.done {
            let now = Instant::now();
            self.timers(now);
            let mut next = self.pump();
            for t in [self.retry_at, self.connect_deadline, Some(self.next_iface_poll)].into_iter().flatten() {
                next = next.min(t);
            }
            if let Some(hb_at) = self.hb_next(now) {
                next = next.min(hb_at);
            }
            if let Some(s) = &self.session {
                if let (true, Some(t)) = (s.need_key, s.pli_at) {
                    next = next.min(t + PLI_RETRY);
                }
            }
            let wait = next.saturating_duration_since(Instant::now()).clamp(Duration::from_millis(1), Duration::from_millis(500));
            match rx.recv_timeout(wait) {
                Ok(m) => self.on_msg(m),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // Drain what is already queued before looping, so a burst of RTP is one pump.
            while let Ok(m) = rx.try_recv() {
                self.on_msg(m);
                if self.done {
                    return;
                }
            }
            if let Some(s) = self.session.as_mut() {
                if s.rtc.is_alive() {
                    let _ = s.rtc.handle_input(Input::Timeout(Instant::now()));
                }
            }
        }
    }

    // ---------- outputs ----------

    fn emit(&mut self, ev: TransportEvent) {
        match self.events.try_send(ev) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(e)) => {
                if matches!(e, TransportEvent::AccessUnit { .. }) {
                    self.stats.dropped_aus.fetch_add(1, Ordering::Relaxed);
                    if let Some(s) = self.session.as_mut() {
                        s.need_key = true;
                    }
                    self.request_pli(false);
                } else {
                    tracing::warn!("[transport] event queue full, dropping {e:?}");
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => self.done = true,
        }
    }

    fn relay_send(&self, text: String) {
        let _ = self.relay_out.send(text);
    }

    fn dc_write(&mut self, text: &str) -> bool {
        let Some(s) = self.session.as_mut() else { return false };
        let Some(cid) = s.dc else { return false };
        match s.rtc.channel(cid) {
            Some(mut ch) => ch.write(false, text.as_bytes()).is_ok(),
            None => false,
        }
    }

    /// Signaling goes over the DataChannel when it is up, otherwise through the relay (spec 7.1).
    fn send_signal(&mut self, sig: &Signal, prefer_dc: bool) {
        let text = signal::to_json(sig);
        if prefer_dc && self.dc_write(&text) {
            return;
        }
        self.relay_send(text);
    }

    /// Run str0m until it has nothing more to do now; returns its next timeout.
    fn pump(&mut self) -> Instant {
        let mut next = Instant::now() + Duration::from_millis(500);
        for _ in 0..16 {
            let mut evs = vec![];
            let mut dead = false;
            if let Some(s) = self.session.as_mut() {
                if s.rtc.is_alive() {
                    loop {
                        match s.rtc.poll_output() {
                            Ok(Output::Timeout(t)) => {
                                next = next.min(t);
                                break;
                            }
                            Ok(Output::Transmit(t)) => {
                                if !self.socks.send(t.source, t.destination, &t.contents) {
                                    tracing::debug!("[rtc] no socket for {}", t.source);
                                }
                            }
                            Ok(Output::Event(e)) => evs.push(e),
                            Err(e) => {
                                tracing::warn!("[rtc] poll_output error: {e:?}");
                                s.rtc.disconnect();
                                break;
                            }
                        }
                    }
                } else {
                    dead = true;
                }
            }
            if dead {
                tracing::info!("[rtc] session ended (rtc closed: DTLS close_notify, SCTP association lost or an I/O error); the phone must start a new connection");
                self.session = None;
                self.hb = None;
                self.conn_event(ConnEvent::HeartbeatLost);
                break;
            }
            if evs.is_empty() {
                break;
            }
            for e in evs {
                self.on_rtc_event(e);
            }
        }
        next
    }

    // ---------- timers ----------

    fn hb_next(&self, now: Instant) -> Option<Instant> {
        // Heartbeat has no "next due" accessor; polling twice per interval is plenty.
        self.hb.as_ref().map(|_| now + Duration::from_millis(HB_INTERVAL_MS / 2))
    }

    fn timers(&mut self, now: Instant) {
        // Heartbeat.
        let ms = self.ms(now);
        if self.hb.is_some() {
            // Live media proves the path even if pongs are late.
            if self.last_media_rx.is_some_and(|t| now.duration_since(t) < Duration::from_millis(HB_INTERVAL_MS)) {
                if let Some(hb) = self.hb.as_mut() {
                    hb.alive();
                }
            }
            // The phone is switching its camera (we just asked), or it told us it is paused.
            let in_grace = self.grace_until.is_some_and(|t| now < t)
                || self.paused_since.is_some_and(|t| now.duration_since(t) < PAUSED_GRACE);
            if in_grace {
                if let Some(hb) = self.hb.as_mut() {
                    hb.alive();
                }
            }
            let act = self.hb.as_mut().and_then(|hb| hb.tick(ms));
            match act {
                Some(HbAction::SendPing { t }) => {
                    let text = signal::to_json(&Ctl::Ping { t });
                    self.dc_write(&text);
                }
                Some(HbAction::Lost) => {
                    let silent = self.last_media_rx.map(|t| now.duration_since(t).as_millis());
                    tracing::info!(
                        "[hb] heartbeat lost: no pong for {} pings and media silent for {:?} ms",
                        HB_MISS_LIMIT,
                        silent
                    );
                    self.conn_event(ConnEvent::HeartbeatLost)
                }
                None => {}
            }
        }
        if now.duration_since(self.win.start) >= STATS_LOG_EVERY {
            self.log_stats(now);
        }
        if self.retry_at.is_some_and(|t| now >= t) {
            self.retry_at = None;
            self.conn_event(ConnEvent::RetryTimerFired);
        }
        if self.connect_deadline.is_some_and(|t| now >= t) {
            self.connect_deadline = None;
            self.conn_event(ConnEvent::ConnectTimedOut);
        }
        if let Some(s) = self.session.as_ref() {
            if s.need_key && s.pli_at.is_some_and(|t| now.duration_since(t) > PLI_RETRY) {
                tracing::info!("[rtc] no keyframe 1 s after PLI; re-requesting");
                self.request_pli(true);
            }
        }
        if now >= self.next_iface_poll {
            self.next_iface_poll = now + IFACE_POLL;
            self.watch_interfaces(now);
        }
    }

    /// Spec 7.1 "runtime interface watch" and the 30 s fail-back to the LAN.
    fn watch_interfaces(&mut self, now: Instant) {
        let ifaces = netwatch::scan();
        let ch = self.socks.sync(&ifaces);
        if self.socks.has_kind(PathKind::Lan) {
            self.lan_since.get_or_insert(now);
        } else {
            self.lan_since = None;
        }
        if !ch.is_empty() {
            if let Some(s) = self.session.as_mut() {
                for a in &ch.added {
                    if let Ok(c) = Candidate::host(*a, "udp") {
                        s.rtc.add_local_candidate(c);
                    }
                }
                for a in &ch.removed {
                    if let Ok(c) = Candidate::host(*a, "udp") {
                        s.rtc.direct_api().invalidate_candidate(&c);
                    }
                }
            }
            // The phone only learns new candidates through a re-offer; if we are already in
            // trouble, ask for one now instead of waiting for the next retry.
            if matches!(self.conn.state(), ConnState::Reconnecting { .. }) {
                tracing::info!("[net] interfaces changed while reconnecting; asking for ICE restart");
                self.send_signal(&Signal::RestartIce, true);
            }
        }
        if self.current_path == Some(PathKind::UsbHotspot)
            && !self.failback_sent
            && matches!(self.conn.state(), ConnState::Live)
            && self.lan_since.is_some_and(|t| now.duration_since(t) >= FAILBACK_AFTER)
        {
            tracing::info!("[net] LAN stable for 30 s; asking the phone to move back");
            self.failback_sent = true;
            self.send_signal(&Signal::RestartIce, true);
        }
    }

    fn log_stats(&mut self, now: Instant) {
        let secs = now.duration_since(self.win.start).as_secs_f32().max(0.001);
        let st = self.stats.snapshot();
        let live = matches!(self.conn.state(), ConnState::Live | ConnState::Paused(_));
        if live || self.win.aus > 0 {
            tracing::info!(
                "[stats] last {:.0}s: {} AUs ({:.1}/s, {:.1} distinct frames/s by RTP ts, {} keyframes), +{} gaps, +{} PLI, +{} NACK, +{} dropped (total gaps {} pli {} nack {} dropped {}), rtt {:?} ms, state {:?}",
                secs,
                self.win.aus,
                self.win.aus as f32 / secs,
                self.win.frames as f32 / secs,
                self.win.keys,
                st.gaps - self.win.prev.gaps,
                st.plis - self.win.prev.plis,
                st.nacks - self.win.prev.nacks,
                st.dropped_aus - self.win.prev.dropped_aus,
                st.gaps,
                st.plis,
                st.nacks,
                st.dropped_aus,
                self.hb.as_ref().and_then(|h| h.rtt_ms()),
                self.conn.state(),
            );
        }
        self.win = StatWindow { prev: st, ..StatWindow::new(now) };
    }

    // ---------- core state machine glue ----------

    fn conn_event(&mut self, ev: ConnEvent) {
        let before = self.conn.state();
        let actions = self.conn.handle(ev);
        let after_state = self.conn.state();
        if before != after_state {
            tracing::info!("[conn] {before:?} -> {after_state:?} (cause: {ev:?})");
        }
        let now = Instant::now();
        for a in actions {
            match a {
                Action::SetVcam(f) => self.emit(TransportEvent::Vcam(f)),
                Action::SendRestartIce => {
                    tracing::info!("[conn] sending restart_ice");
                    self.send_signal(&Signal::RestartIce, true);
                }
                Action::ScheduleRetry { ms } => self.retry_at = Some(now + Duration::from_millis(ms)),
            }
        }
        let after = self.conn.state();
        if matches!(after, ConnState::Live) {
            self.retry_at = None;
            self.connect_deadline = None;
            if !matches!(before, ConnState::Live | ConnState::Paused(_)) {
                let ms = self.ms(now);
                if let Some(hb) = self.hb.as_mut() {
                    hb.reset(ms);
                }
                self.emit(TransportEvent::MediaConnected);
            }
        }
        if matches!(after, ConnState::Idle) {
            self.retry_at = None;
        }
    }

    // ---------- inbound ----------

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Net { local, src, data, at } => {
                let Ok(contents) = data.as_slice().try_into() else { return };
                let input = Input::Receive(at, Receive { proto: Protocol::Udp, source: src, destination: local, contents });
                if let Some(s) = self.session.as_mut() {
                    if s.rtc.accepts(&input) {
                        if let Err(e) = s.rtc.handle_input(input) {
                            tracing::warn!("[rtc] handle_input: {e:?}");
                            s.rtc.disconnect();
                        }
                    }
                }
            }
            Msg::Relay(RelayIn::Text(t)) => match signal::parse_relay(&t) {
                Incoming::Peer { up } => self.on_peer(up),
                Incoming::Signal(sig) => self.on_signal(sig, false),
                Incoming::Ctl(_) | Incoming::Ignored => {}
            },
            Msg::Relay(RelayIn::Up) | Msg::Relay(RelayIn::Down) => {}
            Msg::Cmd(Cmd::SendCtl(c)) => {
                if matches!(&c, Ctl::Set { profile, lens, .. } if profile.is_some() || lens.is_some()) {
                    // The phone re-acquires its camera now; expect media to stall for a while.
                    self.grace_until = Some(Instant::now() + SET_GRACE);
                }
                let text = signal::to_json(&c);
                if !self.dc_write(&text) {
                    tracing::debug!("[ctl] DataChannel not open, dropping {c:?}");
                }
            }
            Msg::Cmd(Cmd::RestartIce) => self.send_signal(&Signal::RestartIce, true),
            Msg::Cmd(Cmd::Keyframe) => {
                if let Some(s) = self.session.as_mut() {
                    s.need_key = true;
                }
                self.request_pli(true);
            }
            Msg::Cmd(Cmd::Shutdown) => self.done = true,
        }
    }

    fn on_peer(&mut self, up: bool) {
        tracing::info!("[relay] phone {}", if up { "joined the room" } else { "left the room" });
        if up {
            let first = !self.phone_up;
            self.phone_up = true;
            // Every (re)join must prove the pairing secret again.
            self.verified = false;
            let nonce = signal::new_nonce();
            self.nonce = Some(nonce.clone());
            self.send_signal(&Signal::Challenge { nonce }, false);
            if first {
                self.emit(TransportEvent::PhoneJoined);
                self.conn_event(ConnEvent::PhoneJoined);
                self.connect_deadline = Some(Instant::now() + CONNECT_TIMEOUT);
            }
        } else if self.phone_up {
            self.phone_up = false;
            self.verified = false;
            self.nonce = None;
            self.emit(TransportEvent::PhoneLeft);
            // The relay socket can drop while media keeps flowing (iOS suspends it first); only
            // treat the phone as gone when there is no live media session either.
            let media_alive = self.last_media_rx.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
            if media_alive {
                tracing::info!("[relay] relay dropped but media is still flowing; keeping the session");
            } else {
                self.conn_event(ConnEvent::PhoneLeft);
            }
        }
    }

    fn on_signal(&mut self, sig: Signal, via_dc: bool) {
        match sig {
            Signal::Hello { .. } => {
                let ok = self.nonce.as_deref().is_some_and(|n| signal::hello_ok(&self.cfg.secret, n, &sig));
                if ok {
                    tracing::info!("[pair] phone proved the pairing secret");
                    self.verified = true;
                } else {
                    tracing::warn!("[pair] bad hello proof; rejecting phone");
                    self.verified = false;
                    self.send_signal(&Signal::Bye, false);
                }
            }
            Signal::Offer { sdp } => {
                // Over the DataChannel the peer is already authenticated by its DTLS session.
                if !via_dc && !self.verified {
                    tracing::warn!("[pair] offer before a valid hello; ignored");
                    return;
                }
                match self.accept_offer(&sdp) {
                    Ok(answer) => self.send_signal(&Signal::Answer { sdp: answer }, via_dc),
                    Err(e) => tracing::warn!("[sdp] offer rejected: {e}"),
                }
            }
            Signal::Bye => tracing::info!("[sig] phone said bye"),
            Signal::Challenge { .. } | Signal::Answer { .. } | Signal::RestartIce => {}
        }
    }

    fn accept_offer(&mut self, sdp: &str) -> Result<String, String> {
        let offer = SdpOffer::from_sdp_string(sdp).map_err(|e| format!("bad offer: {e:?}"))?;
        tracing::info!(
            "[sdp] offer received ({} bytes); video-orientation (CVO) extension offered: {}",
            sdp.len(),
            sdp.contains("urn:3gpp:video-orientation")
        );
        let fp = signal::fingerprint(sdp);
        let reuse = self.session.as_ref().is_some_and(|s| s.rtc.is_alive() && fp.is_some() && s.fingerprint == fp);
        if !reuse {
            // New page load (new DTLS identity): fresh session. The old one is dropped.
            self.session = Some(Session::new(fp, Instant::now()));
            self.hb = None;
        } else {
            tracing::info!("[ice] re-offer on the existing session (ICE restart / renegotiation)");
        }
        let locals = self.socks.locals();
        let s = self.session.as_mut().expect("session set above");
        for l in &locals {
            if let Ok(c) = Candidate::host(*l, "udp") {
                s.rtc.add_local_candidate(c);
            }
        }
        let answer = s.rtc.sdp_api().accept_offer(offer).map_err(|e| format!("accept_offer: {e:?}"))?;
        if let Some(mid) = s.video_mid {
            force_nack(&mut s.rtc, mid);
        }
        Ok(answer.to_sdp_string())
    }

    // ---------- str0m events ----------

    fn request_pli(&mut self, count: bool) {
        let Some(s) = self.session.as_mut() else { return };
        let Some(mid) = s.video_mid else { return };
        if let Some(mut w) = s.rtc.writer(mid) {
            match w.request_keyframe(None, KeyframeRequestKind::Pli) {
                Ok(()) => {
                    s.pli_at = Some(Instant::now());
                    if count {
                        self.stats.plis.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => tracing::debug!("[rtc] request_keyframe: {e:?}"),
            }
        }
    }

    fn on_rtc_event(&mut self, e: Event) {
        match e {
            Event::IceConnectionStateChange(st) => {
                tracing::info!("[ice] state -> {st:?}");
                match st {
                    IceConnectionState::Disconnected => {
                        // Media still flowing proves the path; a transient ICE flap (candidate
                        // re-nomination, interface change) must not black out the picture.
                        let media_alive = self.last_media_rx.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
                        if media_alive {
                            tracing::info!("[ice] disconnected but media is still flowing; ignoring");
                        } else {
                            tracing::info!("[conn] ICE disconnected and no media; treating the link as lost");
                            self.conn_event(ConnEvent::HeartbeatLost);
                        }
                    }
                    IceConnectionState::Connected | IceConnectionState::Completed => {
                        if matches!(self.conn.state(), ConnState::Reconnecting { .. }) {
                            // ICE is back; heartbeat/media will confirm. Probe right away.
                            let ms = self.ms(Instant::now());
                            if let Some(hb) = self.hb.as_mut() {
                                hb.reset(ms);
                            }
                            self.request_pli_if_needed();
                        }
                    }
                    _ => {}
                }
            }
            Event::Connected => {
                tracing::info!("[rtc] DTLS connected");
                self.conn_event(ConnEvent::MediaConnected);
            }
            Event::MediaAdded(m) => {
                if m.kind == MediaKind::Video {
                    if let Some(s) = self.session.as_mut() {
                        s.video_mid = Some(m.mid);
                        s.need_key = true;
                        force_nack(&mut s.rtc, m.mid);
                    }
                    self.request_pli(true);
                }
            }
            Event::MediaData(d) => self.on_media(d),
            Event::Closed => tracing::info!("[rtc] session closed by the peer or the transport (DTLS/SCTP)"),
            Event::ChannelOpen(cid, label) => {
                tracing::info!("[dc] open {label}");
                if let Some(s) = self.session.as_mut() {
                    s.dc = Some(cid);
                }
                self.hb = Some(Heartbeat::new(HB_INTERVAL_MS, HB_MISS_LIMIT, self.ms(Instant::now())));
            }
            Event::ChannelClose(cid) => {
                if let Some(s) = self.session.as_mut() {
                    if s.dc == Some(cid) {
                        s.dc = None;
                    }
                }
                self.hb = None;
            }
            Event::ChannelData(cd) => {
                let text = String::from_utf8_lossy(&cd.data).into_owned();
                match signal::parse_channel(&text) {
                    Incoming::Ctl(c) => self.on_ctl(c),
                    Incoming::Signal(sig) => self.on_signal(sig, true),
                    _ => {}
                }
            }
            Event::PeerStats(ps) => {
                if let Some(p) = ps.selected_candidate_pair {
                    let pair = (p.local.addr, p.remote.addr);
                    let changed = self.session.as_ref().is_some_and(|s| s.last_pair != Some(pair));
                    if changed {
                        if let Some(s) = self.session.as_mut() {
                            s.last_pair = Some(pair);
                        }
                        let kind = self.socks.kind_of(p.local.addr).unwrap_or_else(|| match p.local.addr.ip() {
                            // Same-host phone (test harness, loopback): a working direct path, shown as LAN.
                            std::net::IpAddr::V4(ip) if ip.is_loopback() => PathKind::Lan,
                            std::net::IpAddr::V4(ip) => classify("", ip),
                            _ => PathKind::Ignore,
                        });
                        tracing::info!("[ice] nominated pair {} <-> {} ({kind:?})", p.local.addr, p.remote.addr);
                        if self.current_path != Some(kind) {
                            self.failback_sent = false;
                        }
                        self.current_path = Some(kind);
                    }
                    let ice_rtt = p.current_round_trip_time.map(|d| d.as_millis() as u32);
                    self.maybe_path_event(changed, ice_rtt);
                }
            }
            Event::MediaIngressStats(st) => {
                self.stats.nacks.store(st.nacks, Ordering::Relaxed);
                // The stream may have been (re)created since the last offer.
                if let Some(s) = self.session.as_mut() {
                    if let Some(mid) = s.video_mid {
                        force_nack(&mut s.rtc, mid);
                    }
                }
            }
            _ => {}
        }
    }

    fn request_pli_if_needed(&mut self) {
        if self.session.as_ref().is_some_and(|s| s.need_key) {
            self.request_pli(true);
        }
    }

    fn maybe_path_event(&mut self, force: bool, ice_rtt: Option<u32>) {
        let Some(kind) = self.current_path else { return };
        let rtt = self.hb.as_ref().and_then(|h| h.rtt_ms()).map(|r| (r as u32).max(1)).or(ice_rtt).unwrap_or(0);
        let moved = self.last_path_event.is_none_or(|(k, r)| k != kind || (r == 0) != (rtt == 0) || r.abs_diff(rtt) >= 5);
        let due = self.last_path_at.elapsed() >= Duration::from_secs(2);
        if force || (moved && due) {
            if self.last_path_event != Some((kind, rtt)) {
                self.emit(TransportEvent::PathChanged { kind, rtt_ms: rtt });
            }
            self.last_path_event = Some((kind, rtt));
            self.last_path_at = Instant::now();
        }
    }

    fn on_ctl(&mut self, c: Ctl) {
        let now = Instant::now();
        match &c {
            Ctl::Ping { t } => {
                let text = signal::to_json(&Ctl::Pong { t: *t, battery: None, charging: None });
                self.dc_write(&text);
            }
            Ctl::Pong { t, .. } => {
                let ms = self.ms(now);
                if let Some(hb) = self.hb.as_mut() {
                    hb.pong(*t, ms);
                }
                // A pong proves the path is alive again after a restart.
                self.conn_event(ConnEvent::MediaConnected);
                self.maybe_path_event(false, None);
            }
            Ctl::Paused { reason } => {
                tracing::info!("[phone] camera paused: {reason:?}");
                self.paused_since = Some(now);
                self.conn_event(ConnEvent::CameraPaused(*reason));
            }
            Ctl::Resumed => {
                tracing::info!("[phone] camera resumed");
                self.paused_since = None;
                self.conn_event(ConnEvent::CameraResumed);
            }
            Ctl::Caps { width, height, fps, rotation, current, .. } => {
                tracing::info!("[phone] caps: track {width:?}x{height:?} @ {fps:?} fps, rotation hint {rotation:?}, lens {current}");
            }
            Ctl::Set { .. } => {}
        }
        self.emit(TransportEvent::Ctl(c));
    }

    fn on_media(&mut self, d: str0m::media::MediaData) {
        let now = Instant::now();
        let gap = self.last_media_rx.map(|t| now.duration_since(t));
        self.last_media_rx = Some(now);
        let mut request = false;
        let Some(s) = self.session.as_mut() else { return };
        if let Some(g) = gap.filter(|g| *g >= RESUME_GAP) {
            // Media resumed after a stall (camera switch, Wi-Fi hiccup, app resume): the decoder
            // may have released itself and the first frames are likely not decodable. PLI now
            // instead of waiting for the next periodic keyframe.
            tracing::info!("[rtc] media resumed after {} ms of silence; requesting a keyframe", g.as_millis());
            s.need_key = true;
            request = true;
        }
        if s.first_au {
            s.first_au = false;
            tracing::info!("[rtc] first media data, {} bytes", d.data.len());
            if let Some(mid) = s.video_mid {
                force_nack(&mut s.rtc, mid);
            }
        }
        if !d.contiguous {
            // NACK did not (yet) recover the packets: the access unit is damaged.
            self.stats.gaps.fetch_add(1, Ordering::Relaxed);
            tracing::info!("[rtc] GAP: non-contiguous AU (seq {:?}), requesting keyframe", d.seq_range);
            s.need_key = true;
            if s.pli_at.is_none_or(|t| t.elapsed() > GAP_PLI_MIN_GAP) {
                request = true;
            }
        }
        let rotation = d.ext_vals.video_orientation.map(cvo_degrees);
        if self.last_cvo != Some(rotation) {
            tracing::info!("[rtc] RTP video-orientation (CVO) from the phone: {rotation:?} deg clockwise");
            self.last_cvo = Some(rotation);
        }
        let key = d.is_keyframe();
        if key {
            if let Some(t) = s.pli_at.take() {
                tracing::info!("[pli] keyframe {} ms after PLI", t.elapsed().as_millis());
            }
            s.need_key = false;
        }
        let deliver = !s.need_key;
        if request {
            self.request_pli(true);
        }
        if !deliver {
            return;
        }
        // A decodable frame arriving while we were reconnecting means the media path is back
        // (waiting for the next keyframe would leave the "Reconnecting" picture up for seconds).
        match self.conn.state() {
            ConnState::Reconnecting { .. } => self.conn_event(ConnEvent::MediaConnected),
            ConnState::Connecting if key => self.conn_event(ConnEvent::MediaConnected),
            _ => {}
        }
        self.stats.access_units.fetch_add(1, Ordering::Relaxed);
        let ts = d.time.numer();
        self.win.aus += 1;
        if key {
            self.win.keys += 1;
        }
        if self.win.last_ts != Some(ts) {
            self.win.frames += 1;
            self.win.last_ts = Some(ts);
        }
        self.emit(TransportEvent::AccessUnit { data: d.data.to_vec(), keyframe: key, rtp_ts: ts as u32, rotation });
    }
}


/// RTP video-orientation to "degrees clockwise the receiver must rotate". libwebrtc/WebKit put 1 on
/// the wire for "rotate 90 clockwise"; str0m names the wire value 1 `Deg270` and 3 `Deg90`.
fn cvo_degrees(o: str0m::rtp::VideoOrientation) -> u16 {
    use str0m::rtp::VideoOrientation::*;
    match o {
        Deg0 => 0,
        Deg270 => 90,
        Deg180 => 180,
        Deg90 => 270,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cvo_wire_values_map_like_libwebrtc() {
        use str0m::rtp::VideoOrientation;
        // wire 0,1,2,3 -> render rotation 0,90,180,270 clockwise.
        let by_wire: Vec<u16> = (0u8..4).map(|w| cvo_degrees(VideoOrientation::from(w))).collect();
        assert_eq!(by_wire, vec![0, 90, 180, 270]);
    }
}
