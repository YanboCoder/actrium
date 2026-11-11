import ActrBindings
import CryptoKit
import Foundation
import Security

/// One identity per installation and Realm. Software Ed25519 is stored in
/// ThisDeviceOnly Keychain items; Secure Enclave's P-256 keys are not substituted
/// for the protocol's Ed25519 keys. Neither keys nor installation markers migrate.
public final class RealmIdentityStore: @unchecked Sendable {
    private static let lock = NSLock()
    private let directory: URL
    private let service: String

    public init() throws {
        let support = try FileManager.default.url(for: .applicationSupportDirectory,
            in: .userDomainMask, appropriateFor: nil, create: true)
        directory = support.appendingPathComponent("ActrRealmIdentities", isDirectory: true)
        service = (Bundle.main.bundleIdentifier ?? "io.actrium.sdk") + ".realm-client"
    }

    // Tests use isolated installation sandboxes and Keychain namespaces.
    internal init(directory: URL, service: String) {
        self.directory = directory
        self.service = service
    }

    private func locked<T>(_ body: (String) throws -> T) throws -> T {
        Self.lock.lock()
        defer { Self.lock.unlock() }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var directory = directory
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try directory.setResourceValues(values)
        let marker = directory.appendingPathComponent("installation")
        let installation: String
        if FileManager.default.fileExists(atPath: marker.path) {
            let stored = try String(contentsOf: marker, encoding: .utf8)
            guard let id = UUID(uuidString: stored) else { throw RealmMembershipError.KeyUnavailable }
            installation = id.uuidString
        } else {
            installation = UUID().uuidString
            try Data(installation.utf8).write(to: marker, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
            var marker = marker
            try marker.setResourceValues(values)
        }
        return try body(installation)
    }

    private func record(_ realm: String) -> URL {
        let digest = SHA256.hash(data: Data(realm.utf8)).map { String(format: "%02x", $0) }.joined()
        return directory.appendingPathComponent(digest + ".identity")
    }

    private func query(_ installation: String, _ realm: String) -> [String: Any] {
        let digest = SHA256.hash(data: Data(realm.utf8)).map { String(format: "%02x", $0) }.joined()
        return [kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: installation + "." + digest,
            kSecAttrSynchronizable as String: false]
    }

    private func read(_ installation: String, _ realm: String) throws -> Data? {
        var request = query(installation, realm)
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data, data.count == 32 else {
            throw RealmMembershipError.KeyUnavailable
        }
        return data
    }

    /// Persist before exchanging an invite. Errors never replace an existing key.
    public func membership(
        realm: String, providerId: String, issuer: String,
        allowInsecureDevelopment: Bool = false
    ) throws -> ActrBindings.RealmMembership {
        let publicKey = try locked { installation -> Data in
            if try read(installation, realm) == nil {
                guard !FileManager.default.fileExists(atPath: record(realm).path) else {
                    throw RealmMembershipError.KeyUnavailable
                }
                let key = Curve25519.Signing.PrivateKey()
                var attributes = query(installation, realm)
                attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
                attributes[kSecValueData as String] = key.rawRepresentation
                let status = SecItemAdd(attributes as CFDictionary, nil)
                guard status == errSecSuccess || status == errSecDuplicateItem else {
                    throw RealmMembershipError.KeyUnavailable
                }
            }
            guard var seed = try read(installation, realm) else { throw RealmMembershipError.KeyUnavailable }
            defer { seed.resetBytes(in: seed.startIndex..<seed.endIndex) }
            let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed).publicKey.rawRepresentation
            if FileManager.default.fileExists(atPath: record(realm).path) {
                guard try Data(contentsOf: record(realm)) == key else { throw RealmMembershipError.KeyUnavailable }
            } else {
                try key.write(to: record(realm), options: .atomic)
            }
            return key
        }
        return try ActrBindings.RealmMembership(realm: realm, providerId: providerId, issuer: issuer,
            signer: Signer(store: self, realm: realm, key: publicKey),
            allowInsecureDevelopment: allowInsecureDevelopment)
    }

    /// Explicit rejoin after stopping Hosts. The server's old client must still
    /// be revoked by its owner; resetting local storage is not server revocation.
    public func forgetForRejoin(realm: String) throws {
        try locked { installation in
            let status = SecItemDelete(query(installation, realm) as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound else {
                throw RealmMembershipError.KeyUnavailable
            }
            if FileManager.default.fileExists(atPath: record(realm).path) {
                try FileManager.default.removeItem(at: record(realm))
            }
        }
    }

    private final class Signer: RealmClientSigner, @unchecked Sendable {
        let store: RealmIdentityStore
        let realm: String
        let key: Data
        init(store: RealmIdentityStore, realm: String, key: Data) {
            self.store = store; self.realm = realm; self.key = key
        }
        func publicKey() throws -> Data { key }
        func sign(payload: Data) throws -> Data {
            try store.locked { installation in
                guard var seed = try store.read(installation, realm) else { throw RealmMembershipError.KeyUnavailable }
                defer { seed.resetBytes(in: seed.startIndex..<seed.endIndex) }
                let signer = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
                guard signer.publicKey.rawRepresentation == key else { throw RealmMembershipError.KeyUnavailable }
                return try signer.signature(for: payload)
            }
        }
    }
}
