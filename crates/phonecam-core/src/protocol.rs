use serde::{Deserialize, Serialize};

use crate::controls::Profile;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Desktop,
    Phone,
}

/// Signaling messages (via relay, or DataChannel once up). Spec §14.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Signal {
    /// Desktop → phone when the phone joins: the nonce the phone must prove with (§14a).
    Challenge { nonce: String },
    /// Phone → desktop: `proof` = `pairing::proof_hex(k, nonce)`.
    Hello { token: Option<String>, role: Role, version: u32, proof: String },
    Offer { sdp: String },
    Answer { sdp: String },
    RestartIce,
    Bye,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Call,
    Background,
    Locked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensInfo {
    pub id: String,
    pub label: String,
}

/// Control messages on the `ctl` DataChannel. Spec §14.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ctl {
    Ping { t: u64 },
    Pong { t: u64, battery: Option<f32>, charging: Option<bool> },
    Set {
        #[serde(skip_serializing_if = "Option::is_none")]
        profile: Option<Profile>,
        #[serde(skip_serializing_if = "Option::is_none")]
        zoom: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        torch: Option<bool>,
        /// Phone-side `deviceId` of the lens to switch to.
        #[serde(skip_serializing_if = "Option::is_none")]
        lens: Option<String>,
    },
    Paused { reason: PauseReason },
    Resumed,
    /// Phone to desktop on DC open and after every applied `set`.
    ///
    /// `rotation`, `width`, `height` and `fps` are optional (older pages omit them): the rotation
    /// in degrees clockwise the PC must apply for an upright picture (hint, used only when the RTP
    /// video-orientation extension is not negotiated) and what `track.getSettings()` reports.
    Caps {
        lenses: Vec<LensInfo>,
        current: String,
        zoom_min: f32,
        zoom_max: f32,
        torch: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Profile;

    #[test]
    fn restart_ice_is_a_bare_tag() {
        assert_eq!(serde_json::to_string(&Signal::RestartIce).unwrap(), r#"{"type":"restart_ice"}"#);
        assert_eq!(serde_json::from_str::<Signal>(r#"{"type":"restart_ice"}"#).unwrap(), Signal::RestartIce);
    }

    #[test]
    fn hello_round_trips() {
        let h = Signal::Hello { token: None, role: Role::Phone, version: PROTOCOL_VERSION, proof: "ab".into() };
        let s = serde_json::to_string(&h).unwrap();
        assert_eq!(s, r#"{"type":"hello","token":null,"role":"phone","version":1,"proof":"ab"}"#);
        assert_eq!(serde_json::from_str::<Signal>(&s).unwrap(), h);
    }

    #[test]
    fn challenge_carries_the_desktop_nonce() {
        let c = Signal::Challenge { nonce: "n1".into() };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"type":"challenge","nonce":"n1"}"#);
    }

    #[test]
    fn offer_matches_what_the_spike_page_sends() {
        let m: Signal = serde_json::from_str(r#"{"type":"offer","sdp":"v=0"}"#).unwrap();
        assert_eq!(m, Signal::Offer { sdp: "v=0".into() });
    }

    #[test]
    fn set_omits_unset_fields() {
        let c = Ctl::Set { profile: Some(Profile::Quality), zoom: None, torch: None, lens: None };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"type":"set","profile":"quality"}"#);
    }

    #[test]
    fn pong_and_paused_parse() {
        let p: Ctl = serde_json::from_str(r#"{"type":"pong","t":5,"battery":0.8,"charging":true}"#).unwrap();
        assert_eq!(p, Ctl::Pong { t: 5, battery: Some(0.8), charging: Some(true) });
        let z: Ctl = serde_json::from_str(r#"{"type":"paused","reason":"call"}"#).unwrap();
        assert_eq!(z, Ctl::Paused { reason: PauseReason::Call });
    }

    #[test]
    fn caps_round_trips() {
        let j = r#"{"type":"caps","lenses":[{"id":"a","label":"Back"}],"current":"a","zoom_min":1.0,"zoom_max":10.0,"torch":true}"#;
        let c: Ctl = serde_json::from_str(j).unwrap();
        assert_eq!(
            c,
            Ctl::Caps {
                lenses: vec![LensInfo { id: "a".into(), label: "Back".into() }],
                current: "a".into(),
                zoom_min: 1.0,
                zoom_max: 10.0,
                torch: true,
                rotation: None,
                width: None,
                height: None,
                fps: None,
            }
        );
        assert_eq!(serde_json::to_string(&c).unwrap(), j);
        let k: Ctl = serde_json::from_str(r#"{"type":"caps","lenses":[],"current":"","zoom_min":1,"zoom_max":10,"torch":false}"#).unwrap();
        assert!(matches!(k, Ctl::Caps { zoom_max, .. } if zoom_max == 10.0));
    }

    #[test]
    fn caps_with_stream_info_parses() {
        let j = r#"{"type":"caps","lenses":[],"current":"","zoom_min":1,"zoom_max":1,"torch":false,"rotation":90,"width":1080,"height":1920,"fps":29.97}"#;
        match serde_json::from_str::<Ctl>(j).unwrap() {
            Ctl::Caps { rotation, width, height, fps, .. } => {
                assert_eq!((rotation, width, height), (Some(90), Some(1080), Some(1920)));
                assert!((fps.unwrap() - 29.97).abs() < 1e-3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_type_is_an_error_not_a_panic() {
        assert!(serde_json::from_str::<Ctl>(r#"{"type":"nope"}"#).is_err());
    }
}
