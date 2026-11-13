import Foundation
import ActriumCore

/// Supplies this installation's local Realm id (`installation_id`) — the
/// single identity for both local-realm instance addresses
/// (`actr://<installation-id>.actr.alt/<realm-id>`) and the parser's
/// local-Realm ownership check.
///
/// Generation and validation rules live in the Rust core
/// (`ActriumCore.generateInstallationId` / `validateInstallationId`), so
/// every platform shares one implementation. This provider only owns the
/// platform-specific persistence: read the stored value, re-validate it,
/// generate and persist on first use.
///
/// Persistence is UserDefaults as a working minimum. Losing the id makes
/// every local Realm identity unrecoverable, so migrate to Keychain-backed
/// storage before the local-instance flow ships.
enum InstallationIdProvider {
    static let defaultsKey = "actr.installationId"

    /// In-memory memo: the id is immutable for the process lifetime.
    private static var cached: String?

    /// Return the stable installation id, generating and persisting it on
    /// first use. Every later call returns the same value.
    static func getOrCreate() -> String {
        if let cached {
            return cached
        }
        let id = computeOrCreate()
        cached = id
        return id
    }

    private static func computeOrCreate() -> String {
        let defaults = UserDefaults.standard
        // One-time migration: the shell previously generated its own id in
        // App Support (the removed ActrInstallationIdentity); existing local
        // instance addresses embed that value, so it wins over any
        // UserDefaults value when present.
        if let legacy = legacyAppSupportValue() {
            if defaults.string(forKey: defaultsKey) != legacy {
                defaults.set(legacy, forKey: defaultsKey)
            }
            return legacy
        }
        if let existing = defaults.string(forKey: defaultsKey) {
            do {
                try ActriumCore.validateInstallationId(id: existing)
                return existing
            } catch {
                // Corrupt or legacy value — fall through and regenerate.
            }
        }

        let id = ActriumCore.generateInstallationId()
        defaults.set(id, forKey: defaultsKey)
        return id
    }

    /// Legacy App Support value from the pre-unification shell identity.
    private static func legacyAppSupportValue() -> String? {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        let file = support
            .appendingPathComponent("Actrium", isDirectory: true)
            .appendingPathComponent("installation-id", isDirectory: false)
        guard let raw = try? String(contentsOf: file, encoding: .utf8),
              isValid(raw.trimmingCharacters(in: .whitespacesAndNewlines))
        else { return nil }
        return raw.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Format check (26 chars, base32 a-z/2-7) mirroring validateInstallationId.
    static func isValid(_ id: String) -> Bool {
        let alphabet = Array("abcdefghijklmnopqrstuvwxyz234567")
        return id.count == 26 && id.allSatisfy(alphabet.contains)
    }
}
