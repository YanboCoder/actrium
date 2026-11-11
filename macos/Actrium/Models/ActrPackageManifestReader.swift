import Foundation
import ActriumCore

private let actrManifestLogger = AppLogger.viewModel

struct ActrPackageManifestInfo {
    let name: String?
    let version: String?
    let description: String?
    let appId: String?
    let icon: String?
    let contractName: String?
    let contractVersion: Int?
    let permissions: [String]
    let leftPanelDefaultVisible: Bool
    let rightPanelDefaultVisible: Bool
    let leftPanelScript: String?
    let rightPanelScript: String?
    let unifiedScript: String?
    let systemAppType: SystemTabType?
    let themeColor: String?
}

enum ActrPackageManifestReader {
    /// - Parameter requireSignature: verify manifest.sig (Ed25519) before reading.
    ///   Set for package archives (15-B B3); local dev directories are explicitly
    ///   trusted and pass false.
    static func read(fromRoot root: URL, requireSignature: Bool = false) throws -> ActrPackageManifestInfo {
        if requireSignature {
            let trustedKeys = ActrPackageTrustStore.trustedKeys().map(\.publicKey)
            if !trustedKeys.isEmpty {
                actrManifestLogger.notice(
                    "[ActrPackageManifestReader] verifying signature with \(trustedKeys.count) trusted key(s)"
                )
            }
            do {
                try AppleActrPackageManifestReader.verifyManifestSignature(
                    fromRoot: root,
                    trustedKeys: trustedKeys
                )
            } catch let error as AppleActrPackageSignatureError {
                actrManifestLogger.error(
                    "[ActrPackageManifestReader] \(error.stableCode): \(error.localizedDescription)"
                )
                throw error
            }
        }
        let manifest = try AppleActrPackageManifestReader.read(fromRoot: root)
        logManifest(manifest)
        return ActrPackageManifestInfo(
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            appId: manifest.appId,
            icon: manifest.icon,
            contractName: manifest.contractName,
            contractVersion: manifest.contractVersion,
            permissions: manifest.permissions,
            leftPanelDefaultVisible: manifest.leftPanelDefaultVisible,
            rightPanelDefaultVisible: manifest.rightPanelDefaultVisible,
            leftPanelScript: manifest.leftPanelScript,
            rightPanelScript: manifest.rightPanelScript,
            unifiedScript: manifest.unifiedScript,
            systemAppType: manifest.systemAppType.flatMap(SystemTabType.init(rawValue:)),
            themeColor: manifest.themeColor
        )
    }

    private static func logManifest(_ manifest: AppleActrPackageManifestInfo) {
        if let color = manifest.themeColor {
            actrManifestLogger.notice("[ActrPackageManifestReader] theme color: \(color)")
        }
        if let contractName = manifest.contractName,
           let contractVersion = manifest.contractVersion {
            actrManifestLogger.notice("[ActrPackageManifestReader] workload contract: \(contractName)@v\(contractVersion)")
        }
        if !manifest.permissions.isEmpty {
            actrManifestLogger.debug("[ActrPackageManifestReader] capabilities: \(manifest.permissions.joined(separator: ", "))")
        }
        actrManifestLogger.notice("[ActrPackageManifestReader] payload integrity verified: \(manifest.integrityFileCount) files, \(manifest.integrityAlgorithm)")
    }
}
