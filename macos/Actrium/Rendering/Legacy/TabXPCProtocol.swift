import Foundation

// MARK: - Tab Configuration for XPC Transfer

/// Configuration data for initializing a tab in the renderer process
@objc(TabXPCConfig)
class TabXPCConfig: NSObject, NSSecureCoding {
    static var supportsSecureCoding: Bool { true }

    let tabId: String
    let title: String
    let url: String?
    let htmlContent: String?
    let unifiedScript: String?
    let capabilityManifestJSON: String?
    let leftPanelScript: String?
    let rightPanelScript: String?
    let backgroundColor: String
    let leftPanelVisible: Bool
    let rightPanelVisible: Bool
    let reloadKey: Int
    let contractName: String?
    let contractVersion: Int?
    let permissions: [String]?

    init(
        tabId: String,
        title: String,
        url: String? = nil,
        htmlContent: String? = nil,
        unifiedScript: String? = nil,
        capabilityManifestJSON: String? = nil,
        leftPanelScript: String? = nil,
        rightPanelScript: String? = nil,
        backgroundColor: String = "#1f1f1f",
        leftPanelVisible: Bool = true,
        rightPanelVisible: Bool = false,
        reloadKey: Int = 0,
        contractName: String? = nil,
        contractVersion: Int? = nil,
        permissions: [String]? = nil
    ) {
        self.tabId = tabId
        self.title = title
        self.url = url
        self.htmlContent = htmlContent
        self.unifiedScript = unifiedScript
        self.capabilityManifestJSON = capabilityManifestJSON
        self.leftPanelScript = leftPanelScript
        self.rightPanelScript = rightPanelScript
        self.backgroundColor = backgroundColor
        self.leftPanelVisible = leftPanelVisible
        self.rightPanelVisible = rightPanelVisible
        self.reloadKey = reloadKey
        self.contractName = contractName
        self.contractVersion = contractVersion
        self.permissions = permissions
        super.init()
    }

    required init?(coder: NSCoder) {
        guard let tabId = coder.decodeObject(of: NSString.self, forKey: "tabId") as String?,
              let title = coder.decodeObject(of: NSString.self, forKey: "title") as String?,
              let backgroundColor = coder.decodeObject(of: NSString.self, forKey: "backgroundColor") as String? else {
            return nil
        }
        self.tabId = tabId
        self.title = title
        self.url = coder.decodeObject(of: NSString.self, forKey: "url") as String?
        self.htmlContent = coder.decodeObject(of: NSString.self, forKey: "htmlContent") as String?
        self.unifiedScript = coder.decodeObject(of: NSString.self, forKey: "unifiedScript") as String?
        self.capabilityManifestJSON = coder.decodeObject(of: NSString.self, forKey: "capabilityManifestJSON") as String?
        self.leftPanelScript = coder.decodeObject(of: NSString.self, forKey: "leftPanelScript") as String?
        self.rightPanelScript = coder.decodeObject(of: NSString.self, forKey: "rightPanelScript") as String?
        self.backgroundColor = backgroundColor
        self.leftPanelVisible = coder.decodeBool(forKey: "leftPanelVisible")
        self.rightPanelVisible = coder.decodeBool(forKey: "rightPanelVisible")
        self.reloadKey = Int(coder.decodeInt64(forKey: "reloadKey"))
        self.contractName = coder.decodeObject(of: NSString.self, forKey: "contractName") as String?
        if coder.containsValue(forKey: "contractVersion") {
            self.contractVersion = Int(coder.decodeInt64(forKey: "contractVersion"))
        } else {
            self.contractVersion = nil
        }
        self.permissions = coder.decodeObject(of: [NSArray.self, NSString.self], forKey: "permissions") as? [String]
        super.init()
    }

    func encode(with coder: NSCoder) {
        coder.encode(tabId as NSString, forKey: "tabId")
        coder.encode(title as NSString, forKey: "title")
        coder.encode(url as NSString?, forKey: "url")
        coder.encode(htmlContent as NSString?, forKey: "htmlContent")
        coder.encode(unifiedScript as NSString?, forKey: "unifiedScript")
        coder.encode(capabilityManifestJSON as NSString?, forKey: "capabilityManifestJSON")
        coder.encode(leftPanelScript as NSString?, forKey: "leftPanelScript")
        coder.encode(rightPanelScript as NSString?, forKey: "rightPanelScript")
        coder.encode(backgroundColor as NSString, forKey: "backgroundColor")
        coder.encode(leftPanelVisible, forKey: "leftPanelVisible")
        coder.encode(rightPanelVisible, forKey: "rightPanelVisible")
        coder.encode(Int64(reloadKey), forKey: "reloadKey")
        coder.encode(contractName as NSString?, forKey: "contractName")
        if let contractVersion = contractVersion {
            coder.encode(Int64(contractVersion), forKey: "contractVersion")
        }
        coder.encode(permissions as NSArray?, forKey: "permissions")
    }

    // MARK: - JSON Serialization for stdin communication

    /// Serialize config to JSON string for pipe-based IPC
    func toJSON() -> String? {
        var dict: [String: Any] = [
            "tabId": tabId,
            "title": title,
            "backgroundColor": backgroundColor,
            "leftPanelVisible": leftPanelVisible,
            "rightPanelVisible": rightPanelVisible,
            "reloadKey": reloadKey
        ]
        if let url = url { dict["url"] = url }
        if let html = htmlContent { dict["htmlContent"] = html }
        if let unified = unifiedScript { dict["unifiedScript"] = unified }
        if let capabilityManifestJSON = capabilityManifestJSON {
            dict["capabilityManifestJSON"] = capabilityManifestJSON
        }
        if let left = leftPanelScript { dict["leftPanelScript"] = left }
        if let right = rightPanelScript { dict["rightPanelScript"] = right }
        if let contractName = contractName, let contractVersion = contractVersion {
            dict["contract"] = [
                "name": contractName,
                "version": contractVersion
            ]
        }
        if let permissions = permissions {
            dict["permissions"] = permissions
        }

        guard let data = try? JSONSerialization.data(withJSONObject: dict),
              let json = String(data: data, encoding: .utf8) else {
            return nil
        }
        return json
    }

    /// Parse config from JSON string
    static func fromJSON(_ json: String) -> TabXPCConfig? {
        guard let data = json.data(using: .utf8),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tabId = dict["tabId"] as? String,
              let title = dict["title"] as? String else {
            return nil
        }

        var contractName: String?
        var contractVersion: Int?
        if let contract = dict["contract"] as? [String: Any] {
            contractName = contract["name"] as? String
            if let version = contract["version"] as? Int {
                contractVersion = version
            } else if let versionNumber = contract["version"] as? NSNumber {
                contractVersion = versionNumber.intValue
            }
        }

        var reloadKey = 0
        if let reload = dict["reloadKey"] as? Int {
            reloadKey = reload
        } else if let reloadNumber = dict["reloadKey"] as? NSNumber {
            reloadKey = reloadNumber.intValue
        }

        return TabXPCConfig(
            tabId: tabId,
            title: title,
            url: dict["url"] as? String,
            htmlContent: dict["htmlContent"] as? String,
            unifiedScript: dict["unifiedScript"] as? String,
            capabilityManifestJSON: dict["capabilityManifestJSON"] as? String,
            leftPanelScript: dict["leftPanelScript"] as? String,
            rightPanelScript: dict["rightPanelScript"] as? String,
            backgroundColor: dict["backgroundColor"] as? String ?? "#1f1f1f",
            leftPanelVisible: dict["leftPanelVisible"] as? Bool ?? true,
            rightPanelVisible: dict["rightPanelVisible"] as? Bool ?? false,
            reloadKey: reloadKey,
            contractName: contractName,
            contractVersion: contractVersion,
            permissions: dict["permissions"] as? [String]
        )
    }
}

// MARK: - Tab Metrics from Renderer Process

/// Performance metrics collected from a tab renderer process
@objc(TabXPCMetrics)
class TabXPCMetrics: NSObject, NSSecureCoding {
    static var supportsSecureCoding: Bool { true }

    let tabId: String
    let pid: Int32
    let memoryBytes: UInt64
    let cpuTimeNanoseconds: UInt64
    let networkBytesReceived: UInt64
    let networkBytesSent: UInt64
    let timestamp: Date

    init(
        tabId: String,
        pid: Int32,
        memoryBytes: UInt64,
        cpuTimeNanoseconds: UInt64,
        networkBytesReceived: UInt64 = 0,
        networkBytesSent: UInt64 = 0
    ) {
        self.tabId = tabId
        self.pid = pid
        self.memoryBytes = memoryBytes
        self.cpuTimeNanoseconds = cpuTimeNanoseconds
        self.networkBytesReceived = networkBytesReceived
        self.networkBytesSent = networkBytesSent
        self.timestamp = Date()
        super.init()
    }

    required init?(coder: NSCoder) {
        guard let tabId = coder.decodeObject(of: NSString.self, forKey: "tabId") as String?,
              let timestamp = coder.decodeObject(of: NSDate.self, forKey: "timestamp") as Date? else {
            return nil
        }
        self.tabId = tabId
        self.pid = coder.decodeInt32(forKey: "pid")
        self.memoryBytes = UInt64(coder.decodeInt64(forKey: "memoryBytes"))
        self.cpuTimeNanoseconds = UInt64(coder.decodeInt64(forKey: "cpuTimeNanoseconds"))
        self.networkBytesReceived = UInt64(coder.decodeInt64(forKey: "networkBytesReceived"))
        self.networkBytesSent = UInt64(coder.decodeInt64(forKey: "networkBytesSent"))
        self.timestamp = timestamp
        super.init()
    }

    func encode(with coder: NSCoder) {
        coder.encode(tabId as NSString, forKey: "tabId")
        coder.encode(pid, forKey: "pid")
        coder.encode(Int64(memoryBytes), forKey: "memoryBytes")
        coder.encode(Int64(cpuTimeNanoseconds), forKey: "cpuTimeNanoseconds")
        coder.encode(Int64(networkBytesReceived), forKey: "networkBytesReceived")
        coder.encode(Int64(networkBytesSent), forKey: "networkBytesSent")
        coder.encode(timestamp as NSDate, forKey: "timestamp")
    }

    /// Memory in megabytes
    var memoryMB: Double {
        Double(memoryBytes) / (1024 * 1024)
    }

    /// CPU time in seconds
    var cpuTimeSeconds: Double {
        Double(cpuTimeNanoseconds) / 1_000_000_000
    }
}

// MARK: - Input Event for XPC Transfer

/// Mouse/keyboard event data for forwarding to renderer
@objc(TabXPCEvent)
class TabXPCEvent: NSObject, NSSecureCoding {
    static var supportsSecureCoding: Bool { true }

    enum EventType: Int {
        case mouseDown = 0
        case mouseUp = 1
        case mouseMoved = 2
        case mouseDragged = 3
        case scrollWheel = 4
        case keyDown = 5
        case keyUp = 6
        case flagsChanged = 7
    }

    let eventType: Int
    let locationX: Double
    let locationY: Double
    let modifierFlags: UInt
    let keyCode: UInt16
    let characters: String?
    let scrollDeltaX: Double
    let scrollDeltaY: Double

    // View size for coordinate scaling
    let viewWidth: Double
    let viewHeight: Double

    init(
        eventType: EventType,
        locationX: Double = 0,
        locationY: Double = 0,
        modifierFlags: UInt = 0,
        keyCode: UInt16 = 0,
        characters: String? = nil,
        scrollDeltaX: Double = 0,
        scrollDeltaY: Double = 0,
        viewWidth: Double = 0,
        viewHeight: Double = 0
    ) {
        self.eventType = eventType.rawValue
        self.locationX = locationX
        self.locationY = locationY
        self.modifierFlags = modifierFlags
        self.keyCode = keyCode
        self.characters = characters
        self.scrollDeltaX = scrollDeltaX
        self.scrollDeltaY = scrollDeltaY
        self.viewWidth = viewWidth
        self.viewHeight = viewHeight
        super.init()
    }

    required init?(coder: NSCoder) {
        self.eventType = coder.decodeInteger(forKey: "eventType")
        self.locationX = coder.decodeDouble(forKey: "locationX")
        self.locationY = coder.decodeDouble(forKey: "locationY")
        self.modifierFlags = UInt(coder.decodeInt64(forKey: "modifierFlags"))
        self.keyCode = UInt16(coder.decodeInteger(forKey: "keyCode"))
        self.characters = coder.decodeObject(of: NSString.self, forKey: "characters") as String?
        self.scrollDeltaX = coder.decodeDouble(forKey: "scrollDeltaX")
        self.scrollDeltaY = coder.decodeDouble(forKey: "scrollDeltaY")
        self.viewWidth = coder.decodeDouble(forKey: "viewWidth")
        self.viewHeight = coder.decodeDouble(forKey: "viewHeight")
        super.init()
    }

    func encode(with coder: NSCoder) {
        coder.encode(eventType, forKey: "eventType")
        coder.encode(locationX, forKey: "locationX")
        coder.encode(locationY, forKey: "locationY")
        coder.encode(Int64(modifierFlags), forKey: "modifierFlags")
        coder.encode(Int(keyCode), forKey: "keyCode")
        coder.encode(characters as NSString?, forKey: "characters")
        coder.encode(scrollDeltaX, forKey: "scrollDeltaX")
        coder.encode(scrollDeltaY, forKey: "scrollDeltaY")
        coder.encode(viewWidth, forKey: "viewWidth")
        coder.encode(viewHeight, forKey: "viewHeight")
    }

    // MARK: - JSON Serialization for stdin communication

    /// Serialize event to JSON string for pipe-based IPC
    func toJSON() -> String? {
        let dict: [String: Any] = [
            "type": eventType,
            "x": locationX,
            "y": locationY,
            "modifiers": modifierFlags,
            "keyCode": keyCode,
            "chars": characters ?? "",
            "scrollX": scrollDeltaX,
            "scrollY": scrollDeltaY,
            "vw": viewWidth,
            "vh": viewHeight
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: dict),
              let json = String(data: data, encoding: .utf8) else {
            return nil
        }
        return "EVENT:\(json)"
    }

    /// Parse event from JSON string
    static func fromJSON(_ json: String) -> TabXPCEvent? {
        guard let data = json.data(using: .utf8),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let typeRaw = dict["type"] as? Int,
              let type = EventType(rawValue: typeRaw) else {
            return nil
        }

        return TabXPCEvent(
            eventType: type,
            locationX: dict["x"] as? Double ?? 0,
            locationY: dict["y"] as? Double ?? 0,
            modifierFlags: (dict["modifiers"] as? UInt) ?? 0,
            keyCode: UInt16(dict["keyCode"] as? Int ?? 0),
            characters: dict["chars"] as? String,
            scrollDeltaX: dict["scrollX"] as? Double ?? 0,
            scrollDeltaY: dict["scrollY"] as? Double ?? 0,
            viewWidth: dict["vw"] as? Double ?? 0,
            viewHeight: dict["vh"] as? Double ?? 0
        )
    }
}

// MARK: - XPC Protocols

/// Protocol for the XPC Broker (main app as server, helpers connect to register)
/// This enables the "reverse connection" pattern where helpers connect first
@objc(TabBrokerProtocol)
protocol TabBrokerProtocol {
    /// Helper registers itself with its anonymous endpoint
    /// - Parameters:
    ///   - tabId: The tab ID this helper is serving
    ///   - endpoint: The helper's anonymous XPC listener endpoint
    ///   - pid: The helper's process ID
    ///   - reply: Callback with success status and optional error message
    func registerHelper(tabId: String, endpoint: NSXPCListenerEndpoint, pid: Int32, reply: @escaping (Bool, String?) -> Void)
}

/// Protocol for the Tab Renderer process (called by main app)
@objc(TabRendererProtocol)
protocol TabRendererProtocol {
    /// Initialize and load tab content
    func loadContent(_ config: TabXPCConfig, reply: @escaping (Bool, String?) -> Void)

    /// Update tab content (e.g., reload)
    func updateContent(_ config: TabXPCConfig, reply: @escaping (Bool, String?) -> Void)

    /// Forward input event to the renderer
    func handleEvent(_ event: TabXPCEvent)

    /// Update viewport size
    func updateViewport(width: Double, height: Double)

    /// Get current metrics
    func getMetrics(reply: @escaping (TabXPCMetrics?) -> Void)

    /// Request the IOSurfaceID for cross-process rendering
    func getIOSurfaceID(reply: @escaping (UInt32) -> Void)

    /// Gracefully terminate the renderer
    func terminate()
}

/// Protocol for callbacks from Renderer to Main App
@objc(TabRendererCallbackProtocol)
protocol TabRendererCallbackProtocol {
    /// Console log from the tab
    func consoleLog(level: String, message: String, source: String)

    /// Tab title changed
    func titleChanged(_ newTitle: String)

    /// Navigation request (e.g., open new tab)
    func navigationRequest(url: String, newTab: Bool)

    /// Surface updated (need to redraw)
    func surfaceDidUpdate()

    /// Renderer crashed or encountered fatal error
    func rendererDidCrash(reason: String)
}

// MARK: - XPC Interface Factory

/// Factory for creating XPC interfaces with proper allowed classes
enum TabXPCInterface {
    /// Fixed Mach service name for the broker (using App Group)
    static let brokerServiceName = "group.io.actrium.actro.broker"

    /// Create the broker protocol interface
    static func brokerInterface() -> NSXPCInterface {
        let interface = NSXPCInterface(with: TabBrokerProtocol.self)
        // NSXPCListenerEndpoint is automatically allowed as it's a system type
        return interface
    }

    /// Create the renderer protocol interface
    static func rendererInterface() -> NSXPCInterface {
        let interface = NSXPCInterface(with: TabRendererProtocol.self)

        // Allow TabXPCConfig for loadContent/updateContent
        let configClasses = NSSet(array: [TabXPCConfig.self]) as! Set<AnyHashable>
        interface.setClasses(configClasses, for: #selector(TabRendererProtocol.loadContent(_:reply:)), argumentIndex: 0, ofReply: false)
        interface.setClasses(configClasses, for: #selector(TabRendererProtocol.updateContent(_:reply:)), argumentIndex: 0, ofReply: false)

        // Allow TabXPCEvent for handleEvent
        let eventClasses = NSSet(array: [TabXPCEvent.self]) as! Set<AnyHashable>
        interface.setClasses(eventClasses, for: #selector(TabRendererProtocol.handleEvent(_:)), argumentIndex: 0, ofReply: false)

        // Allow TabXPCMetrics for getMetrics reply
        let metricsClasses = NSSet(array: [TabXPCMetrics.self]) as! Set<AnyHashable>
        interface.setClasses(metricsClasses, for: #selector(TabRendererProtocol.getMetrics(reply:)), argumentIndex: 0, ofReply: true)

        return interface
    }

    /// Create the callback protocol interface
    static func callbackInterface() -> NSXPCInterface {
        NSXPCInterface(with: TabRendererCallbackProtocol.self)
    }
}
