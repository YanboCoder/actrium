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
/// (plan 15 IF-1).
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
        keys.append(contentsOf: loadVerifiedPublishers())
        // ~/.actr/dev-key.json is writable by any local process, so trusting it
        // in release builds would let anything bypass signature verification.
        // Keep it for development only (05 §14 local development signing key).
        if allowsDevelopmentKeychain {
            keys.append(contentsOf: loadFromDevKeychain())
        }
        return keys
    }

    // MARK: - Verified publisher keys (discovery funnel)

    /// `~/Library/Application Support/Actrium/verified-publishers.json` —
    /// publisher keys pinned per domain after the discovery funnel verified a
    /// package manifest signature with them (profile-authenticated trust).
    /// Restart restore re-verifies with these keys instead of requiring a
    /// manually installed key.
    private static var verifiedPublishersFile: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        return support
            .appendingPathComponent("Actrium", isDirectory: true)
            .appendingPathComponent("verified-publishers.json", isDirectory: false)
    }

    /// Records publisher keys that successfully verified a package manifest
    /// signature during discovery. Keys are pinned per publisher domain; a
    /// later import or restart restore verifies with the pinned keys even
    /// without a fresh discovery round.
    static func recordVerifiedPublisherKeys(domain: String, keys: [TrustedKey]) {
        guard !domain.isEmpty, !keys.isEmpty else { return }
        let file = verifiedPublishersFile
        var store: [String: [[String: String]]] = [:]
        if let data = try? Data(contentsOf: file),
           let loaded = try? JSONSerialization.jsonObject(with: data) as? [String: [[String: String]]] {
            store = loaded
        }
        var entries = store[domain] ?? []
        for key in keys {
            let base64 = key.publicKey.base64EncodedString()
            guard !entries.contains(where: { $0["public_key"] == base64 }) else { continue }
            entries.append(["public_key": base64, "key_id": key.keyId])
        }
        store[domain] = entries
        try? FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: store) {
            try? data.write(to: file, options: .atomic)
        }
    }

    private static func loadVerifiedPublishers() -> [TrustedKey] {
        guard let data = try? Data(contentsOf: verifiedPublishersFile),
              let store = try? JSONSerialization.jsonObject(with: data) as? [String: [[String: String]]]
        else { return [] }
        return store.values.flatMap { entries in
            entries.compactMap { entry in
                guard let base64 = entry["public_key"],
                      let keyData = Data(base64Encoded: base64),
                      keyData.count == 32
                else { return nil }
                return TrustedKey(
                    publicKey: keyData,
                    keyId: entry["key_id"] ?? keyId(for: keyData),
                    origin: "verified-publishers.json"
                )
            }
        }
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
