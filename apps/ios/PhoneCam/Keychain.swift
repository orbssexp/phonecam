import Foundation
import Security

/// Stores the 32-byte pairing secret. Never log it.
enum Keychain {
    private static let service = "com.orbssexp.phonecam"
    private static let account = "pairing-k"

    private static var base: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account]
    }

    static func load() -> Data? {
        var q = base
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let d = out as? Data, d.count == 32 else { return nil }
        return d
    }

    @discardableResult
    static func save(_ key: Data) -> Bool {
        SecItemDelete(base as CFDictionary)
        var q = base
        q[kSecValueData as String] = key
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
    }
}
