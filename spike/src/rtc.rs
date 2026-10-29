//! str0m ICE-lite receiver loop. One thread owns all Rtc sessions and UDP sockets.
use crate::decode::Au;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::channel::ChannelId;
use str0m::format::Codec;
use str0m::media::{KeyframeRequestKind, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

pub enum Msg {
    Net { sock: usize, src: SocketAddr, data: Vec<u8>, at: Instant },
    Cmd(Cmd),
}
pub enum Cmd {
    Offer { sid: u64, sdp: String, reply: tokio::sync::oneshot::Sender<Result<String, String>> },
    Pli,
}

struct Session {
    sid: u64,
    rtc: Rtc,
    created: Instant,
    video_mid: Option<Mid>,
    ctl: Option<ChannelId>,
    need_key: bool,
    pli_at: Option<Instant>,
    connected_at: Option<Instant>,
    first_key_logged: bool,
    last_pair: String,
    frames_in: u64,
    gaps: u64,
}

pub struct Config {
    pub ips: Vec<Ipv4Addr>,
    pub loss_pct: Arc<AtomicU32>,
}

/// Spawn one blocking reader thread per interface socket. Returns the sockets (for sending).
pub fn bind_sockets(ips: &[Ipv4Addr], tx: Sender<Msg>, loss_pct: Arc<AtomicU32>) -> Vec<Arc<UdpSocket>> {
    let mut socks = vec![];
    for (i, ip) in ips.iter().enumerate() {
        let s = Arc::new(UdpSocket::bind(SocketAddr::new((*ip).into(), 0)).expect("bind udp"));
        let local = s.local_addr().unwrap();
        println!("[rtc] UDP socket #{i} on {local}");
        let s2 = s.clone();
        let tx = tx.clone();
        let loss = loss_pct.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            let mut rng: u64 = 0x9E3779B97F4A7C15;
            loop {
                match s2.recv_from(&mut buf) {
                    Ok((n, src)) => {
                        let p = loss.load(Ordering::Relaxed);
                        if p > 0 && n > 12 && (buf[0] >> 6) == 2 && (buf[1] & 0x7f) >= 96 {
                            // xorshift
                            rng ^= rng << 13;
                            rng ^= rng >> 7;
                            rng ^= rng << 17;
                            if (rng % 1000) < (p as u64) {
                                continue; // simulated RTP loss (p is per-mille)
                            }
                        }
                        if tx.send(Msg::Net { sock: i, src, data: buf[..n].to_vec(), at: Instant::now() }).is_err() {
                            return;
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        socks.push(s);
    }
    socks
}

pub fn run(cfg: Config, rx: Receiver<Msg>, socks: Vec<Arc<UdpSocket>>, dec_tx: Sender<Au>) {
    let _ = cfg;
    let locals: Vec<SocketAddr> = socks.iter().map(|s| s.local_addr().unwrap()).collect();
    let mut sessions: HashMap<u64, Session> = HashMap::new();
    let mut last_stats_log = Instant::now();
    loop {
        // drain outputs
        let mut next = Instant::now() + Duration::from_millis(500);
        for s in sessions.values_mut() {
            loop {
                if !s.rtc.is_alive() {
                    break;
                }
                match s.rtc.poll_output() {
                    Ok(Output::Timeout(t)) => {
                        next = next.min(t);
                        break;
                    }
                    Ok(Output::Transmit(t)) => {
                        if let Some(i) = locals.iter().position(|l| *l == t.source) {
                            let _ = socks[i].send_to(&t.contents, t.destination);
                        } else {
                            println!("[rtc] no socket for transmit source {}", t.source);
                        }
                    }
                    Ok(Output::Event(e)) => handle_event(s, e, &dec_tx),
                    Err(e) => {
                        println!("[rtc] session {} poll_output error: {e:?}", s.sid);
                        s.rtc.disconnect();
                    }
                }
            }
            // keyframe retry if nothing arrived
            if s.need_key {
                if let (Some(t), Some(mid)) = (s.pli_at, s.video_mid) {
                    if t.elapsed() > Duration::from_millis(1000) {
                        println!("[rtc] no keyframe 1s after PLI; re-requesting");
                        send_pli(s, mid);
                    }
                }
            }
        }
        sessions.retain(|_, s| {
            let alive = s.rtc.is_alive();
            if !alive {
                println!("[rtc] session {} gone (alive for {:?}, {} frames, {} gaps)", s.sid, s.created.elapsed(), s.frames_in, s.gaps);
            }
            alive
        });
        if last_stats_log.elapsed() > Duration::from_secs(10) {
            last_stats_log = Instant::now();
        }

        let wait = next.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
        match rx.recv_timeout(wait) {
            Ok(Msg::Net { sock, src, data, at }) => {
                let Ok(contents) = data.as_slice().try_into() else { continue };
                let input = Input::Receive(at, Receive { proto: Protocol::Udp, source: src, destination: locals[sock], contents });
                if let Some(s) = sessions.values_mut().find(|s| s.rtc.accepts(&input)) {
                    if let Err(e) = s.rtc.handle_input(input) {
                        println!("[rtc] session {} handle_input error: {e:?}", s.sid);
                        s.rtc.disconnect();
                    }
                }
            }
            Ok(Msg::Cmd(Cmd::Offer { sid, sdp, reply })) => {
                let r = handle_offer(&mut sessions, sid, &sdp, &locals);
                let _ = reply.send(r);
            }
            Ok(Msg::Cmd(Cmd::Pli)) => {
                for s in sessions.values_mut() {
                    if let Some(mid) = s.video_mid {
                        println!("[rtc] manual PLI requested");
                        s.need_key = true;
                        send_pli(s, mid);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
        let now = Instant::now();
        for s in sessions.values_mut() {
            if s.rtc.is_alive() {
                let _ = s.rtc.handle_input(Input::Timeout(now));
            }
        }
    }
}

fn handle_offer(sessions: &mut HashMap<u64, Session>, sid: u64, sdp: &str, locals: &[SocketAddr]) -> Result<String, String> {
    let offer = SdpOffer::from_sdp_string(sdp).map_err(|e| format!("bad offer: {e:?}"))?;
    for l in sdp.lines().filter(|l| l.starts_with("a=candidate") || l.starts_with("a=ice-options")) {
        println!("[ice] remote: {l}");
    }
    let restart = sessions.contains_key(&sid);
    let s = sessions.entry(sid).or_insert_with(|| {
        let rtc = RtcConfig::new()
            .set_ice_lite(true)
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(1)))
            .build(Instant::now());
        Session {
            sid,
            rtc,
            created: Instant::now(),
            video_mid: None,
            ctl: None,
            need_key: true,
            pli_at: None,
            connected_at: None,
            first_key_logged: false,
            last_pair: String::new(),
            frames_in: 0,
            gaps: 0,
        }
    });
    if restart {
        println!("[ice] session {sid}: re-offer (ICE restart / renegotiation)");
    }
    for l in locals {
        match Candidate::host(*l, "udp") {
            Ok(c) => {
                if s.rtc.add_local_candidate(c).is_some() {
                    println!("[ice] local candidate host {l}");
                }
            }
            Err(e) => println!("[ice] candidate {l}: {e:?}"),
        }
    }
    let answer: SdpAnswer = s.rtc.sdp_api().accept_offer(offer).map_err(|e| format!("accept_offer: {e:?}"))?;
    let a = answer.to_sdp_string();
    for l in a.lines().filter(|l| l.starts_with("a=candidate") || l.starts_with("a=ice-lite") || l.starts_with("m=")) {
        println!("[sdp] answer: {l}");
    }
    Ok(a)
}

fn send_pli(s: &mut Session, mid: Mid) {
    if let Some(mut w) = s.rtc.writer(mid) {
        match w.request_keyframe(None, KeyframeRequestKind::Pli) {
            Ok(()) => {
                s.pli_at = Some(Instant::now());
            }
            Err(e) => println!("[rtc] request_keyframe failed: {e:?}"),
        }
    }
}

fn handle_event(s: &mut Session, e: Event, dec_tx: &Sender<Au>) {
    match e {
        Event::IceConnectionStateChange(st) => {
            println!("[ice] session {} state -> {st:?} (+{} ms)", s.sid, s.created.elapsed().as_millis());
            if st == IceConnectionState::Disconnected {
                println!("[ice] session {} disconnected", s.sid);
            }
        }
        Event::Connected => {
            s.connected_at = Some(Instant::now());
            println!("[rtc] session {} CONNECTED (DTLS done) +{} ms", s.sid, s.created.elapsed().as_millis());
        }
        Event::MediaAdded(m) => {
            println!("[rtc] media added mid={:?} kind={:?} dir={:?}", m.mid, m.kind, m.direction);
            if m.kind == MediaKind::Video {
                s.video_mid = Some(m.mid);
                s.need_key = true;
                send_pli(s, m.mid);
                println!("[rtc] initial PLI sent");
            }
        }
        Event::MediaData(d) => {
            s.frames_in += 1;
            if s.frames_in == 1 {
                println!("[rtc] first MediaData codec={:?} len={} +{} ms", d.params.spec().codec, d.data.len(), s.created.elapsed().as_millis());
            }
            debug_assert!(d.params.spec().codec == Codec::H264);
            if !d.contiguous {
                s.gaps += 1;
                println!("[rtc] GAP: non-contiguous AU (seq {:?}), requesting PLI", d.seq_range);
                s.need_key = true;
                if s.pli_at.is_none() || s.pli_at.unwrap().elapsed() > Duration::from_millis(300) {
                    if let Some(mid) = s.video_mid {
                        send_pli(s, mid);
                    }
                }
            }
            let key = d.is_keyframe();
            if key {
                if let Some(t) = s.pli_at.take() {
                    println!("[pli] KEYFRAME received {} ms after PLI (len {})", t.elapsed().as_millis(), d.data.len());
                } else if !s.first_key_logged {
                    println!("[pli] first KEYFRAME without pending PLI, len {}", d.data.len());
                }
                s.first_key_logged = true;
                s.need_key = false;
            }
            if s.need_key {
                return; // wait for IDR before feeding the decoder
            }
            let _ = dec_tx.send(Au { data: d.data.clone(), key, at: d.network_time });
        }
        Event::ChannelOpen(cid, label) => {
            println!("[dc] channel open id={cid:?} label={label}");
            s.ctl = Some(cid);
        }
        Event::ChannelData(cd) => {
            let txt = String::from_utf8_lossy(&cd.data);
            println!("[dc] data: {}", txt.chars().take(1500).collect::<String>());
            if let Some(mut ch) = s.rtc.channel(cd.id) {
                if txt.contains("\"type\":\"ping\"") {
                    let _ = ch.write(false, txt.replace("ping", "pong").as_bytes());
                }
            }
        }
        Event::PeerStats(ps) => {
            let _ = ps;
        }
        Event::MediaIngressStats(st) => {
            let line = format!("{:?}", st);
            if s.frames_in % 1 == 0 && line.len() < 4000 {
                // log compactly once every ~5 s
                if s.created.elapsed().as_secs() % 5 == 0 {
                    println!("[stats] ingress {}", line.chars().take(300).collect::<String>());
                }
            }
        }
        _ => {}
    }
    let _ = &s.last_pair;
}
