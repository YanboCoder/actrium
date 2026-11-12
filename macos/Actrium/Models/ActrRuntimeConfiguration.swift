import Foundation

/// Host-generated `actr.toml` for package-backed instances.
///
/// `.actr` packages carry the workload, not runtime configuration. The Host
/// therefore writes one config file per device and hands its path to
/// `ActrNode.from(packageConfig:packagePath:)`.
///
/// The values default to a local developer stack and can be overridden with:
///   - `ACTRIUM_SIGNALING_URL`
///   - `ACTRIUM_ISSUER_ENDPOINT`
///   - `ACTRIUM_REALM`
enum ActrRuntimeConfiguration {
    static var signalingURL: String {
        environment("ACTRIUM_SIGNALING_URL")
            ?? "ws://127.0.0.1:8081/signaling/ws"
    }

    static var issuerEndpoint: String {
        environment("ACTRIUM_ISSUER_ENDPOINT")
            ?? "http://127.0.0.1:8081/issuer"
    }

    static var realm: String {
        environment("ACTRIUM_REALM") ?? "local@actr.alt"
    }

    /// Writes the runtime config into App Support and returns its file URL.
    /// Rewriting on every start keeps environment overrides authoritative.
    static func configFileURL() throws -> URL {
        let support = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.temporaryDirectory
        let directory = support
            .appendingPathComponent("Actrium", isDirectory: true)
            .appendingPathComponent("runtime", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let url = directory.appendingPathComponent("actr.toml", isDirectory: false)
        try toml().write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    static func toml() -> String {
        """
        edition = 1

        [signaling]
        url = "\(signalingURL)"

        [issuer_endpoint]
        url = "\(issuerEndpoint)"

        [deployment]
        realm = "\(realm)"

        [discovery]
        visible = true

        [observability]
        filter_level = "info"
        tracing_enabled = false

        [webrtc]
        force_relay = false
        """
    }

    private static func environment(_ key: String) -> String? {
        guard let value = ProcessInfo.processInfo.environment[key],
              !value.isEmpty
        else { return nil }
        return value
    }
}
