use anyhow::Result;
use rcgen::{
    date_time_ymd, BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use std::net::{IpAddr, Ipv4Addr};

pub struct Certs {
    pub ca_der: Vec<u8>,
    pub ca_pem: String,
    pub leaf_chain_pem: String,
    pub leaf_key_pem: String,
}

/// CA key + cert persist in %LOCALAPPDATA%\PhoneCam-spike so the profile installed on the
/// iPhone keeps matching across runs. Only the leaf is regenerated.
fn ca_store() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(Into::into).unwrap_or_else(|| std::env::temp_dir());
    std::path::PathBuf::from(base).join("PhoneCam-spike")
}

/// Local CA with Name Constraints (private ranges + phonecam.local) and a leaf with SAN IPs, <=398 days.
pub fn make(ips: &[Ipv4Addr]) -> Result<Certs> {
    let dir = ca_store();
    let (key_path, der_path) = (dir.join("ca-key.pem"), dir.join("ca-cert.der"));
    let stored = std::fs::read_to_string(&key_path).ok().zip(std::fs::read(&der_path).ok());
    let ca_key = match &stored {
        Some((pem, _)) => KeyPair::from_pem(pem)?,
        None => KeyPair::generate()?,
    };
    let mut ca = CertificateParams::new(vec![])?;
    ca.distinguished_name.push(DnType::CommonName, "PhoneCam Local CA (spike)");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.not_before = date_time_ymd(2026, 10, 1);
    ca.not_after = date_time_ymd(2036, 10, 1);
    ca.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([10, 0, 0, 0], 8)),
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([172, 16, 0, 0], 12)),
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([192, 168, 0, 0], 16)),
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([127, 0, 0, 0], 8)),
            GeneralSubtree::DnsName("phonecam.local".into()),
        ],
        excluded_subtrees: vec![],
    });
    let ca_der = match stored {
        Some((_, der)) => der,
        None => {
            let der = ca.self_signed(&ca_key)?.der().to_vec();
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&key_path, ca_key.serialize_pem())?;
            std::fs::write(&der_path, &der)?;
            println!("[cert] new CA saved to {}", dir.display());
            der
        }
    };
    let ca_pem = {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&ca_der);
        let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
    };
    let issuer = rcgen::Issuer::new(ca.clone(), &ca_key);

    let leaf_key = KeyPair::generate()?;
    let mut leaf = CertificateParams::new(vec![])?;
    leaf.distinguished_name.push(DnType::CommonName, "PhoneCam spike server");
    leaf.subject_alt_names = ips.iter().map(|i| SanType::IpAddress(IpAddr::V4(*i))).collect();
    leaf.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.not_before = date_time_ymd(2026, 10, 28);
    leaf.not_after = date_time_ymd(2027, 11, 20); // ~388 days, under iOS 398 limit
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer)?;

    Ok(Certs {
        leaf_chain_pem: format!("{}{}", leaf_cert.pem(), ca_pem),
        ca_der,
        ca_pem,
        leaf_key_pem: leaf_key.serialize_pem(),
    })
}

pub fn mobileconfig(ca_der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(ca_der);
    format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>PayloadContent</key><array><dict>
<key>PayloadCertificateFileName</key><string>phonecam-ca.cer</string>
<key>PayloadContent</key><data>{b64}</data>
<key>PayloadDescription</key><string>PhoneCam local CA (name-constrained to private IPs)</string>
<key>PayloadDisplayName</key><string>PhoneCam Local CA</string>
<key>PayloadIdentifier</key><string>local.phonecam.spike.ca</string>
<key>PayloadType</key><string>com.apple.security.root</string>
<key>PayloadUUID</key><string>7B1C6C0E-5A55-4C8B-9E0B-2D4B5A1F0001</string>
<key>PayloadVersion</key><integer>1</integer>
</dict></array>
<key>PayloadDescription</key><string>Installs the PhoneCam spike CA</string>
<key>PayloadDisplayName</key><string>PhoneCam Local CA</string>
<key>PayloadIdentifier</key><string>local.phonecam.spike.profile</string>
<key>PayloadRemovalDisallowed</key><false/>
<key>PayloadType</key><string>Configuration</string>
<key>PayloadUUID</key><string>7B1C6C0E-5A55-4C8B-9E0B-2D4B5A1F0002</string>
<key>PayloadVersion</key><integer>1</integer>
</dict></plist>
"#)
}
