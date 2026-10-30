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
