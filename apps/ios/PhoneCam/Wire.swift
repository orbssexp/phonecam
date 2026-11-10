import Foundation
import CryptoKit

/// Wire protocol "native v1": framing, hex helpers, handshake proofs.
enum Wire {
    static let maxFrame = 8 * 1024 * 1024
    static let typeControl: UInt8 = 0x01
    static let typeVideo: UInt8 = 0x03

    /// u32 BE len (type + payload) | u8 type | payload
    static func frame(type: UInt8, payload: Data) -> Data {
        var d = Data(capacity: 5 + payload.count)
        let len = UInt32(payload.count + 1)
        d.append(UInt8(len >> 24 & 0xFF)); d.append(UInt8(len >> 16 & 0xFF))
        d.append(UInt8(len >> 8 & 0xFF)); d.append(UInt8(len & 0xFF))
        d.append(type)
        d.append(payload)
        return d
    }

    /// Video payload: u64 BE pts_us | u8 flags | u8 rot | Annex-B
    static func videoPayload(ptsUs: UInt64, keyframe: Bool, rot: UInt8, annexB: Data) -> Data {
        var d = Data(capacity: 10 + annexB.count)
        for i in (0..<8).reversed() { d.append(UInt8((ptsUs >> UInt64(i * 8)) & 0xFF)) }
        d.append(keyframe ? 1 : 0)
        d.append(rot & 3)
        d.append(annexB)
        return d
    }

    static func hex(_ d: Data) -> String { d.map { String(format: "%02x", $0) }.joined() }

    static func fromHex(_ s: String) -> Data? {
        let c = Array(s.utf8)
        guard c.count % 2 == 0 else { return nil }
        var out = Data(capacity: c.count / 2)
        func nib(_ b: UInt8) -> UInt8? {
            switch b {
            case 48...57: return b - 48
            case 97...102: return b - 87
            case 65...70: return b - 55
            default: return nil
            }
        }
        var i = 0
        while i < c.count {
            guard let h = nib(c[i]), let l = nib(c[i + 1]) else { return nil }
            out.append(h << 4 | l)
            i += 2
        }
        return out
    }

    static func randomNonceHex() -> String {
        var b = [UInt8](repeating: 0, count: 16)
        for i in 0..<16 { b[i] = UInt8.random(in: 0...255) }
        return hex(Data(b))
    }

    static func hmacHex(key: Data, message: String) -> String {
        let mac = HMAC<SHA256>.authenticationCode(for: Data(message.utf8), using: SymmetricKey(data: key))
        return hex(Data(mac))
    }

    static func phoneProof(key: Data, pcNonce: String, phoneNonce: String) -> String {
        hmacHex(key: key, message: "phonecam-native-v1|phone|" + pcNonce + phoneNonce)
    }

    static func desktopProof(key: Data, pcNonce: String, phoneNonce: String) -> String {
        hmacHex(key: key, message: "phonecam-native-v1|desktop|" + phoneNonce + pcNonce)
    }

    /// Constant-time comparison of two hex strings.
    static func safeEqual(_ a: String, _ b: String) -> Bool {
        let x = Array(a.utf8), y = Array(b.utf8)
        guard x.count == y.count else { return false }
        var r: UInt8 = 0
        for i in 0..<x.count { r |= x[i] ^ y[i] }
        return r == 0
    }

    /// First 16 hex chars of SHA-256("phonecam-id-v1" || k).
    static func serviceId(key: Data) -> String {
        var d = Data("phonecam-id-v1".utf8)
        d.append(key)
        return String(hex(Data(SHA256.hash(data: d))).prefix(16))
    }
}

/// Handshake messages use the key "t".
struct HandshakeMsg: Codable {
    var t: String
    var v: Int?
    var nonce: String?
    var proof: String?
}

struct LensInfo: Codable {
    var id: String
    var label: String
}

/// Control message; same JSON shapes as phonecam_core::protocol::Ctl (tag "type", snake_case).
/// Unset optionals are omitted from the output.
struct Ctl: Codable {
    var type: String
    var t: UInt64?
    var battery: Float?
    var charging: Bool?
    var profile: String?
    var zoom: Float?
    var torch: Bool?
    var lens: String?
    var reason: String?
    var lenses: [LensInfo]?
    var current: String?
    var zoom_min: Float?
    var zoom_max: Float?
    var rotation: UInt16?
    var width: UInt32?
    var height: UInt32?
    var fps: Float?

    init(type: String) { self.type = type }
}
