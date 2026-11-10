//! PhoneCam desktop transport (contract A): relay signaling, str0m ICE-lite H.264 receiver with
//! NACK/RTX, runtime interface watch, DataChannel heartbeat and the pairing check.
//!
//! ```ignore
//! let (handle, mut events) = Receiver::start(cfg);
//! while let Some(ev) = events.recv().await { /* TransportEvent */ }
//! handle.shutdown();
//! ```
//! `Receiver::start` spawns its own threads (engine, relay client with a private tokio runtime,
//! one reader per interface), so it can be called from any context.

mod engine;
mod netwatch;
mod relay;
pub mod rtc_config;
pub mod signal;

pub use phonecam_core::path::PathKind;
pub use phonecam_core::{Ctl, VcamFrame};

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use tokio::sync::mpsc as tmpsc;

#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the signaling relay, e.g. `wss://phonecam-relay.example.workers.dev`.
    pub relay_url: String,
    /// URL of the static phone page (only used to build the QR URL).
    pub page_url: String,
    /// 128-bit room id, base64url (relay accepts `[A-Za-z0-9_-]{22,128}`).
    pub room_id: String,
    /// Pairing secret `k` (spec 14a). Never sent to the relay.
    pub secret: Vec<u8>,
}

impl Config {
    /// URL to encode in the QR code. Everything after `#` stays on the phone (never in an HTTP
    /// request): `room`, `relay` and the pairing secret `k` as lowercase hex.
    pub fn phone_url(&self) -> String {
        let k: String = self.secret.iter().map(|b| format!("{b:02x}")).collect();
        format!("{}#room={}&relay={}&k={}", self.page_url, self.room_id, self.relay_url.trim_end_matches('/'), k)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransportEvent {
    /// The phone joined the relay room (before it has proved the pairing secret).
    PhoneJoined,
    /// The phone left the relay room.
    PhoneLeft,
    /// DTLS is up, or media/heartbeat resumed after a loss.
    MediaConnected,
    /// One complete H.264 access unit (Annex B), starting at a keyframe.
    ///
    /// `rotation` is the RTP video-orientation (CVO) the sender attached, as degrees clockwise the
    /// receiver must rotate for an upright picture; `None` when the extension was not negotiated.
    AccessUnit { data: Vec<u8>, keyframe: bool, rtp_ts: u32, rotation: Option<u16> },
    /// Control message from the phone (pong, paused, resumed, ...).
    Ctl(Ctl),
    /// Selected ICE pair changed, or the measured RTT moved. `rtt_ms` is 0 until measured.
    PathChanged { kind: PathKind, rtt_ms: u32 },
    /// What the virtual camera should show now (core state machine, spec 7.3).
    Vcam(VcamFrame),
}

/// Counters for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub access_units: u64,
    /// Access units that arrived non-contiguous (a NACK did not recover them in time).
    pub gaps: u64,
    /// Keyframe requests (PLI) sent.
    pub plis: u64,
    /// NACK packets sent by the receiver (cumulative, from str0m).
    pub nacks: u64,
    /// Access units dropped because the consumer was too slow.
    pub dropped_aus: u64,
}

#[derive(Default)]
pub(crate) struct SharedStats {
    pub access_units: AtomicU64,
    pub gaps: AtomicU64,
    pub plis: AtomicU64,
    pub nacks: AtomicU64,
    pub dropped_aus: AtomicU64,
}

impl SharedStats {
    fn snapshot(&self) -> Stats {
        Stats {
            access_units: self.access_units.load(Ordering::Relaxed),
            gaps: self.gaps.load(Ordering::Relaxed),
            plis: self.plis.load(Ordering::Relaxed),
            nacks: self.nacks.load(Ordering::Relaxed),
            dropped_aus: self.dropped_aus.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone)]
pub struct ReceiverHandle {
    tx: mpsc::Sender<engine::Msg>,
    stats: Arc<SharedStats>,
    loss: Arc<AtomicU32>,
}

impl ReceiverHandle {
    /// Send a control message to the phone over the `ctl` DataChannel (dropped if it is not open).
    pub fn send_ctl(&self, c: Ctl) {
        let _ = self.tx.send(engine::Msg::Cmd(engine::Cmd::SendCtl(c)));
    }
    /// Ask the phone to restart ICE now (DataChannel if open, otherwise the relay).
    pub fn restart_ice(&self) {
        let _ = self.tx.send(engine::Msg::Cmd(engine::Cmd::RestartIce));
    }
    /// Request a keyframe (PLI). Access units are withheld until one arrives.
    pub fn request_keyframe(&self) {
        let _ = self.tx.send(engine::Msg::Cmd(engine::Cmd::Keyframe));
    }
    pub fn stats(&self) -> Stats {
        self.stats.snapshot()
    }
    /// Test hook: drop this many per mille of incoming RTP packets.
    #[doc(hidden)]
    pub fn set_rx_loss_permille(&self, p: u32) {
        self.loss.store(p, Ordering::Relaxed);
    }
    /// Stop all threads. Idempotent; events end after this.
    pub fn shutdown(&self) {
        let _ = self.tx.send(engine::Msg::Cmd(engine::Cmd::Shutdown));
    }
}

pub struct Receiver;

impl Receiver {
    pub fn start(cfg: Config) -> (ReceiverHandle, tmpsc::Receiver<TransportEvent>) {
        let (ev_tx, ev_rx) = tmpsc::channel(256);
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(SharedStats::default());
        let loss = Arc::new(AtomicU32::new(0));
        engine::spawn(cfg, tx.clone(), rx, ev_tx, stats.clone(), loss.clone());
        (ReceiverHandle { tx, stats, loss }, ev_rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_url_keeps_everything_in_the_fragment() {
        let c = Config {
            relay_url: "wss://r.example/".into(),
            page_url: "https://p.example/".into(),
            room_id: "room".into(),
            secret: vec![0xab, 0x01],
        };
        assert_eq!(c.phone_url(), "https://p.example/#room=room&relay=wss://r.example&k=ab01");
    }
}
