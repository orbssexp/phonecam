# phonecam-core Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `crates/phonecam-core`, a pure, portable Rust library containing the PhoneCam protocol types, energy profiles, backoff, heartbeat, path classification, pairing proof, and the connection state machine.

**Architecture:** Pure library with no I/O, no threads and no clock: callers pass the current time in milliseconds and act on the returned actions. That keeps it unit-testable and lets the Windows app (tokio) and the future iOS app (Swift via UniFFI, GCD) drive the same logic. Spec: `docs/superpowers/specs/2026-10-29-phonecam-design.md` §5, §7, §9, §14, §14a.

**Tech Stack:** Rust 2021, serde/serde_json, hmac + sha2 (pure Rust, iOS-safe).

**Out of scope for this plan:** networking (`phonecam-transport`), decoding/virtual camera, UI, UniFFI bindings. Those are separate plans.

---

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml` (repo root) | Cargo workspace; excludes the throwaway `spike/` |
| `crates/phonecam-core/Cargo.toml` | Crate manifest |
| `crates/phonecam-core/src/lib.rs` | Module wiring + re-exports |
| `crates/phonecam-core/src/controls.rs` | `Profile` → `VideoMode` (energy profiles, §9) |
| `crates/phonecam-core/src/protocol.rs` | Wire messages `Signal`, `Ctl`, `Role`, `PauseReason` (§14) |
| `crates/phonecam-core/src/backoff.rs` | Exponential backoff 1 s → 30 s (§7.2) |
| `crates/phonecam-core/src/heartbeat.rs` | Ping scheduling, miss counting, RTT (§7.2) |
| `crates/phonecam-core/src/path.rs` | Interface classification + ICE local preference (§7.1) |
| `crates/phonecam-core/src/pairing.rs` | HMAC pairing proof (§14a) |
| `crates/phonecam-core/src/conn.rs` | Connection state machine → actions incl. virtual-camera frame (§7.2–7.4) |

Each module has its tests in a `#[cfg(test)] mod tests` block at the bottom of the same file.

Environment for every command below (Git Bash):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd /c/Users/gusta/orca/projects/phone-cam
```

---

## Chunk 1: Workspace, profiles, protocol

### Task 1: Workspace + empty crate

**Files:**
- Create: `Cargo.toml`
- Create: `crates/phonecam-core/Cargo.toml`
- Create: `crates/phonecam-core/src/lib.rs`

- [ ] **Step 1: Create the workspace manifest** `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = ["crates/phonecam-core"]
exclude = ["spike"]
```

- [ ] **Step 2: Create the crate manifest** `crates/phonecam-core/Cargo.toml`

```toml
[package]
name = "phonecam-core"
version = "0.1.0"
edition = "2021"
license = "MIT"
description = "Portable PhoneCam core: protocol, profiles, heartbeat, pairing, connection state machine"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
hmac = "0.12"
sha2 = "0.10"
```

- [ ] **Step 3: Create** `crates/phonecam-core/src/lib.rs`

```rust
//! Portable PhoneCam core. No I/O, no threads, no clock: callers pass `now_ms`
//! and act on returned values, so Windows (tokio) and iOS (Swift/GCD) share it.
```

- [ ] **Step 4: Build**

Run: `cargo build -p phonecam-core`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/phonecam-core
git commit -m "feat(core): add cargo workspace and phonecam-core crate"
```

### Task 2: Energy profiles

**Files:**
- Create: `crates/phonecam-core/src/controls.rs`
- Modify: `crates/phonecam-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests** in a new `crates/phonecam-core/src/controls.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_balanced_1080p30() {
        let m = Profile::default().mode();
        assert_eq!((m.width, m.height, m.fps), (1920, 1080, 30));
    }

    #[test]
    fn profiles_map_to_spec_modes() {
        assert_eq!(Profile::Economy.mode(), VideoMode { width: 1280, height: 720, fps: 30, max_bitrate_bps: 2_500_000 });
        assert_eq!(Profile::Balanced.mode(), VideoMode { width: 1920, height: 1080, fps: 30, max_bitrate_bps: 5_000_000 });
        assert_eq!(Profile::Quality.mode(), VideoMode { width: 3840, height: 2160, fps: 30, max_bitrate_bps: 15_000_000 });
    }

    #[test]
    fn profile_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&Profile::Economy).unwrap(), "\"economy\"");
    }
}
```

Add `pub mod controls;` to `lib.rs`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core controls`
Expected: FAIL to compile, with errors like `cannot find type Profile`.

- [ ] **Step 3: Implement** by adding above the tests in `controls.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Energy/quality profile chosen in the UI (spec §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Economy,
    #[default]
    Balanced,
    Quality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub max_bitrate_bps: u32,
}

impl Profile {
    pub fn mode(self) -> VideoMode {
        match self {
            Profile::Economy => VideoMode { width: 1280, height: 720, fps: 30, max_bitrate_bps: 2_500_000 },
            Profile::Balanced => VideoMode { width: 1920, height: 1080, fps: 30, max_bitrate_bps: 5_000_000 },
            Profile::Quality => VideoMode { width: 3840, height: 2160, fps: 30, max_bitrate_bps: 15_000_000 },
        }
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core controls`
Expected: `3 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): energy profiles"
```

### Task 3: Wire protocol

**Files:**
- Create: `crates/phonecam-core/src/protocol.rs`
- Modify: `crates/phonecam-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests** in a new `crates/phonecam-core/src/protocol.rs`

```rust
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
    fn unknown_type_is_an_error_not_a_panic() {
        assert!(serde_json::from_str::<Ctl>(r#"{"type":"nope"}"#).is_err());
    }
}
```

Add `pub mod protocol;` to `lib.rs`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core protocol`
Expected: FAIL to compile (`cannot find type Signal`).

- [ ] **Step 3: Implement** by adding above the tests in `protocol.rs`:

```rust
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
}
```

Note: `Set` deserializes missing fields as `None` because they're `Option`.

**Deferred to the transport plan, on purpose:** the SDP signature (`sig` on `Offer`/`Answer`, spec §14a), `camState` on `Pong`, the `state{}` echo, and `set{res,fps}`. Those fields get added together with the code that produces and consumes them.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core protocol`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): signaling and control protocol types"
```

---

## Chunk 2: Backoff, heartbeat, paths, pairing

### Task 4: Backoff

**Files:**
- Create: `crates/phonecam-core/src/backoff.rs`
- Modify: `crates/phonecam-core/src/lib.rs` (add `pub mod backoff;`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_1s_and_caps_at_30s() {
        let mut b = Backoff::new(1_000, 30_000);
        let got: Vec<u64> = (0..7).map(|_| b.next_delay_ms()).collect();
        assert_eq!(got, vec![1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]);
    }

    #[test]
    fn reset_starts_over() {
        let mut b = Backoff::new(1_000, 30_000);
        b.next_delay_ms();
        b.next_delay_ms();
        b.reset();
        assert_eq!(b.next_delay_ms(), 1_000);
    }

    #[test]
    fn never_overflows() {
        let mut b = Backoff::new(1_000, 30_000);
        for _ in 0..200 { assert!(b.next_delay_ms() <= 30_000); }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core backoff`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
/// Exponential backoff: base, 2×base, 4×base … capped. Spec §7.2.
#[derive(Debug, Clone)]
pub struct Backoff {
    attempt: u32,
    base_ms: u64,
    cap_ms: u64,
}

impl Backoff {
    pub fn new(base_ms: u64, cap_ms: u64) -> Self {
        Self { attempt: 0, base_ms, cap_ms }
    }

    pub fn next_delay_ms(&mut self) -> u64 {
        let shift = self.attempt.min(32);
        let d = self.base_ms.saturating_mul(1u64 << shift).min(self.cap_ms);
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core backoff`
Expected: `3 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): exponential backoff"
```

### Task 5: Heartbeat

**Files:**
- Create: `crates/phonecam-core/src/heartbeat.rs`
- Modify: `crates/phonecam-core/src/lib.rs` (add `pub mod heartbeat;`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pings_every_interval() {
        let mut h = Heartbeat::new(2_000, 3, 0);
        assert_eq!(h.tick(1_999), None);
        assert_eq!(h.tick(2_000), Some(HbAction::SendPing { t: 2_000 }));
        assert_eq!(h.tick(2_500), None);
        assert_eq!(h.tick(4_000), Some(HbAction::SendPing { t: 4_000 }));
    }

    #[test]
    fn lost_after_6s_without_pong() {
        // Spec §7.2: "3 missed pongs (6 s)". Pings at 2 s and 4 s go unanswered; at 6 s, Lost.
        let mut h = Heartbeat::new(2_000, 3, 0);
        assert_eq!(h.tick(2_000), Some(HbAction::SendPing { t: 2_000 }));
        assert_eq!(h.tick(4_000), Some(HbAction::SendPing { t: 4_000 }));
        assert_eq!(h.tick(6_000), Some(HbAction::Lost));
    }

    #[test]
    fn pong_clears_misses_and_measures_rtt() {
        let mut h = Heartbeat::new(2_000, 3, 0);
        h.tick(2_000);
        h.tick(4_000);
        h.pong(4_000, 4_007);
        assert_eq!(h.rtt_ms(), Some(7));
        assert_eq!(h.tick(6_000), Some(HbAction::SendPing { t: 6_000 }));
        assert_eq!(h.tick(8_000), Some(HbAction::SendPing { t: 8_000 }));
        assert_eq!(h.tick(10_000), Some(HbAction::Lost));
    }

    #[test]
    fn reset_restarts_the_schedule() {
        let mut h = Heartbeat::new(2_000, 3, 0);
        h.tick(2_000);
        h.tick(4_000);
        h.reset(10_000);
        assert_eq!(h.tick(11_000), None);
        assert_eq!(h.tick(12_000), Some(HbAction::SendPing { t: 12_000 }));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core heartbeat`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
/// What the caller should do after a tick. Spec §7.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HbAction {
    /// Send `Ctl::Ping { t }` on the DataChannel. (`pong` doesn't match `t` against
    /// outstanding pings: any pong proves the link is alive, which is all we need.)
    SendPing { t: u64 },
    /// `miss_limit` pings went unanswered. Repeats every interval until `pong`/`reset`.
    Lost,
}

#[derive(Debug, Clone)]
pub struct Heartbeat {
    interval_ms: u64,
    miss_limit: u32,
    next_ping_at: u64,
    outstanding: u32,
    rtt_ms: Option<u64>,
}

impl Heartbeat {
    pub fn new(interval_ms: u64, miss_limit: u32, now_ms: u64) -> Self {
        Self { interval_ms, miss_limit, next_ping_at: now_ms + interval_ms, outstanding: 0, rtt_ms: None }
    }

    pub fn tick(&mut self, now_ms: u64) -> Option<HbAction> {
        if now_ms < self.next_ping_at {
            return None;
        }
        self.next_ping_at = now_ms + self.interval_ms;
        // With interval 2 s and limit 3, this fires 6 s after the last pong (spec §7.2).
        if self.outstanding + 1 >= self.miss_limit {
            return Some(HbAction::Lost);
        }
        self.outstanding += 1;
        Some(HbAction::SendPing { t: now_ms })
    }

    pub fn pong(&mut self, t: u64, now_ms: u64) {
        self.outstanding = 0;
        self.rtt_ms = Some(now_ms.saturating_sub(t));
    }

    pub fn rtt_ms(&self) -> Option<u64> {
        self.rtt_ms
    }

    pub fn reset(&mut self, now_ms: u64) {
        self.outstanding = 0;
        self.next_ping_at = now_ms + self.interval_ms;
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core heartbeat`
Expected: `4 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): heartbeat with miss detection and rtt"
```

### Task 6: Path classification

**Naming note:** spec §7.1 says "Wi-Fi > USB". This plan uses `Lan > UsbHotspot` for the same rule, and the spec has been updated to match. The desktop may be on Ethernet or Wi-Fi; both reach the phone over the LAN. Round 0 showed the user's PC is on Ethernet, so the classes are **Lan** (preferred) and **UsbHotspot** (fallback), not Wi-Fi vs USB.

**Files:**
- Create: `crates/phonecam-core/src/path.rs`
- Modify: `crates/phonecam-core/src/lib.rs` (add `pub mod path;`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn apple_adapter_is_usb_hotspot() {
        assert_eq!(classify("Apple Mobile Device Ethernet", Ipv4Addr::new(172, 20, 10, 2)), PathKind::UsbHotspot);
    }

    #[test]
    fn hotspot_subnet_is_usb_even_with_generic_name() {
        assert_eq!(classify("Ethernet 3", Ipv4Addr::new(172, 20, 10, 5)), PathKind::UsbHotspot);
    }

    #[test]
    fn outside_hotspot_slash_28_is_lan() {
        assert_eq!(classify("Ethernet 3", Ipv4Addr::new(172, 20, 10, 16)), PathKind::Lan);
    }

    #[test]
    fn home_network_is_lan() {
        assert_eq!(classify("Ethernet", Ipv4Addr::new(192, 168, 15, 145)), PathKind::Lan);
        assert_eq!(classify("Wi-Fi", Ipv4Addr::new(10, 0, 0, 7)), PathKind::Lan);
    }

    #[test]
    fn public_and_loopback_are_ignored() {
        assert_eq!(classify("Ethernet", Ipv4Addr::new(8, 8, 8, 8)), PathKind::Ignore);
        assert_eq!(classify("Loopback", Ipv4Addr::LOCALHOST), PathKind::Ignore);
    }

    #[test]
    fn lan_outranks_usb() {
        assert!(local_preference(PathKind::Lan) > local_preference(PathKind::UsbHotspot));
        assert_eq!(local_preference(PathKind::Ignore), 0);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core path`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
use std::net::Ipv4Addr;

/// Which path an interface gives us to the phone. Spec §7.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Home/office network (Wi-Fi or Ethernet on the PC side). Preferred.
    Lan,
    /// iPhone Personal Hotspot over USB (172.20.10.0/28). Fallback.
    UsbHotspot,
    /// Public, loopback or link-local: never offered as a candidate.
    Ignore,
}

fn in_usb_hotspot_subnet(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    a == 172 && b == 20 && c == 10 && d < 16
}

pub fn classify(adapter_description: &str, ip: Ipv4Addr) -> PathKind {
    if adapter_description.to_ascii_lowercase().contains("apple mobile device") || in_usb_hotspot_subnet(ip) {
        PathKind::UsbHotspot
    } else if ip.is_private() {
        PathKind::Lan
    } else {
        PathKind::Ignore
    }
}

/// ICE candidate local preference (RFC 8445 §5.1.2.1); higher wins.
pub fn local_preference(kind: PathKind) -> u16 {
    match kind {
        PathKind::Lan => 65_535,
        PathKind::UsbHotspot => 32_767,
        PathKind::Ignore => 0,
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core path`
Expected: `6 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): interface path classification and ICE preference"
```

### Task 7: Pairing proof

**Files:**
- Create: `crates/phonecam-core/src/pairing.rs`
- Modify: `crates/phonecam-core/src/lib.rs` (add `pub mod pairing;`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const K: &[u8] = b"0123456789abcdef";
    const N: &[u8] = b"nonce-1";

    #[test]
    fn proof_verifies_with_same_secret_and_nonce() {
        let p = proof_hex(K, N);
        assert_eq!(p.len(), 64);
        assert!(verify_hex(K, N, &p));
    }

    #[test]
    fn wrong_secret_or_nonce_fails() {
        let p = proof_hex(K, N);
        assert!(!verify_hex(b"fedcba9876543210", N, &p));
        assert!(!verify_hex(K, b"nonce-2", &p));
    }

    #[test]
    fn malformed_or_truncated_proof_fails() {
        let p = proof_hex(K, N);
        assert!(!verify_hex(K, N, &p[..62]));
        assert!(!verify_hex(K, N, "zz"));
        assert!(!verify_hex(K, N, ""));
    }

    #[test]
    fn non_hex_characters_are_rejected() {
        // u8::from_str_radix accepts a leading '+'; a proof must have one canonical encoding.
        let p = proof_hex(K, N);
        let plus = format!("+{}", &p[1..]);
        assert!(!verify_hex(K, N, &plus));
    }

    #[test]
    fn proof_is_deterministic() {
        assert_eq!(proof_hex(K, N), proof_hex(K, N));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core pairing`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
//! Pairing proof (spec §14a): the desktop sends a nonce; the phone answers with
//! HMAC-SHA256(secret, DOMAIN || nonce). The secret only travels in the QR URL fragment.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const DOMAIN: &[u8] = b"phonecam-pair-v1";

fn mac(secret: &[u8], nonce: &[u8]) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    m.update(DOMAIN);
    m.update(nonce);
    m
}

pub fn proof_hex(secret: &[u8], nonce: &[u8]) -> String {
    mac(secret, nonce).finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time check. Returns false for any malformed input.
pub fn verify_hex(secret: &[u8], nonce: &[u8], candidate: &str) -> bool {
    match decode_hex(candidate) {
        Some(bytes) => mac(secret, nonce).verify_slice(&bytes).is_ok(),
        None => false,
    }
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core pairing`
Expected: `5 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): HMAC pairing proof"
```

---

## Chunk 3: Connection state machine + portability gate

### Task 8: Connection state machine

The state machine decides **what the virtual camera shows** and **when to ask the phone for an ICE restart**. It never does I/O itself.

Transitions (spec §7.2–7.4):

| From | Event | To | Actions |
|---|---|---|---|
| Idle / Reconnecting | `PhoneJoined` | Connecting | none |
| Connecting | `ConnectTimedOut` (transport fires it 10 s after `PhoneJoined` without media) | Reconnecting{1} | `SetVcam(Reconnecting)`, `SendRestartIce`, `ScheduleRetry{10_000}` |
| Connecting / Reconnecting | `MediaConnected` | Live | `SetVcam(Camera)`; backoff reset |
| Live / Paused | `HeartbeatLost` | Reconnecting{1} | `SetVcam(Reconnecting)`, `SendRestartIce`, `ScheduleRetry{10_000}` |
| Reconnecting{n} | `RetryTimerFired` | Reconnecting{n+1} | `SendRestartIce`, `ScheduleRetry{backoff}` |
| Reconnecting | `HeartbeatLost` | Reconnecting | none (already handling it) |
| Live | `CameraPaused(r)` | Paused(r) | `SetVcam(Paused)` |
| Paused | `CameraResumed` | Live | `SetVcam(Camera)` |
| Connecting / Live / Paused | `PhoneLeft` | Reconnecting{1} | `SetVcam(Reconnecting)` (waits for `PhoneJoined`; no restart without a peer) |
| Idle | `PhoneLeft` | Idle | none |
| Reconnecting | `PhoneLeft` | Reconnecting | none (already waiting for the phone) |
| any other combination | — | unchanged | none |

`Idle` shows the placeholder frame. The caller sets it once at startup via `Conn::initial_vcam()`.

**What `SendRestartIce` means for transport:** send `Signal::RestartIce` over the DataChannel if it's open, otherwise through the relay. With the relay design, "full re-signal" (spec §7.2) is the same message on the relay path: the phone re-offers. That's why core has no separate `Resignal` action. Deferred to transport: the "Wi-Fi stable for 30 s → move back" restart (§7.1), which depends on interface watching.

**Files:**
- Create: `crates/phonecam-core/src/conn.rs`
- Modify: `crates/phonecam-core/src/lib.rs` (add `pub mod conn;`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PauseReason;

    fn live() -> Conn {
        let mut c = Conn::new();
        c.handle(Event::PhoneJoined);
        c.handle(Event::MediaConnected);
        c
    }

    #[test]
    fn starts_idle_with_placeholder() {
        let c = Conn::new();
        assert_eq!(c.state(), ConnState::Idle);
        assert_eq!(Conn::initial_vcam(), VcamFrame::Placeholder);
    }

    #[test]
    fn join_then_media_goes_live() {
        let mut c = Conn::new();
        assert!(c.handle(Event::PhoneJoined).is_empty());
        assert_eq!(c.state(), ConnState::Connecting);
        assert_eq!(c.handle(Event::MediaConnected), vec![Action::SetVcam(VcamFrame::Camera)]);
        assert_eq!(c.state(), ConnState::Live);
    }

    #[test]
    fn connect_timeout_starts_restarting() {
        let mut c = Conn::new();
        c.handle(Event::PhoneJoined);
        assert_eq!(c.handle(Event::ConnectTimedOut), vec![
            Action::SetVcam(VcamFrame::Reconnecting),
            Action::SendRestartIce,
            Action::ScheduleRetry { ms: 10_000 },
        ]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
    }

    #[test]
    fn heartbeat_loss_asks_for_ice_restart() {
        let mut c = live();
        let a = c.handle(Event::HeartbeatLost);
        assert_eq!(a, vec![
            Action::SetVcam(VcamFrame::Reconnecting),
            Action::SendRestartIce,
            Action::ScheduleRetry { ms: 10_000 },
        ]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
    }

    #[test]
    fn repeated_loss_while_reconnecting_is_ignored() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        assert!(c.handle(Event::HeartbeatLost).is_empty());
    }

    #[test]
    fn retries_back_off_from_1s() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        assert_eq!(c.handle(Event::RetryTimerFired), vec![Action::SendRestartIce, Action::ScheduleRetry { ms: 1_000 }]);
        assert_eq!(c.handle(Event::RetryTimerFired), vec![Action::SendRestartIce, Action::ScheduleRetry { ms: 2_000 }]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 3 });
    }

    #[test]
    fn recovery_resets_backoff() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        c.handle(Event::RetryTimerFired);
        c.handle(Event::RetryTimerFired);
        c.handle(Event::MediaConnected);
        assert_eq!(c.state(), ConnState::Live);
        c.handle(Event::HeartbeatLost);
        assert_eq!(c.handle(Event::RetryTimerFired), vec![Action::SendRestartIce, Action::ScheduleRetry { ms: 1_000 }]);
    }

    #[test]
    fn call_pauses_and_resumes() {
        let mut c = live();
        assert_eq!(c.handle(Event::CameraPaused(PauseReason::Call)), vec![Action::SetVcam(VcamFrame::Paused)]);
        assert_eq!(c.state(), ConnState::Paused(PauseReason::Call));
        assert_eq!(c.handle(Event::CameraResumed), vec![Action::SetVcam(VcamFrame::Camera)]);
        assert_eq!(c.state(), ConnState::Live);
    }

    #[test]
    fn loss_while_paused_reconnects() {
        let mut c = live();
        c.handle(Event::CameraPaused(PauseReason::Locked));
        assert_eq!(c.handle(Event::HeartbeatLost)[0], Action::SetVcam(VcamFrame::Reconnecting));
    }

    #[test]
    fn phone_leaving_waits_without_restart() {
        let mut c = live();
        assert_eq!(c.handle(Event::PhoneLeft), vec![Action::SetVcam(VcamFrame::Reconnecting)]);
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
        c.handle(Event::PhoneJoined);
        assert_eq!(c.state(), ConnState::Connecting);
    }

    #[test]
    fn phone_leaving_while_already_reconnecting_is_ignored() {
        let mut c = live();
        c.handle(Event::HeartbeatLost);
        assert!(c.handle(Event::PhoneLeft).is_empty());
        assert_eq!(c.state(), ConnState::Reconnecting { attempt: 1 });
    }

    #[test]
    fn irrelevant_events_do_nothing() {
        let mut c = Conn::new();
        assert!(c.handle(Event::HeartbeatLost).is_empty());
        assert!(c.handle(Event::CameraResumed).is_empty());
        assert!(c.handle(Event::RetryTimerFired).is_empty());
        assert!(c.handle(Event::PhoneLeft).is_empty());
        assert_eq!(c.state(), ConnState::Idle);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phonecam-core conn`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
use crate::backoff::Backoff;
use crate::protocol::PauseReason;

/// First retry waits for the ICE restart to complete; later ones back off from 1 s.
const FIRST_RESTART_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Idle,
    Connecting,
    Live,
    Paused(PauseReason),
    Reconnecting { attempt: u32 },
}

/// What the "PhoneCam" virtual camera should show (spec §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcamFrame {
    Placeholder,
    Camera,
    Reconnecting,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    PhoneJoined,
    MediaConnected,
    /// No media within 10 s of `PhoneJoined` (timer owned by transport).
    ConnectTimedOut,
    HeartbeatLost,
    RetryTimerFired,
    CameraPaused(PauseReason),
    CameraResumed,
    PhoneLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SetVcam(VcamFrame),
    /// Send `Signal::RestartIce` to the phone (DataChannel first, relay fallback).
    SendRestartIce,
    /// Call `handle(Event::RetryTimerFired)` after `ms`, unless state changed.
    ScheduleRetry { ms: u64 },
}

#[derive(Debug, Clone)]
pub struct Conn {
    state: ConnState,
    backoff: Backoff,
}

impl Default for Conn {
    fn default() -> Self {
        Self::new()
    }
}

impl Conn {
    pub fn new() -> Self {
        Self { state: ConnState::Idle, backoff: Backoff::new(1_000, 30_000) }
    }

    pub fn initial_vcam() -> VcamFrame {
        VcamFrame::Placeholder
    }

    pub fn state(&self) -> ConnState {
        self.state
    }

    pub fn handle(&mut self, ev: Event) -> Vec<Action> {
        use ConnState::*;
        match (self.state, ev) {
            (Idle | Reconnecting { .. }, Event::PhoneJoined) => {
                self.state = Connecting;
                vec![]
            }
            (Connecting | Reconnecting { .. }, Event::MediaConnected) => {
                self.state = Live;
                self.backoff.reset();
                vec![Action::SetVcam(VcamFrame::Camera)]
            }
            (Live | Paused(_), Event::HeartbeatLost) | (Connecting, Event::ConnectTimedOut) => {
                self.state = Reconnecting { attempt: 1 };
                vec![
                    Action::SetVcam(VcamFrame::Reconnecting),
                    Action::SendRestartIce,
                    Action::ScheduleRetry { ms: FIRST_RESTART_TIMEOUT_MS },
                ]
            }
            (Reconnecting { attempt }, Event::RetryTimerFired) => {
                self.state = Reconnecting { attempt: attempt + 1 };
                vec![Action::SendRestartIce, Action::ScheduleRetry { ms: self.backoff.next_delay_ms() }]
            }
            (Live, Event::CameraPaused(r)) => {
                self.state = Paused(r);
                vec![Action::SetVcam(VcamFrame::Paused)]
            }
            (Paused(_), Event::CameraResumed) => {
                self.state = Live;
                vec![Action::SetVcam(VcamFrame::Camera)]
            }
            (Connecting | Live | Paused(_), Event::PhoneLeft) => {
                self.state = Reconnecting { attempt: 1 };
                vec![Action::SetVcam(VcamFrame::Reconnecting)]
            }
            _ => vec![],
        }
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p phonecam-core conn`
Expected: `12 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/phonecam-core
git commit -m "feat(core): connection state machine"
```

### Task 9: Re-exports, lint, portability gate

**Files:**
- Modify: `crates/phonecam-core/src/lib.rs`

- [ ] **Step 1: Finalize** `lib.rs` (replace the whole file)

```rust
//! Portable PhoneCam core. No I/O, no threads, no clock: callers pass `now_ms`
//! and act on returned values, so Windows (tokio) and iOS (Swift/GCD) share it.

pub mod backoff;
pub mod conn;
pub mod controls;
pub mod heartbeat;
pub mod pairing;
pub mod path;
pub mod protocol;

pub use conn::{Action, Conn, ConnState, Event, VcamFrame};
pub use controls::{Profile, VideoMode};
pub use protocol::{Ctl, PauseReason, Role, Signal, PROTOCOL_VERSION};
```

- [ ] **Step 2: Full test + lint**

Run: `cargo test -p phonecam-core && cargo clippy -p phonecam-core -- -D warnings`
Expected: `40 passed` (3+7+3+4+6+5+12) and clippy finishes with no warnings.

- [ ] **Step 3: Portability gate (iOS target)**

Run:
```bash
rustup target add aarch64-apple-ios
cargo check -p phonecam-core --target aarch64-apple-ios
```
Expected: `Finished`. This proves the core has no Windows-only dependency. `check` doesn't link, so no Apple SDK is needed.

- [ ] **Step 4: Commit**

```bash
git add crates/phonecam-core Cargo.lock
git commit -m "feat(core): public API re-exports; verified iOS target check"
```

---

## Next plans (not in this document)

1. `phonecam-transport`: str0m receiver with NACK/RTX/TWCC, interface watch, relay client, drives `Conn` and `Heartbeat`.
2. `decode-win` + `vcam-win`: NVIDIA-first decode with fixed-size scaled output to softcam, plus placeholder frames.
3. `apps/web-client` + `apps/relay`: production phone page (pinch zoom, Dual Wide default, black screen, pairing proof) and the relay moved out of `spike/`.
4. `apps/desktop`: Tauri + shadcn UI with tray lifecycle.
