import Foundation
import CryptoKit

/// Size cap for prototype package downloads (05 §15; 64 MiB prototype cap).
enum ActrPackageDownloadLimits {
    nonisolated static let maxBytes: Int64 = 64 * 1024 * 1024
}

/// Policy deciding which URLs enter the package direct-link download flow.
enum ActrPackageLinkPolicy {
    /// True for package direct links: an https URL whose path ends with `.actr`.
    /// http is accepted only for loopback/localhost hosts (05 §14 dev sources;
    /// formal addresses never fall back to HTTP).
    static func isPackageDownloadURL(_ url: URL) -> Bool {
        guard let host = url.host, !host.isEmpty else { return false }
        guard url.path.lowercased().hasSuffix(".actr") else { return false }
        switch url.scheme?.lowercased() {
        case "https":
            return true
        case "http":
            return isLoopbackHost(host)
        default:
            return false
        }
    }

    /// Exact loopback match: `127.0.0.1` is matched literally so names like
    /// `127.0.0.1.evil.com` are not treated as loopback.
    static func isLoopbackHost(_ host: String) -> Bool {
        host == "localhost"
            || host == "::1"
            || host == "[::1]"
            || host == "127.0.0.1"
            || host == "[0:0:0:0:0:0:0:1]"
    }
}

/// Package link awaiting user confirmation — confirm before connect (04 §3).
struct PendingPackageDownload: Identifiable {
    let id = UUID()
    let url: URL

    var host: String { url.host ?? "" }
    var filename: String { url.lastPathComponent }

    var sizeLimitText: String {
        ByteCountFormatter.string(fromByteCount: ActrPackageDownloadLimits.maxBytes, countStyle: .file)
    }
}

/// State of an in-flight (or failed) package download, shown in the progress sheet.
struct ActivePackageDownload: Identifiable {
    let id = UUID()
    let pending: PendingPackageDownload

    var phase: Phase = .downloading
    var errorMessage: String?

    enum Phase {
        case downloading
        case verifying
        case failed
    }
}

/// Notice shown when the user enters a formal `actr://` address.
/// Realm Profile resolution (04 §3 open flow) is not wired yet in the prototype,
/// so the address cannot be opened; the notice tells the user what to do instead.
struct ActrUriNotice: Identifiable {
    let id = UUID()
    let uri: String
    let domain: String
    /// Optional failure detail (e.g. local instance not found); when nil the
    /// sheet explains the pending formal Realm Profile resolution.
    var detail: String? = nil
}

/// Pending invite-code redemption (temporary flow): an actr:// link entered in
/// the address bar waits for an invite code first; on "Redeem" the
/// Rust-provided method takes over the follow-up flow.
struct PendingInviteRedeem: Identifiable {
    let id = UUID()
    let uri: String
    let domain: String
}

enum ActrPackageDownloadError: LocalizedError {
    case sizeExceeded(maxBytes: Int64)
    case cancelled
    case failed(reason: String)

    var errorDescription: String? {
        switch self {
        case .sizeExceeded(let maxBytes):
            let limit = ByteCountFormatter.string(fromByteCount: maxBytes, countStyle: .file)
            return "Package exceeds the size limit \(limit); download stopped"
        case .cancelled:
            return nil
        case .failed(let reason):
            return "Download failed: \(reason)"
        }
    }
}

enum ActrPackageDownloadService {
    struct Result {
        let localURL: URL
        /// 64-char lowercase hex SHA-256 of the downloaded archive.
        let digest: String
    }

    /// Streams the archive to a stable temporary file, aborting as soon as the
    /// size cap is crossed (05 §15: unfinished downloads stay isolated). The
    /// transfer is also rejected when a redirect lands on a different host or
    /// downgrades the scheme, so the host confirmed in the sheet stays the
    /// trust root (04 §3 "confirm before connect").
    nonisolated static func download(from url: URL) async throws -> URL {
        let bytes: URLSession.AsyncBytes
        let response: URLResponse
        do {
            (bytes, response) = try await URLSession.shared.bytes(from: url)
        } catch let error as URLError where error.code == .cancelled {
            throw ActrPackageDownloadError.cancelled
        } catch is CancellationError {
            throw ActrPackageDownloadError.cancelled
        } catch {
            throw ActrPackageDownloadError.failed(reason: error.localizedDescription)
        }

        if let http = response as? HTTPURLResponse {
            guard (200..<300).contains(http.statusCode) else {
                throw ActrPackageDownloadError.failed(reason: "HTTP \(http.statusCode)")
            }
        }

        guard let finalURL = response.url,
              finalURL.host?.lowercased() == url.host?.lowercased(),
              finalURL.scheme?.lowercased() == url.scheme?.lowercased()
        else {
            throw ActrPackageDownloadError.failed(reason: "Redirect target does not match the confirmed address")
        }

        // Pre-check via declared Content-Length when the server provides it.
        if response.expectedContentLength > ActrPackageDownloadLimits.maxBytes {
            throw ActrPackageDownloadError.sizeExceeded(maxBytes: ActrPackageDownloadLimits.maxBytes)
        }

        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent("actr-pkg-\(UUID().uuidString).actr")
        _ = FileManager.default.createFile(atPath: destination.path, contents: nil)
        guard let handle = FileHandle(forWritingAtPath: destination.path) else {
            throw ActrPackageDownloadError.failed(reason: "Could not create the download temporary file")
        }

        var written: Int64 = 0
        var buffer = Data()
        buffer.reserveCapacity(1 << 20)

        func flush() throws {
            guard !buffer.isEmpty else { return }
            written += Int64(buffer.count)
            guard written <= ActrPackageDownloadLimits.maxBytes else {
                throw ActrPackageDownloadError.sizeExceeded(maxBytes: ActrPackageDownloadLimits.maxBytes)
            }
            try handle.write(contentsOf: buffer)
            buffer.removeAll(keepingCapacity: true)
        }

        do {
            for try await byte in bytes {
                buffer.append(byte)
                if buffer.count >= (1 << 20) {
                    try flush()
                }
            }
            try flush()
            try handle.close()
        } catch let error as ActrPackageDownloadError {
            try? handle.close()
            try? FileManager.default.removeItem(at: destination)
            throw error
        } catch let error as URLError where error.code == .cancelled {
            try? handle.close()
            try? FileManager.default.removeItem(at: destination)
            throw ActrPackageDownloadError.cancelled
        } catch is CancellationError {
            try? handle.close()
            try? FileManager.default.removeItem(at: destination)
            throw ActrPackageDownloadError.cancelled
        } catch {
            try? handle.close()
            try? FileManager.default.removeItem(at: destination)
            throw ActrPackageDownloadError.failed(reason: error.localizedDescription)
        }

        return destination
    }

    /// Computes the SHA-256 digest of a file off the main actor.
    nonisolated static func sha256Digest(of url: URL) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    continuation.resume(returning: try sha256File(at: url))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Synchronous SHA-256 of a file (call off the main actor for large files).
    nonisolated static func sha256FileSync(at url: URL) throws -> String {
        try sha256File(at: url)
    }

    nonisolated private static func sha256File(at url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while let chunk = try handle.read(upToCount: 1024 * 1024), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}
