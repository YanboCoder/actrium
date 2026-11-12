import Foundation

private let logger = AppLogger.app

// MARK: - Tab Content Type

/// Native content types rendered by SwiftUI in the main process
enum NativeContentType: Equatable {
    case startPage
}

    /// Tab content type, which selects the rendering engine.
enum TabContentType: Equatable {
    /// React Native application.
    /// - bundleFingerprint: identical applications share a process.
    case reactNative(bundleFingerprint: String)

    /// Web content rendered by WKWebView.
    case web(url: URL)

    /// Native SwiftUI content (runs in main process, no XPC needed)
    case native(type: NativeContentType)

    /// Empty tab with no content.
    case empty
}

/// System tab types for built-in pages (like Chrome's chrome://history)
/// Uses about: URL scheme (e.g., about:history, about:bookmarks)
enum SystemTabType: String, Codable {
    case history
    case bookmarks
    case debug
    case startPage

    var title: String {
        switch self {
        case .history: return "History"
        case .bookmarks: return "Bookmarks"
        case .debug: return "Debug"
        case .startPage: return "New Tab"
        }
    }

    var icon: String {
        switch self {
        case .history: return "clock.arrow.circlepath"
        case .bookmarks: return "bookmark"
        case .debug: return "ladybug"
        case .startPage: return "house"
        }
    }

    /// The about: URL for this system tab type
    /// e.g., about:history, about:bookmarks, about:blank
    var aboutURL: URL {
        let path: String
        switch self {
        case .history: path = "history"
        case .bookmarks: path = "bookmarks"
        case .debug: path = "debug"
        case .startPage: path = "blank"
        }
        return URL(string: "about:\(path)")!
    }

    /// Parse a URL to get the corresponding SystemTabType
    /// Returns nil if the URL is not a valid about: URL for system apps
    static func from(url: URL) -> SystemTabType? {
        guard url.scheme == "about" else { return nil }
        // about: URLs have the path in the "path" component (e.g., about:history -> path = "history")
        // Note: URL(string: "about:history")?.path returns "history"
        let path = url.path.isEmpty ? url.absoluteString.replacingOccurrences(of: "about:", with: "") : url.path
        switch path {
        case "history": return .history
        case "bookmarks": return .bookmarks
        case "debug": return .debug
        case "blank": return .startPage
        default: return nil
        }
    }
}

struct Tab: Identifiable {
    let id = UUID()
    var title: String
    var url: URL?
    var appId: String?
    /// Per-tab Service actor endpoint supplied after Attach.
    var serviceCallUrl: String?
    /// Host-designated permission to use the legacy ACTR_CALL bridge.
    var legacyActorBridge: Bool = false
    var htmlContent: String?

    /// System tab type (nil for regular tabs, set for built-in pages like History/Bookmarks)
    var systemTabType: SystemTabType?

    /// Tab group ID (nil if not in a group)
    var groupId: UUID?

    /// Guest application script (deprecated; use the panel scripts instead).
    var guestScript: String?

    /// Left panel script (menu.js) for the legacy dual-engine architecture.
    var leftPanelScript: String?

    /// Right extension panel script (ext.js) for the legacy dual-engine architecture.
    var rightPanelScript: String?

    /// v2.0 unified script (unified-app.js) for the single-engine architecture.
    var unifiedScript: String?

    /// Digest-addressed extraction directory holding the verified guest bytes
    /// (15-B B4). The renderer prefers this over the inline `unifiedScript`.
    var guestExtractedPath: String?
    /// Manifest-declared entry file, resolved relative to `guestExtractedPath`.
    var guestEntry: String?

    /// Raw capability manifest from the verified package payload.
    var capabilityManifestJSON: String?

    /// Phase 1 contracts used for host prompts and version compatibility.
    var contractName: String?
    var contractVersion: Int?

    /// Phase 1 permissions used for host prompts and future gating.
    var permissions: [String]

    /// Reload counter passed to React Native as a remount key.
    var reloadNonce: Int

    /// Whether the left panel is visible by default.
    var leftPanelDefaultVisible: Bool

    /// Whether the right panel is visible by default.
    var rightPanelDefaultVisible: Bool

    /// Background color for this tab (Hex format: "#rrggbb")
    var backgroundColor: String

    /// Custom app icon (SF Symbol name) - can be set by RCTNApp
    var appIcon: String?

    init(
        title: String,
        url: URL? = nil,
        appId: String? = nil,
        serviceCallUrl: String? = nil,
        legacyActorBridge: Bool = false,
        htmlContent: String? = nil,
        systemTabType: SystemTabType? = nil,
        groupId: UUID? = nil,
        guestScript: String? = nil,
        leftPanelScript: String? = nil,
        rightPanelScript: String? = nil,
        unifiedScript: String? = nil,
        guestExtractedPath: String? = nil,
        guestEntry: String? = nil,
        capabilityManifestJSON: String? = nil,
        contractName: String? = nil,
        contractVersion: Int? = nil,
        permissions: [String] = [],
        reloadNonce: Int = 0,
        leftPanelDefaultVisible: Bool = true,
        rightPanelDefaultVisible: Bool = false,
        backgroundColor: String? = nil,
        appIcon: String? = nil
    ) {
        self.title = title
        self.url = url
        self.appId = appId
        self.serviceCallUrl = serviceCallUrl
        self.legacyActorBridge = legacyActorBridge
        self.htmlContent = htmlContent
        self.systemTabType = systemTabType
        self.groupId = groupId
        self.guestScript = guestScript
        self.leftPanelScript = leftPanelScript
        self.rightPanelScript = rightPanelScript
        self.unifiedScript = unifiedScript
        self.guestExtractedPath = guestExtractedPath
        self.guestEntry = guestEntry
        self.capabilityManifestJSON = capabilityManifestJSON
        self.contractName = contractName
        self.contractVersion = contractVersion
        self.permissions = permissions
        self.reloadNonce = reloadNonce
        self.leftPanelDefaultVisible = leftPanelDefaultVisible
        self.rightPanelDefaultVisible = rightPanelDefaultVisible
        self.backgroundColor = backgroundColor ?? "#1e1e1e"
        self.appIcon = appIcon
    }

    // MARK: - Content Type Detection

    /// Determines tab content type.
    /// Priority: Web URL, React Native app, native system page, then empty.
    var contentType: TabContentType {
        let result: TabContentType

        // 1. Check for an HTTP(S) Web URL.
        if let url = url, url.scheme == "http" || url.scheme == "https" {
            result = .web(url: url)
        }
        // 2. Check for a React Native application with script content.
        else if let fingerprint = bundleFingerprint {
            result = .reactNative(bundleFingerprint: fingerprint)
        }
        // 3. startPage is native SwiftUI; other system pages remain React Native.
        else if let systemType = systemTabType {
            switch systemType {
            case .startPage:
                result = .native(type: .startPage)
            case .history, .bookmarks, .debug:
                result = .reactNative(bundleFingerprint: "system.\(systemType.rawValue)")
            }
        }
        // 4. Empty tab.
        else {
            result = .empty
        }

        return result
    }

    /// Bundle fingerprint identifying a React Native application for XPC reuse.
    var bundleFingerprint: String? {
        // Prefer the unifiedScript hash as the fingerprint.
        if let script = unifiedScript {
            return "script.\(script.stableHash)"
        }

        // Legacy fallback: use leftPanelScript or guestScript.
        if let script = leftPanelScript ?? guestScript {
            return "legacy.\(script.stableHash)"
        }

        // contractName identifies Host/Guest compatibility, not the application.
        // Using it as a process fingerprint can co-locate unrelated applications
        // and mismatch Overview/tab content with titles.
        if let name = contractName {
            return "contract.\(name)"
        }

        return nil
    }

    /// Generates a random dark hexadecimal color.
    static func generateRandomColor() -> String {
        let hue = Double.random(in: 0...360)
        let saturation = Double.random(in: 0.5...0.7)  // 50-70%
        let lightness = Double.random(in: 0.15...0.2)  // 15-20%

        // Convert HSL to RGB.
        let rgb = hslToRGB(h: hue, s: saturation, l: lightness)

        // Convert RGB to hexadecimal.
        let r = Int(rgb.r * 255)
        let g = Int(rgb.g * 255)
        let b = Int(rgb.b * 255)
        let hexColor = String(format: "#%02x%02x%02x", r, g, b)

        logger.debug("Generated random background color: \(hexColor) from HSL(\(Int(hue)), \(Int(saturation * 100))%, \(Int(lightness * 100))%)")
        return hexColor
    }

    /// HSL-to-RGB helper.
    private static func hslToRGB(h: Double, s: Double, l: Double) -> (r: Double, g: Double, b: Double) {
        let c = (1 - abs(2 * l - 1)) * s
        let x = c * (1 - abs((h / 60).truncatingRemainder(dividingBy: 2) - 1))
        let m = l - c / 2

        var r: Double, g: Double, b: Double

        switch h {
        case 0..<60:
            (r, g, b) = (c, x, 0)
        case 60..<120:
            (r, g, b) = (x, c, 0)
        case 120..<180:
            (r, g, b) = (0, c, x)
        case 180..<240:
            (r, g, b) = (0, x, c)
        case 240..<300:
            (r, g, b) = (x, 0, c)
        default:
            (r, g, b) = (c, 0, x)
        }

        return (r + m, g + m, b + m)
    }
}
