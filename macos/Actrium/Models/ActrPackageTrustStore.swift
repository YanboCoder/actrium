import Foundation
import CryptoKit

/// Prototype trusted public keys for .actr signature verification (15-B B3).
///
/// Packages do not carry the signing public key — manifest.toml only records a
/// `signature.key-id` fingerprint. Trust is established out-of-band; during the
/// prototype the host collects keys from:
///
/// 1. `~/Library/Application Support/Actrium/trusted-keys.json`
///    `[{"public_key": "<base64>", "key_id"?: "actr-pub-…"}]`
/// 2. `~/.actr/dev-key.json` — the `actr pkg keygen` default keychain
///    (05 §14 local development signing key), field `public_key` (base64).
///
/// Long-term target: unified verifyActrPackage FFI on actr-ffi + Publisher Registry
/// (15 号计划 IF-1).
enum ActrPackageTrustStore {
    struct TrustedKey {
        /// Raw 32-byte Ed25519 public key.
        let publicKey: Data
        /// `actr-pub-<sha256(pubkey)[..16]>` (actr-pack lib.rs), for diagnostics.
        let keyId: String
        let origin: String
    }

    static func trustedKeys() -> [TrustedKey] {
        var keys: [TrustedKey] = []
        keys.append(contentsOf: loadFromAppSupport())
        // ~/.actr/dev-key.json is writable by any local process, so trusting it
        // in release builds would let anything bypass signature verification.
        // Keep it for development only (05 §14 local development signing key).
        if allowsDevelopmentKeychain {
            keys.append(contentsOf: loadFromDevKeychain())
        }
        return keys
    }

    private static var allowsDevelopmentKeychain: Bool {
        #if DEBUG
        return true
        #else
        return ProcessInfo.processInfo.environment["ACTR_ALLOW_DEV_KEY"] == "1"
        #endif
    }

    // MARK: - Key id (aligned with actr_pack::compute_key_id)

    static func keyId(for publicKey: Data) -> String {
        // Mirror actr_pack::compute_key_id: "actr-pub-" + the first 16 hex
        // characters of sha256(public_key), i.e. 8 digest bytes (not 16).
        let hex = SHA256.hash(data: publicKey).map { String(format: "%02x", $0) }.joined()
        let prefix = hex.prefix(16)
        return "actr-pub-\(prefix)"
    }

    // MARK: - Sources

    /// `~/Library/Application Support/Actrium/trusted-keys.json`
    private static func loadFromAppSupport() -> [TrustedKey] {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        guard let support else { return [] }
        let file = support
            .appendingPathComponent("Actrium", isDirectory: true)
            .appendingPathComponent("trusted-keys.json", isDirectory: false)
        guard let data = try? Data(contentsOf: file),
              let entries = try? JSONSerialization.jsonObject(with: data) as? [[String: String]]
        else { return [] }
        return entries.compactMap { entry in
            guard let base64 = entry["public_key"],
                  let keyData = Data(base64Encoded: base64),
                  keyData.count == 32
            else { return nil }
            return TrustedKey(
                publicKey: keyData,
                keyId: entry["key_id"] ?? keyId(for: keyData),
                origin: file.lastPathComponent
            )
        }
    }

    /// `~/.actr/dev-key.json` — the `actr pkg keygen` default keychain.
    private static func loadFromDevKeychain() -> [TrustedKey] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let file = home
            .appendingPathComponent(".actr", isDirectory: true)
            .appendingPathComponent("dev-key.json", isDirectory: false)
        guard let data = try? Data(contentsOf: file),
              let entry = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let base64 = entry["public_key"] as? String,
              let keyData = Data(base64Encoded: base64),
              keyData.count == 32
        else { return [] }
        return [
            TrustedKey(
                publicKey: keyData,
                keyId: keyId(for: keyData),
                origin: file.lastPathComponent
            )
        ]
    }
}
