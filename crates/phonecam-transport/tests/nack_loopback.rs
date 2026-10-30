//! Round 0 finding (nacks=0): loopback of a str0m sender into the receiver config under 3% RTP loss.
use phonecam_transport::rtc_config::new_rtc;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

fn drain(r: &mut Rtc, now: Instant, from: SocketAddr, to: SocketAddr, out: &mut Vec<(Vec<u8>, SocketAddr, SocketAddr)>, ev: &mut Vec<Event>) -> Instant {
    let _ = (from, to);
    loop {
        match r.poll_output().unwrap() {
            Output::Timeout(t) => return t.max(now),
            Output::Transmit(t) => out.push((t.contents.to_vec(), t.source, t.destination)),
            Output::Event(e) => ev.push(e),
        }
    }
}

#[test]
fn nack_with_loss() {
    str0m::crypto::from_feature_flags().install_process_default();
    let t0 = Instant::now();
    let a_addr: SocketAddr = "10.0.0.1:5000".parse().unwrap();
    let b_addr: SocketAddr = "10.0.0.2:6000".parse().unwrap();
    let mut a = Rtc::builder().build(t0); // sender, full-client
    let mut b = new_rtc(t0); // receiver (ice-lite)
    a.add_local_candidate(Candidate::host(a_addr, "udp").unwrap());
    b.add_local_candidate(Candidate::host(b_addr, "udp").unwrap());
    let mut api = a.sdp_api();
    let mid = api.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    let (offer, pending) = api.apply().unwrap();
    let offer_s = offer.to_sdp_string();
    let answer = b.sdp_api().accept_offer(str0m::change::SdpOffer::from_sdp_string(&offer_s).unwrap()).unwrap();
    println!("ANSWER:\n{}", answer.to_sdp_string());
    a.sdp_api().accept_answer(pending, answer).unwrap();

    let mut now = t0;
    let mut rng: u64 = 0x9E3779B97F4A7C15;
    let mut loss_permille = 0u64;
    let mut pt = None;
    let mut sent_frames = 0u64;
    let mut got = 0u64;
    let mut gaps = 0u64;
    let mut connected = false;
    let mut nacks = 0u64;
    let mut next_frame = t0;
    let mut rtp_time = 0u64;
    let end = t0 + Duration::from_secs(40);
    while now < end {
        let (mut ao, mut bo, mut ae, mut be) = (vec![], vec![], vec![], vec![]);
        let ta = drain(&mut a, now, a_addr, b_addr, &mut ao, &mut ae);
        let tb = drain(&mut b, now, b_addr, a_addr, &mut bo, &mut be);
        for e in ae {
            if let Event::Connected = e {
                connected = true;
            }
        }
        for e in be {
            match e {
                Event::MediaData(d) => {
                    got += 1;
                    if !d.contiguous {
                        gaps += 1;
                    }
                }
                Event::MediaIngressStats(s) => nacks = s.nacks,
                _ => {}
            }
        }
        if pt.is_none() {
            if let Some(w) = a.writer(mid) {
                pt = w.payload_params().find(|p| p.spec().codec == Codec::H264).map(|p| p.pt());
            }
        }
        if now >= t0 + Duration::from_secs(10) {
            loss_permille = 30;
        }
        // route
        for (pkt, src, dst) in ao.into_iter() {
            let is_rtp = pkt.len() > 12 && (pkt[0] >> 6) == 2 && (pkt[1] & 0x7f) >= 96;
            if is_rtp {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                if rng % 1000 < loss_permille {
                    continue;
                }
            }
            let _ = b.handle_input(Input::Receive(now, Receive { proto: Protocol::Udp, source: src, destination: dst, contents: pkt.as_slice().try_into().unwrap() }));
        }
        for (pkt, src, dst) in bo.into_iter() {
            let _ = a.handle_input(Input::Receive(now, Receive { proto: Protocol::Udp, source: src, destination: dst, contents: pkt.as_slice().try_into().unwrap() }));
        }
        if connected && now >= next_frame {
            if let (Some(pt), Some(w)) = (pt, a.writer(mid)) {
                let mut au = vec![0, 0, 0, 1, 0x41];
                au.extend((0..6000).map(|i| (i % 200) as u8 + 1));
                rtp_time += 3000;
                if sent_frames.is_multiple_of(30) {
                    au[4] = 0x65;
                }
                let _ = w.write(pt, now, MediaTime::from_90khz(rtp_time), au);
                sent_frames += 1;
            }
            next_frame += Duration::from_millis(33);
        }
        let next = ta.min(tb).min(if connected { next_frame } else { now + Duration::from_millis(10) });
        now = next.max(now + Duration::from_micros(500));
        let _ = a.handle_input(Input::Timeout(now));
        let _ = b.handle_input(Input::Timeout(now));
    }
    println!("sent={sent_frames} got={got} gaps={gaps} nacks_sent_by_receiver={nacks}");
    assert!(nacks > 0, "receiver never sent a NACK");
    assert!(gaps <= 2, "NACK/RTX should repair 3% loss without gaps, got {gaps}");
    assert!(got + 3 >= sent_frames, "frames lost: sent={sent_frames} got={got}");
}
