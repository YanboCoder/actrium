import Foundation

/// Digest-addressed package cache (05 §15):
///
///     <root>/packages/sha256/<digest>/package.actr
///     <root>/extracted/sha256/<digest>/...
///
/// Rules (05 §15):
/// - the original archive is kept for re-verification;
/// - extracted files are produced only from the archived package;
/// - writes are atomic (staging dir, then rename into place);
/// - unfinished work stays in staging paths, never at the final path.
///
/// The 6-day prototype scope (15-B B2) is the cache layout + digest dedupe;
/// eviction / cleanup policy is deferred.
enum ActrPackageCache {
    /// Cache root: ~/Library/Caches/<bundle-id>/ (system-managed, purgeable).
    static var root: URL {
        let base = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        let bundleID = Bundle.main.bundleIdentifier ?? "io.actrium.actro"
        return base.appendingPathComponent(bundleID, isDirectory: true)
    }

    // MARK: - Paths

    /// Digest-addressed path of the archived package.
    static func archiveURL(forDigest digest: String) -> URL {
        root
            .appendingPathComponent("packages/sha256/\(digest)", isDirectory: true)
            .appendingPathComponent("package.actr")
    }

    /// Digest-addressed extraction directory.
    static func extractedURL(forDigest digest: String) -> URL {
        root.appendingPathComponent("extracted/sha256/\(digest)", isDirectory: true)
    }

    // MARK: - Archive install

    /// Copies an archive into its digest-addressed location; reuses an existing
    /// copy (same digest implies identical content). Returns the cached path.
    static func installArchive(at sourceArchiveURL: URL, digest: String) throws -> URL {
        let fm = FileManager.default
        let destination = Self.archiveURL(forDigest: digest)
        if fm.fileExists(atPath: destination.path) { return destination }

        let destinationDir = destination.deletingLastPathComponent()
        try fm.createDirectory(at: destinationDir, withIntermediateDirectories: true)
        let staging = destinationDir
            .appendingPathComponent(".staging-\(UUID().uuidString)", isDirectory: false)
        defer { try? fm.removeItem(at: staging) }

        do {
            try fm.copyItem(at: sourceArchiveURL, to: staging)
            try fm.moveItem(at: staging, to: destination)
        } catch {
            // Another writer landed the same digest first — reuse it.
            guard fm.fileExists(atPath: destination.path) else { throw error }
        }
        return destination
    }

    // MARK: - Extraction

    /// Extracts a cached archive into its digest-addressed directory; reuses an
    /// existing extraction (same digest = same content, no re-extract on reopen).
    ///
    /// `validate` runs against the staging directory *before* it is moved to
    /// the digest-addressed path, so unverified content never becomes the
    /// cached extraction (05 §15: extracted files only come from verified
    /// packages). A hit on an existing extraction is re-validated as well,
    /// since the cache directory is not a trust boundary on its own.
    static func extractArchiveIfNeeded(
        _ archiveURL: URL,
        digest: String,
        validate: ((URL) throws -> Void)? = nil
    ) throws -> URL {
        let fm = FileManager.default
        let destination = extractedURL(forDigest: digest)
        if fm.fileExists(atPath: destination.path) {
            try validate?(destination)
            return destination
        }

        let parent = destination.deletingLastPathComponent()
        try fm.createDirectory(at: parent, withIntermediateDirectories: true)
        let staging = parent
            .appendingPathComponent(".staging-\(UUID().uuidString)", isDirectory: true)
        do {
            try fm.createDirectory(at: staging, withIntermediateDirectories: true)
            try unzipArchive(archiveURL, to: staging)
            try validate?(staging)
            do {
                try fm.moveItem(at: staging, to: destination)
            } catch {
                guard fm.fileExists(atPath: destination.path) else { throw error }
                try? fm.removeItem(at: staging)
            }
        } catch {
            try? fm.removeItem(at: staging)
            throw error
        }
        return destination
    }

    // MARK: - Direct-link URL memo (B2: no re-download for a known link)

    /// Returns the digest previously downloaded for a direct link, when the
    /// cached archive for it still exists. Falls through to download otherwise.
    static func cachedDigest(forDownloadURL url: URL) -> String? {
        guard let record = urlMap()[url.absoluteString], let digest = record["digest"],
              FileManager.default.fileExists(atPath: archiveURL(forDigest: digest).path)
        else { return nil }
        return digest
    }

    /// Remembers which digest a direct link resolved to.
    static func rememberDownloadURL(_ url: URL, digest: String) {
        var map = urlMap()
        map[url.absoluteString] = [
            "digest": digest,
            "installedAtUnix": String(Date().timeIntervalSince1970),
        ]
        saveURLMap(map)
    }

    private static func urlMapFile() -> URL {
        root.appendingPathComponent("download-url-map.json")
    }

    private static func urlMap() -> [String: [String: String]] {
        guard let data = try? Data(contentsOf: urlMapFile()),
              let map = try? JSONSerialization.jsonObject(with: data) as? [String: [String: String]]
        else { return [:] }
        return map
    }

    private static func saveURLMap(_ map: [String: [String: String]]) {
        let file = urlMapFile()
        try? FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard let data = try? JSONSerialization.data(withJSONObject: map) else { return }
        try? data.write(to: file, options: .atomic)
    }

    // MARK: - Unzip

    /// Unzips an .actr archive with /usr/bin/ditto (reliable macOS unzipping).
    static func unzipArchive(_ archive: URL, to destination: URL) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-xk", archive.path, destination.path]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        try process.run()
        process.waitUntilExit()

        if process.terminationStatus != 0 {
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let output = String(data: data, encoding: .utf8) ?? ""
            throw NSError(
                domain: "ACTRUnzipError",
                code: Int(process.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: "Unzip failed: \(output)"]
            )
        }
    }
}
