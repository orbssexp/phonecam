//! Interface enumeration and per-interface UDP sockets (spec 7.1 "runtime interface watch").
//! Round 0 bound sockets only at startup, so a USB-hotspot adapter that appeared later (the
//! Apple Mobile Device Ethernet adapter) was never used. The engine polls every 2 s.
use crate::engine::Msg;
use phonecam_core::path::{classify, PathKind};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    pub ip: Ipv4Addr,
    pub name: String,
    pub kind: PathKind,
}

/// Usable interfaces right now: private IPv4, not loopback or link-local, classified Lan/Usb.
pub fn scan() -> Vec<Iface> {
    let mut v = vec![];
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if let std::net::IpAddr::V4(ip) = i.ip() {
            if i.is_loopback() || ip.is_link_local() {
                continue;
            }
            let kind = classify(&i.name, ip);
            if kind != PathKind::Ignore {
                v.push(Iface { ip, name: i.name, kind });
            }
        }
    }
    v.sort_by_key(|i| i.ip);
    v.dedup_by_key(|i| i.ip);
    v
}

/// (interfaces to add, addresses to drop) given what is bound and what exists now.
pub fn diff(bound: &HashSet<Ipv4Addr>, now: &[Iface]) -> (Vec<Iface>, Vec<Ipv4Addr>) {
    let present: HashSet<Ipv4Addr> = now.iter().map(|i| i.ip).collect();
    let add = now.iter().filter(|i| !bound.contains(&i.ip)).cloned().collect();
    let mut gone: Vec<Ipv4Addr> = bound.iter().filter(|ip| !present.contains(ip)).copied().collect();
    gone.sort();
    (add, gone)
}

struct Sock {
    udp: Arc<UdpSocket>,
    local: SocketAddr,
    stop: Arc<AtomicBool>,
    iface: Iface,
}

#[derive(Default)]
pub struct Changes {
    pub added: Vec<SocketAddr>,
    pub removed: Vec<SocketAddr>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

pub struct Sockets {
    by_ip: HashMap<Ipv4Addr, Sock>,
    tx: Sender<Msg>,
    loss: Arc<AtomicU32>,
    failed: HashSet<Ipv4Addr>,
}

impl Sockets {
    pub fn new(tx: Sender<Msg>, loss: Arc<AtomicU32>) -> Self {
        Self { by_ip: HashMap::new(), tx, loss, failed: HashSet::new() }
    }

    pub fn locals(&self) -> Vec<SocketAddr> {
        let mut v: Vec<_> = self.by_ip.values().map(|s| s.local).collect();
        v.sort();
        v
    }

    pub fn send(&self, source: SocketAddr, dest: SocketAddr, data: &[u8]) -> bool {
        match self.by_ip.get(&ip4(source)).filter(|s| s.local == source) {
            Some(s) => s.udp.send_to(data, dest).is_ok(),
            None => false,
        }
    }

    pub fn kind_of(&self, local: SocketAddr) -> Option<PathKind> {
        self.by_ip.get(&ip4(local)).map(|s| s.iface.kind)
    }

    pub fn has_kind(&self, kind: PathKind) -> bool {
        self.by_ip.values().any(|s| s.iface.kind == kind)
    }

    /// Reconcile the bound sockets with `now`. Returns the candidate addresses that changed.
    pub fn sync(&mut self, now: &[Iface]) -> Changes {
        let bound: HashSet<Ipv4Addr> = self.by_ip.keys().copied().collect();
        let (add, gone) = diff(&bound, now);
        let mut ch = Changes::default();
        for ip in gone {
            if let Some(s) = self.by_ip.remove(&ip) {
                s.stop.store(true, Ordering::Relaxed);
                tracing::info!("[net] interface gone: {} {}", s.iface.name, ip);
                ch.removed.push(s.local);
            }
        }
        self.failed.retain(|ip| now.iter().any(|i| i.ip == *ip));
        for i in add {
            if self.failed.contains(&i.ip) {
                continue;
            }
            match self.bind(&i) {
                Ok(s) => {
                    tracing::info!("[net] interface up: {} {} ({:?}) -> {}", i.name, i.ip, i.kind, s.local);
                    ch.added.push(s.local);
                    self.by_ip.insert(i.ip, s);
                }
                Err(e) => {
                    tracing::warn!("[net] cannot bind {}: {e}", i.ip);
                    self.failed.insert(i.ip);
                }
            }
        }
        ch
    }

    fn bind(&self, iface: &Iface) -> std::io::Result<Sock> {
        let udp = Arc::new(UdpSocket::bind(SocketAddr::new(iface.ip.into(), 0))?);
        udp.set_read_timeout(Some(Duration::from_millis(250)))?;
        let local = udp.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (u2, s2, tx, loss) = (udp.clone(), stop.clone(), self.tx.clone(), self.loss.clone());
        std::thread::spawn(move || reader(u2, local, s2, tx, loss));
        Ok(Sock { udp, local, stop, iface: iface.clone() })
    }

    pub fn stop_all(&mut self) {
        for s in self.by_ip.values() {
            s.stop.store(true, Ordering::Relaxed);
        }
        self.by_ip.clear();
    }
}

fn ip4(a: SocketAddr) -> Ipv4Addr {
    match a.ip() {
        std::net::IpAddr::V4(v) => v,
        _ => Ipv4Addr::UNSPECIFIED,
    }
}

fn reader(udp: Arc<UdpSocket>, local: SocketAddr, stop: Arc<AtomicBool>, tx: Sender<Msg>, loss: Arc<AtomicU32>) {
    let mut buf = vec![0u8; 2048];
    let mut rng: u64 = 0x9E3779B97F4A7C15;
    while !stop.load(Ordering::Relaxed) {
        match udp.recv_from(&mut buf) {
            Ok((n, src)) => {
                let p = loss.load(Ordering::Relaxed);
                if p > 0 && n > 12 && (buf[0] >> 6) == 2 && (buf[1] & 0x7f) >= 96 {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    if (rng % 1000) < p as u64 {
                        continue; // test hook: simulated RTP loss, per mille
                    }
                }
                if tx.send(Msg::Net { local, src, data: buf[..n].to_vec(), at: Instant::now() }).is_err() {
                    return;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)), // e.g. ICMP reset on Windows
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(a: u8, b: u8, c: u8, d: u8, kind: PathKind) -> Iface {
        Iface { ip: Ipv4Addr::new(a, b, c, d), name: "x".into(), kind }
    }

    #[test]
    fn new_usb_adapter_is_added_and_missing_wifi_is_dropped() {
        let wifi = i(192, 168, 1, 5, PathKind::Lan);
        let usb = i(172, 20, 10, 2, PathKind::UsbHotspot);
        let bound: HashSet<_> = [wifi.ip].into_iter().collect();
        let (add, gone) = diff(&bound, std::slice::from_ref(&usb));
        assert_eq!(add, vec![usb]);
        assert_eq!(gone, vec![wifi.ip]);
    }

    #[test]
    fn unchanged_set_has_no_diff() {
        let wifi = i(192, 168, 1, 5, PathKind::Lan);
        let bound: HashSet<_> = [wifi.ip].into_iter().collect();
        let (add, gone) = diff(&bound, &[wifi]);
        assert!(add.is_empty() && gone.is_empty());
    }

    #[test]
    fn sockets_bind_and_release() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut s = Sockets::new(tx, Arc::new(AtomicU32::new(0)));
        // 127.0.0.1 never comes out of `scan()`, but binding works the same for any local IP.
        let lo = Iface { ip: Ipv4Addr::LOCALHOST, name: "lo".into(), kind: PathKind::Lan };
        let ch = s.sync(std::slice::from_ref(&lo));
        assert_eq!(ch.added.len(), 1);
        assert!(s.sync(std::slice::from_ref(&lo)).is_empty());
        let ch = s.sync(&[]);
        assert_eq!(ch.removed.len(), 1);
        assert!(s.locals().is_empty());
    }
}
