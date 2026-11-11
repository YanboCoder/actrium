import Foundation
import ActriumCore

/// Supplies this installation's local Realm id (`installation_id`).
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

    /// Return the stable installation id, generating and persisting it on
    /// first use. Every later call returns the same value.
    static func getOrCreate() -> String {
        let defaults = UserDefaults.standard
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
}
