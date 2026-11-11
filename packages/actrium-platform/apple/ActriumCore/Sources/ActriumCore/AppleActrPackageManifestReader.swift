import CryptoKit
import Foundation

public struct AppleActrPackageManifestInfo {
    public let name: String?
    public let version: String?
    public let description: String?
    public let appId: String?
    public let icon: String?
    public let contractName: String?
    public let contractVersion: Int?
    public let permissions: [String]
    public let leftPanelDefaultVisible: Bool
    public let rightPanelDefaultVisible: Bool
    public let leftPanelScript: String?
    public let rightPanelScript: String?
    public let unifiedScript: String?
    public let systemAppType: String?
    public let themeColor: String?
    public let integrityAlgorithm: String
    public let integrityFileCount: Int
}

/// Ed25519 signature verification failures (stable codes align with 04 §7).
public enum AppleActrPackageSignatureError: Error, LocalizedError {
    case signatureMissing
    case noTrustedKeys
    case signatureInvalid

    /// Stable error code for logging / tests / error pages (04 §7).
    public var stableCode: String {
        switch self {
        case .signatureMissing, .noTrustedKeys, .signatureInvalid:
            return "PACKAGE_SIGNATURE_INVALID"
        }
    }

    public var errorDescription: String? {
        switch self {
        case .signatureMissing:
            return "程序包缺少 manifest.sig 签名（PACKAGE_SIGNATURE_INVALID）"
        case .noTrustedKeys:
            return "没有可用的可信公钥，无法验证程序包签名（PACKAGE_SIGNATURE_INVALID）。"
                + "请运行 actr pkg keygen 或配置可信公钥后重试"
        case .signatureInvalid:
            return "程序包签名验证失败（PACKAGE_SIGNATURE_INVALID），已拒绝打开"
        }
    }
}

public enum AppleActrPackageManifestReader {
    public static func read(fromRoot root: URL) throws -> AppleActrPackageManifestInfo {
        guard let manifestURL = findManifest(in: root) else {
            throw packageError(code: 1, message: "manifest.toml was not found")
        }
        let source = try String(contentsOf: manifestURL, encoding: .utf8)
        let manifest = try parseActrPackageManifest(source: source)
        let plan = try createActrIntegrityPlan(manifestToml: source)
        try verifyPayloads(fromRoot: manifestURL.deletingLastPathComponent(), plan: plan)

        return AppleActrPackageManifestInfo(
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            appId: manifest.appId,
            icon: manifest.icon,
            contractName: manifest.contractName,
            contractVersion: manifest.contractVersion.map(Int.init),
            permissions: manifest.permissions,
            leftPanelDefaultVisible: manifest.leftPanelDefaultVisible,
            rightPanelDefaultVisible: manifest.rightPanelDefaultVisible,
            leftPanelScript: nil,
            rightPanelScript: nil,
            unifiedScript: manifest.entry,
            systemAppType: manifest.systemAppType,
            themeColor: manifest.themeColor,
            integrityAlgorithm: plan?.algorithm ?? "sha256",
            integrityFileCount: plan?.files.count ?? 0
        )
    }

    // MARK: - Signature verification (15-B B3)

    /// Verifies manifest.sig (64-byte Ed25519 over the raw manifest.toml bytes,
    /// actr-package/2) against a set of trusted raw public keys. Mirrors
    /// actr_pack::verify_actor_package: fail-closed on a signed package.
    ///
    /// Trusted key sourcing is platform-specific and stays with the host app;
    /// the long-term target is a unified verifyActrPackage FFI on actr-ffi
    /// (15 号计划 IF-1).
    ///
    /// - Parameter trustedKeys: raw 32-byte Ed25519 public keys; any match accepts.
    public static func verifyManifestSignature(fromRoot root: URL, trustedKeys: [Data]) throws {
        guard let manifestURL = findManifest(in: root) else {
            // read(fromRoot:) reports the missing manifest with code 1.
            return
        }
        let signatureURL = manifestURL.deletingLastPathComponent()
            .appendingPathComponent("manifest.sig", isDirectory: false)
        guard FileManager.default.fileExists(atPath: signatureURL.path) else {
            throw AppleActrPackageSignatureError.signatureMissing
        }
        let signature = try Data(contentsOf: signatureURL)
        guard signature.count == 64 else {
            throw AppleActrPackageSignatureError.signatureInvalid
        }
        guard !trustedKeys.isEmpty else {
            throw AppleActrPackageSignatureError.noTrustedKeys
        }

        let manifestBytes = try Data(contentsOf: manifestURL)
        for keyData in trustedKeys {
            guard keyData.count == 32,
                  let key = try? Curve25519.Signing.PublicKey(rawRepresentation: keyData)
            else { continue }
            if key.isValidSignature(signature, for: manifestBytes) {
                return
            }
        }
        throw AppleActrPackageSignatureError.signatureInvalid
    }

    private static func verifyPayloads(
        fromRoot root: URL,
        plan: PackageIntegrityPlanInfo?
    ) throws {
        guard let plan else {
            throw packageError(code: 2, message: "the package does not declare payload files")
        }
        for file in plan.files {
            let fileURL = try resolvedPayloadURL(root: root, path: file.path)
            let actualHash = try sha256Hex(for: fileURL)
            guard actualHash == file.expectedHash else {
                throw packageError(
                    code: 3,
                    message: "payload digest mismatch for \(file.path)"
                )
            }
        }
    }

    private static func resolvedPayloadURL(root: URL, path: String) throws -> URL {
        let rootURL = root.standardizedFileURL.resolvingSymlinksInPath()
        let fileURL = root
            .appendingPathComponent(path, isDirectory: false)
            .standardizedFileURL
            .resolvingSymlinksInPath()
        let rootPath = rootURL.path
        let filePath = fileURL.path
        guard filePath.hasPrefix(rootPath + "/") else {
            throw packageError(code: 4, message: "payload path escapes package: \(path)")
        }
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: filePath, isDirectory: &isDirectory),
              !isDirectory.boolValue else {
            throw packageError(code: 5, message: "payload file is missing: \(path)")
        }
        return fileURL
    }

    private static func sha256Hex(for fileURL: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: fileURL)
        defer { try? handle.close() }
        var hasher = SHA256()
        while true {
            let chunk = try handle.read(upToCount: 1024 * 1024) ?? Data()
            if chunk.isEmpty { break }
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    private static func findManifest(in root: URL) -> URL? {
        let direct = root.appendingPathComponent("manifest.toml", isDirectory: false)
        if FileManager.default.fileExists(atPath: direct.path) {
            return direct
        }
        if let enumerator = FileManager.default.enumerator(
            at: root,
            includingPropertiesForKeys: nil
        ) {
            for case let fileURL as URL in enumerator
            where fileURL.lastPathComponent == "manifest.toml" {
                return fileURL
            }
        }
        return nil
    }

    private static func packageError(code: Int, message: String) -> NSError {
        NSError(
            domain: "ActrPackage",
            code: code,
            userInfo: [NSLocalizedDescriptionKey: message]
        )
    }
}
