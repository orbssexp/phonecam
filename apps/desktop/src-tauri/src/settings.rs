//! User settings + pairing persistence under `%APPDATA%\PhoneCam`.

use phonecam_core::{Ctl, Profile};
use phonecam_media_win::DecoderPref;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decoder {
    #[default]
    Auto,
    Cuda,
    Cpu,
}

impl Decoder {
    pub fn pref(self) -> DecoderPref {
        match self {
            Decoder::Auto => DecoderPref::Auto,
            Decoder::Cuda => DecoderPref::Cuda,
            Decoder::Cpu => DecoderPref::Cpu,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub lens: Option<String>,
    pub profile: Profile,
    pub zoom: f32,
    pub torch: bool,
    pub mirror: bool,
    pub decoder: Decoder,
}

impl Default for Settings {
    fn default() -> Self {
        Self { lens: None, profile: Profile::Balanced, zoom: 1.0, torch: false, mirror: false, decoder: Decoder::Auto }
    }
}

/// `Partial<Settings>` from the UI. `lens: null` clears the lens; a missing key leaves it alone.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    #[serde(deserialize_with = "some_option")]
    pub lens: Option<Option<String>>,
    pub profile: Option<Profile>,
    pub zoom: Option<f32>,
    pub torch: Option<bool>,
    pub mirror: Option<bool>,
    pub decoder: Option<Decoder>,
}

fn some_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

/// What a patch actually changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Diff {
    pub lens: bool,
    pub profile: bool,
    pub zoom: bool,
    pub torch: bool,
    pub mirror: bool,
    pub decoder: bool,
}

impl Diff {
    pub fn any_phone(&self) -> bool {
        self.lens || self.profile || self.zoom || self.torch
    }
}

impl Settings {
    /// Apply `p`; zoom is clamped into `zoom_range` when the phone has reported one.
    pub fn apply(&mut self, p: SettingsPatch, zoom_range: Option<(f32, f32)>) -> Diff {
        let mut d = Diff::default();
        if let Some(v) = p.lens {
            d.lens = self.lens != v;
            self.lens = v;
        }
        if let Some(v) = p.profile {
            d.profile = self.profile != v;
            self.profile = v;
        }
        if let Some(mut v) = p.zoom.filter(|z| z.is_finite() && *z > 0.0) {
            if let Some((lo, hi)) = zoom_range {
                v = v.clamp(lo, hi.max(lo));
            }
            d.zoom = (self.zoom - v).abs() > f32::EPSILON;
            self.zoom = v;
        }
        if let Some(v) = p.torch {
            d.torch = self.torch != v;
            self.torch = v;
        }
        if let Some(v) = p.mirror {
            d.mirror = self.mirror != v;
            self.mirror = v;
        }
        if let Some(v) = p.decoder {
            d.decoder = self.decoder != v;
            self.decoder = v;
        }
        d
    }

    /// `Ctl::Set` carrying only the fields in `d` (or everything when `all`).
    pub fn ctl_set(&self, d: Diff, all: bool) -> Ctl {
        Ctl::Set {
            profile: (all || d.profile).then_some(self.profile),
            zoom: (all || d.zoom).then_some(self.zoom),
            torch: (all || d.torch).then_some(self.torch),
            lens: if all || d.lens { self.lens.clone() } else { None },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub room_id: String,
    pub secret_hex: String,
}

impl Pairing {
    pub fn generate() -> Self {
        let mut room = [0u8; 16];
        let mut secret = [0u8; 32];
        getrandom::getrandom(&mut room).expect("os rng");
        getrandom::getrandom(&mut secret).expect("os rng");
        use base64::Engine;
        Self {
            room_id: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(room),
            secret_hex: secret.iter().map(|b| format!("{b:02x}")).collect(),
        }
    }

    pub fn secret(&self) -> Vec<u8> {
        (0..self.secret_hex.len() / 2)
            .filter_map(|i| u8::from_str_radix(self.secret_hex.get(2 * i..2 * i + 2)?, 16).ok())
            .collect()
    }

    fn valid(&self) -> bool {
        (22..=128).contains(&self.room_id.len())
            && self.room_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && self.secret_hex.len() == 64
            && self.secret().len() == 32
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("PhoneCam")
}

pub fn load_settings(dir: &Path) -> Settings {
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(dir: &Path, s: &Settings) {
    write_json(dir, "settings.json", s);
}

pub fn load_or_create_pairing(dir: &Path) -> Pairing {
    let loaded: Option<Pairing> = std::fs::read_to_string(dir.join("pairing.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(Pairing::valid);
    loaded.unwrap_or_else(|| {
        let p = Pairing::generate();
        save_pairing(dir, &p);
        p
    })
}

pub fn save_pairing(dir: &Path, p: &Pairing) {
    write_json(dir, "pairing.json", p);
}

fn write_json<T: Serialize>(dir: &Path, name: &str, v: &T) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
        std::fs::rename(&tmp, dir.join(name))
    };
    if let Err(e) = write() {
        tracing::warn!("could not save {name}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("phonecam-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn patch_distinguishes_missing_and_null_lens() {
        let p: SettingsPatch = serde_json::from_str(r#"{"zoom":2}"#).unwrap();
        assert!(p.lens.is_none());
        let p: SettingsPatch = serde_json::from_str(r#"{"lens":null}"#).unwrap();
        assert_eq!(p.lens, Some(None));
        let p: SettingsPatch = serde_json::from_str(r#"{"lens":"x","decoder":"cuda"}"#).unwrap();
        assert_eq!(p.lens, Some(Some("x".into())));
        assert_eq!(p.decoder, Some(Decoder::Cuda));
    }

    #[test]
    fn apply_reports_only_real_changes_and_clamps_zoom() {
        let mut s = Settings::default();
        let p = SettingsPatch { zoom: Some(50.0), torch: Some(false), mirror: Some(true), ..Default::default() };
        let d = s.apply(p, Some((1.0, 10.0)));
        assert_eq!(s.zoom, 10.0);
        assert!(d.zoom && d.mirror && !d.torch);
        assert!(d.any_phone());
        match s.ctl_set(d, false) {
            Ctl::Set { zoom, torch, profile, lens } => {
                assert_eq!(zoom, Some(10.0));
                assert!(torch.is_none() && profile.is_none() && lens.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn settings_round_trip_through_disk_and_tolerate_garbage() {
        let d = tmp("settings");
        let s = Settings { profile: Profile::Quality, lens: Some("cam1".into()), ..Default::default() };
        save_settings(&d, &s);
        assert_eq!(load_settings(&d), s);
        std::fs::write(d.join("settings.json"), "{nope").unwrap();
        assert_eq!(load_settings(&d), Settings::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pairing_is_stable_until_regenerated() {
        let d = tmp("pairing");
        let a = load_or_create_pairing(&d);
        assert_eq!(a.room_id.len(), 22);
        assert_eq!(a.secret().len(), 32);
        assert_eq!(load_or_create_pairing(&d), a);
        let b = Pairing::generate();
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&d);
    }
}
