# Deep-research prompt: transport methods for iPhone -> Windows live video

Goal: find out whether any transport is faster (lower latency) or more stable than what we use.

Baselines to compare against:
- **A, ours ("native v1")**: native iOS app (AVFoundation + VideoToolbox H.264, real time, no
  B-frames) -> one TCP connection on the LAN (phone listens, mDNS discovery, HMAC-SHA256 mutual
  auth, no encryption, framing `u32 len | type | payload`, one H.264 Annex-B access unit per frame,
  JSON control on the same socket, TCP_NODELAY, sender drops to the next keyframe on backlog) ->
  Rust receiver, FFmpeg D3D11VA decode, DirectShow virtual camera.
  Profiles: 720p30 5 Mbps, 1080p30 10, 1080p60 16, 4K30 25-30.
- **B, previous**: Safari page over WebRTC (H.264 RTP/UDP, DTLS-SRTP, NACK/PLI) to a Rust ICE-lite
  receiver (str0m) with a WebSocket relay for signalling. Capped at 1080p, froze under loss.
- **C, Iriun Webcam**: native app, mDNS, single TCP connection, USB via usbmuxd, FFmpeg decode.

Alternatives to evaluate: tuned TCP (buffers, TCP_NOTSENT_LOWAT, congestion control); custom
UDP/RTP with NACK/RTX and FEC; QUIC streams vs datagrams (RFC 9221), Media over QUIC, RTP over
QUIC; SRT and RIST; native libwebrtc; NDI / NDI HX; USB (usbmuxd tunnel, USB networking, UVC);
router-less links (Wi-Fi Aware, AWDL, Multipeer Connectivity); codec as a variable (H.264 vs HEVC
vs AV1, intra-refresh, slices, long-term reference frames).

For each: measured end-to-end latency on 5 GHz Wi-Fi and USB (with source); behaviour at 0.1 %, 1 %
and 5 % loss and 20-50 ms jitter; how much TCP head-of-line blocking matters at 10-30 Mbps on a
LAN; recovery after screen lock, backgrounding and network change; iOS restrictions (background,
local-network permission, free Apple ID sideloading); Windows firewall/NAT needs; mature Swift and
Rust libraries with licences; effort relative to A; who uses it in production (Camo, EpocCam,
DroidCam, Iriun, NDI HX Camera, Continuity Camera, Moonlight/Sunshine, Parsec, Steam Link).

Output: comparison table; per-method pros/cons with sources; ranked recommendation (cheap wins for
A now, best alternative if A is not enough, what is not worth it); a separate list of what could
not be confirmed. Prefer primary sources and separate measurement from opinion.
