//! str0m session configuration. This is where NACK/RTX is decided (spec 7.3a).
//!
//! Round 0 saw `nacks=0` under 3% loss. Root cause: the spike's sender page called
//! `setCodecPreferences()` with only the `video/h264` entries, which removes the `video/rtx`
//! payload types from the offer. str0m enables NACK per stream only when the negotiated payload
//! has an RTX payload type (`suppress_nack = payload.resend.is_none()`), so it never sent one and
//! every lost packet ended in a GAP followed by a PLI. [`force_nack`] makes the receiver NACK even
//! when the sender offers no RTX (libwebrtc/WebKit then resend the original packet on the media
//! SSRC); the sender page should also keep `video/rtx` in its codec preferences.
use std::time::{Duration, Instant};
use str0m::media::Mid;
use str0m::{Rtc, RtcConfig};

/// How long an incomplete frame may wait for a retransmission before it is dropped (GAP -> PLI).
/// NACK goes out within 33 ms of the hole and a LAN retransmission needs about 1 RTT (5 ms),
/// so this leaves room for two NACK rounds while staying a "small jitter buffer" (spec 7.3a).
pub const REORDER_TIMEOUT: Duration = Duration::from_millis(100);

pub fn rtc_config() -> RtcConfig {
    RtcConfig::new()
        .set_ice_lite(true)
        .clear_codecs()
        // Every default H.264 payload type comes with its RTX type and nack/pli/transport-cc feedback.
        .enable_h264(true)
        .set_reordering_timeout_video(Some(REORDER_TIMEOUT))
        .set_stats_interval(Some(Duration::from_secs(1)))
}

pub fn new_rtc(now: Instant) -> Rtc {
    rtc_config().build(now)
}

/// Turn NACK on for the video stream of `mid` even when no RTX was negotiated.
/// Returns false while the stream does not exist yet (call again after the first packet).
pub fn force_nack(rtc: &mut Rtc, mid: Mid) -> bool {
    match rtc.direct_api().stream_rx_by_mid(mid, None) {
        Some(s) => {
            s.suppress_nack(false);
            true
        }
        None => false,
    }
}
