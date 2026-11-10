//! `AppState` (the IPC shape, camelCase) and the pure reducer from transport events to it.

use crate::settings::Settings;
use phonecam_core::path::PathKind;
use phonecam_core::{Ctl, LensInfo, PauseReason, VcamFrame};
use phonecam_transport::TransportEvent;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnUi {
    Idle,
    Connecting,
    Live,
    Paused,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PathUi {
    Lan,
    Usb,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub phone_url: String,
    pub qr_svg: String,
    pub conn: ConnUi,
    pub pause_reason: Option<PauseReason>,
    pub path: Option<PathUi>,
    pub rtt_ms: Option<u32>,
    pub battery: Option<f32>,
    pub charging: Option<bool>,
    pub decoder_name: Option<String>,
    /// Virtual-camera output size (fixed), not the stream size.
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Decoded stream as measured on the PC (pre-rotation): size and decoded frames per second.
    pub src_width: u32,
    pub src_height: u32,
    pub src_fps: u32,
    /// What the phone's camera track reports (`Ctl::Caps`); 0 until known.
    pub phone_width: u32,
    pub phone_height: u32,
    pub phone_fps: u32,
    /// Automatic rotation currently applied (degrees clockwise): RTP orientation or phone hint.
    pub auto_rotation: u16,
    pub lenses: Vec<LensInfo>,
    pub zoom_min: f32,
    pub zoom_max: f32,
    pub torch_available: bool,
    pub settings: Settings,
}

/// Connection-side state fed by transport events.
#[derive(Debug, Clone)]
pub struct View {
    pub conn: ConnUi,
    pub pause_reason: Option<PauseReason>,
    pub path: Option<PathUi>,
    pub rtt_ms: Option<u32>,
    pub battery: Option<f32>,
    pub charging: Option<bool>,
    pub lenses: Vec<LensInfo>,
    pub current_lens: Option<String>,
    pub zoom_min: f32,
    pub zoom_max: f32,
    pub torch_available: bool,
    pub phone_width: u32,
    pub phone_height: u32,
    pub phone_fps: f32,
    /// Rotation hint from `Ctl::Caps` (used when the stream carries no RTP orientation).
    pub rotation_hint: u16,
    phone_joined: bool,
    caps_applied: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            conn: ConnUi::Idle,
            pause_reason: None,
            path: None,
            rtt_ms: None,
            battery: None,
            charging: None,
            lenses: vec![],
            current_lens: None,
            zoom_min: 1.0,
            zoom_max: 1.0,
            torch_available: false,
            phone_width: 0,
            phone_height: 0,
            phone_fps: 0.0,
            rotation_hint: 0,
            phone_joined: false,
            caps_applied: false,
        }
    }
}

impl View {
    pub fn zoom_range(&self) -> Option<(f32, f32)> {
        (self.zoom_max > self.zoom_min).then_some((self.zoom_min, self.zoom_max))
    }

    /// Returns true when this event is the first `caps` of a phone session: the caller should
    /// push the persisted settings to the phone once.
    pub fn on_event(&mut self, ev: &TransportEvent) -> bool {
        match ev {
            TransportEvent::PhoneJoined => {
                self.phone_joined = true;
                self.caps_applied = false;
                if self.conn == ConnUi::Idle {
                    self.conn = ConnUi::Connecting;
                }
            }
            TransportEvent::PhoneLeft => {
                self.phone_joined = false;
                self.caps_applied = false;
                self.path = None;
                self.rtt_ms = None;
                self.battery = None;
                self.charging = None;
                self.phone_width = 0;
                self.phone_height = 0;
                self.phone_fps = 0.0;
                self.rotation_hint = 0;
                if self.conn == ConnUi::Connecting {
                    self.conn = ConnUi::Idle;
                }
            }
            TransportEvent::Vcam(f) => match f {
                VcamFrame::Placeholder => {
                    self.conn = if self.phone_joined { ConnUi::Connecting } else { ConnUi::Idle };
                }
                VcamFrame::Camera => {
                    self.conn = ConnUi::Live;
                    self.pause_reason = None;
                }
                VcamFrame::Reconnecting => self.conn = ConnUi::Reconnecting,
                VcamFrame::Paused => self.conn = ConnUi::Paused,
            },
            TransportEvent::PathChanged { kind, rtt_ms } => {
                self.path = match kind {
                    PathKind::Lan => Some(PathUi::Lan),
                    PathKind::UsbHotspot => Some(PathUi::Usb),
                    PathKind::Ignore => None,
                };
                self.rtt_ms = (*rtt_ms > 0).then_some(*rtt_ms);
            }
            TransportEvent::Ctl(c) => match c {
                Ctl::Pong { battery, charging, .. } => {
                    self.battery = *battery;
                    self.charging = *charging;
                }
                Ctl::Paused { reason } => self.pause_reason = Some(*reason),
                Ctl::Resumed => self.pause_reason = None,
                Ctl::Caps { lenses, current, zoom_min, zoom_max, torch, rotation, width, height, fps } => {
                    self.phone_width = width.unwrap_or(0);
                    self.phone_height = height.unwrap_or(0);
                    self.phone_fps = fps.unwrap_or(0.0);
                    self.rotation_hint = rotation.unwrap_or(0);
                    self.lenses = lenses.clone();
                    self.current_lens = Some(current.clone());
                    self.zoom_min = *zoom_min;
                    self.zoom_max = *zoom_max;
                    self.torch_available = *torch;
                    return !std::mem::replace(&mut self.caps_applied, true);
                }
                Ctl::Ping { .. } | Ctl::Set { .. } => {}
            },
            TransportEvent::MediaConnected | TransportEvent::AccessUnit { .. } => {}
        }
        false
    }
}

pub fn qr_svg(url: &str) -> String {
    use qrcode::render::svg;
    match qrcode::QrCode::new(url.as_bytes()) {
        Ok(c) => c.render::<svg::Color>().min_dimensions(256, 256).quiet_zone(true).build(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> TransportEvent {
        TransportEvent::Ctl(Ctl::Caps {
            lenses: vec![LensInfo { id: "a".into(), label: "Back".into() }],
            current: "a".into(),
            zoom_min: 1.0,
            zoom_max: 10.0,
            torch: true,
            rotation: Some(90),
            width: Some(1080),
            height: Some(1920),
            fps: Some(30.0),
        })
    }

    #[test]
    fn conn_follows_phone_and_vcam() {
        let mut v = View::default();
        v.on_event(&TransportEvent::Vcam(VcamFrame::Placeholder));
        assert_eq!(v.conn, ConnUi::Idle);
        v.on_event(&TransportEvent::PhoneJoined);
        assert_eq!(v.conn, ConnUi::Connecting);
        v.on_event(&TransportEvent::Vcam(VcamFrame::Camera));
        assert_eq!(v.conn, ConnUi::Live);
        v.on_event(&TransportEvent::Ctl(Ctl::Paused { reason: PauseReason::Call }));
        v.on_event(&TransportEvent::Vcam(VcamFrame::Paused));
        assert_eq!((v.conn, v.pause_reason), (ConnUi::Paused, Some(PauseReason::Call)));
        v.on_event(&TransportEvent::Ctl(Ctl::Resumed));
        v.on_event(&TransportEvent::Vcam(VcamFrame::Camera));
        assert_eq!((v.conn, v.pause_reason), (ConnUi::Live, None));
        v.on_event(&TransportEvent::PhoneLeft);
        v.on_event(&TransportEvent::Vcam(VcamFrame::Reconnecting));
        assert_eq!(v.conn, ConnUi::Reconnecting);
    }

    #[test]
    fn caps_first_per_session_requests_apply() {
        let mut v = View::default();
        v.on_event(&TransportEvent::PhoneJoined);
        assert!(v.on_event(&caps()));
        assert!(!v.on_event(&caps()), "second caps (lens switch) must not re-apply");
        assert_eq!(v.zoom_range(), Some((1.0, 10.0)));
        assert_eq!((v.phone_width, v.phone_height, v.phone_fps, v.rotation_hint), (1080, 1920, 30.0, 90));
        assert!(v.torch_available);
        v.on_event(&TransportEvent::PhoneLeft);
        v.on_event(&TransportEvent::PhoneJoined);
        assert!(v.on_event(&caps()));
    }

    #[test]
    fn path_and_pong_map_to_ui() {
        let mut v = View::default();
        v.on_event(&TransportEvent::PathChanged { kind: PathKind::UsbHotspot, rtt_ms: 0 });
        assert_eq!((v.path, v.rtt_ms), (Some(PathUi::Usb), None));
        v.on_event(&TransportEvent::PathChanged { kind: PathKind::Lan, rtt_ms: 12 });
        assert_eq!((v.path, v.rtt_ms), (Some(PathUi::Lan), Some(12)));
        v.on_event(&TransportEvent::Ctl(Ctl::Pong { t: 1, battery: Some(0.5), charging: Some(true) }));
        assert_eq!((v.battery, v.charging), (Some(0.5), Some(true)));
    }

    #[test]
    fn qr_is_svg() {
        assert!(qr_svg("https://x.example/#room=a").contains("<svg"));
    }

    #[test]
    fn app_state_is_camel_case() {
        let s = AppState {
            phone_url: "u".into(),
            qr_svg: "s".into(),
            conn: ConnUi::Live,
            pause_reason: None,
            path: Some(PathUi::Lan),
            rtt_ms: Some(3),
            battery: None,
            charging: None,
            decoder_name: None,
            fps: 30,
            width: 1,
            height: 2,
            src_width: 3,
            src_height: 4,
            src_fps: 30,
            phone_width: 5,
            phone_height: 6,
            phone_fps: 30,
            auto_rotation: 0,
            lenses: vec![],
            zoom_min: 1.0,
            zoom_max: 2.0,
            torch_available: false,
            settings: Settings::default(),
        };
        let j = serde_json::to_value(&s).unwrap();
        for k in ["phoneUrl", "qrSvg", "conn", "pauseReason", "rttMs", "decoderName", "zoomMin", "torchAvailable", "settings", "srcWidth", "srcHeight", "srcFps", "phoneWidth", "phoneHeight", "phoneFps", "autoRotation"] {
            assert!(j.get(k).is_some(), "{k}");
        }
        assert_eq!(j["conn"], "live");
        assert_eq!(j["path"], "lan");
        assert_eq!(j["settings"]["profile"], "balanced");
        assert_eq!(j["settings"]["decoder"], "auto");
        assert_eq!(j["settings"]["rotation"], 0);
    }
}
