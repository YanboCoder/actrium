import Foundation
import React
import React_RCTAppDelegate
import ReactAppDependencyProvider

private let logger = AppLogger.reactNative

// Easy Log Functions - Output to os.Logger(and optional remote servers)
/// Convenience logging helpers.
///
/// - Always logs to `os.Logger`.
/// - Only forwards to the remote log server in DEBUG (to avoid production noise / PII risk).
private func nlog(_ message: String) {
    logger.debug(message)
    #if DEBUG
    NativeLogger.nativeLog(message)
    #endif
}

private func nnotice(_ message: String) {
    logger.notice(message)
    #if DEBUG
    NativeLogger.nativeLog(message)
    #endif
}

private func nwarn(_ message: String) {
    logger.warning(message)
    #if DEBUG
    NativeLogger.nativeWarn(message)
    #endif
}

private func nerror(_ message: String) {
    logger.error(message)
    #if DEBUG
    NativeLogger.nativeError(message)
    #endif
}

private func nfault(_ message: String) {
    logger.fault(message)
    #if DEBUG
    NativeLogger.nativeError(message)
    #endif
}

final class ActriumReactNativeFactoryDelegate: RCTDefaultReactNativeFactoryDelegate {
    override init() {
        super.init()
        self.dependencyProvider = RCTAppDependencyProvider()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func bundleURL() -> URL? {
        #if DEBUG
        // Forced to leave while developing Metro(Supporting heat loads);Debug The environment is not going back offline. runtime.jsbundle(avoiding to expire) bundle Result"Whitescreen Unrendered")
        // 🟢 Special attention is paid to: iPad The real thing is, when you debug it,RCTBundleURLProvider Yeah. IP:Port The Analysis of Unstable
        // Direct Construct URL It's to make sure iPad It's a very precise connection. Mac Metro (8082) The surest way to do it.
        let packagerHost = "localhost" // Or fill it in. Mac Local area network IP Address
        let port = 8082
        let url = URL(string: "http://\(packagerHost):\(port)/index.bundle?platform=ios&dev=true&minify=false")
        
        print("🌐 [Native] Force Pointing Metro URL: \(url?.absoluteString ?? "nil")")
        return url
        #else
        // Release offline bundle. Not "main.jsbundle": that collides with the
        // RN default product and Xcode errors "Multiple commands produce …".
        // The name must match the --bundle-output file name produced by
        // ios/Scripts/* (af, test-build.sh, upload-testflight.sh).
        return Bundle.main.url(forResource: "runtime", withExtension: "jsbundle")
        #endif
    }

    override func sourceURL(for bridge: RCTBridge) -> URL? {
        return bundleURL()
    }
}

final class ActriumReactNativeHost {
    static let shared = ActriumReactNativeHost()

    private let factoryDelegate: ActriumReactNativeFactoryDelegate
    private let factory: RCTReactNativeFactory
    private var bridge: RCTBridge?
    private var viewCache: [UUID: RCTRootView] = [:]

    private init() {
        nnotice("🏗️ [Native] ActriumReactNativeHost Initialize")

        #if DEBUG
        // macOS RN: LogBox/RedBox Possible trigger on some versions UIBlock Unusual (e.g.) NULL CGColor) , which leads to UI No Rendering
        // Disable it first RedBox(LogBox.show And you'll be. gate) avoids affecting the main interface rendering.
        RCTRedBoxSetEnabled(false)
        nwarn("🧪 [Native] Debug: Disabled RedBox/LogBox(avoidance) UIBlock Unusual White Screen)")
        #endif

        factoryDelegate = ActriumReactNativeFactoryDelegate()
        factory = RCTReactNativeFactory(delegate: factoryDelegate)
        nnotice("✅ [Native] Factory Create finished")
    }

    func viewForTab(tabId: UUID, initialProperties: [String: Any]) -> UIView {
        if let cachedView = viewCache[tabId] {
            nlog("♻️ [Native] Use Cache RootView: \(tabId)")
            return cachedView
        }

        nnotice("📱 [Native] Yes Tab \(tabId) Create New RootView")
        let rootView = createRootView(initialProperties: initialProperties)
        if let rctRootView = rootView as? RCTRootView {
            viewCache[tabId] = rctRootView
        }
        return rootView
    }

    func removeViewForTab(tabId: UUID) {
        if viewCache.removeValue(forKey: tabId) != nil {
            nlog("🗑️ [Native] Remove Tab Cache View: \(tabId)")
            // I can do extra cleaning here.
        }
    }

    func createRootView(initialProperties: [String: Any]) -> UIView {
        nnotice("📱 [Native] Create RootView,module name: actrium")

        let rootView = factory.rootViewFactory.view(
            withModuleName: "actrium",
            initialProperties: initialProperties,
            launchOptions: nil
        )

        if let rctRootView = rootView as? RCTRootView {
            // Store bridge reference
            self.bridge = rctRootView.bridge
            
            // Listening React Content first mounted (when) contentView ♪ I've got my first subview ♪
            NotificationCenter.default.addObserver(
                forName: NSNotification.Name(rawValue: "RCTContentDidAppearNotification"),
                object: rctRootView,
                queue: .main
            ) { _ in
                
                // Output Summary
            }

            // Listening Bridge Load complete.
            NotificationCenter.default.addObserver(
                forName: .RCTJavaScriptDidLoad,
                object: nil,
                queue: .main
            ) { _ in
            }

            // Listening Bridge Loading failed
            NotificationCenter.default.addObserver(
                forName: .RCTJavaScriptDidFailToLoad,
                object: nil,
                queue: .main
            ) { notification in
                if let error = notification.userInfo?["error"] as? Error {
                    nerror("❌ [Native] JS Loading failed: \(error.localizedDescription)")
                }
            }

            nnotice("✅ [Native] RCTRootView Created successfully")

            // Inspection bridge Status
            let bridge = rctRootView.bridge

            #if DEBUG
            // Disable RN Dev Menu Default Shortcut Keys (%2)Cmd+D, Cmd+I)
            // We use our own. DevTools Replace
            if let devMenu = bridge.module(forName: "DevMenu") as? RCTDevMenu {
                devMenu.hotkeysEnabled = false
            }
            
            // Use RCTDevSettings Configure Disable Shake Gestures
            if let devSettings = bridge.devSettings {
                devSettings.isShakeToShowDevMenuEnabled = false
            }
            #endif

            // Set Background Transparent
            rctRootView.backgroundColor = UIColor.clear
            

            // Delayed inspection JavaScript Load successful
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {

                // Manually repaired RCTRootContentView It's... frame
                let contentView = rctRootView.contentView
                let parentFrame = rctRootView.bounds
                contentView.frame = parentFrame
                contentView.setNeedsLayout()
                contentView.layoutIfNeeded()
            }

            DispatchQueue.main.asyncAfter(deadline: .now() + 5.0) {

                // Recursive print view level
                func logSubviews(_ view: UIView, depth: Int = 0) {
                    let indent = String(repeating: "  ", count: depth)
                    for (index, subview) in view.subviews.enumerated() {
                        nlog("\(indent)[\(index)] \(type(of: subview)) - frame: \(subview.frame.size)")
                        logSubviews(subview, depth: depth + 1)
                    }
                }
                logSubviews(rctRootView)

                // Inspection RCTRootContentView Subview
                let contentView = rctRootView.contentView
                if contentView.subviews.isEmpty {
                    nerror("❌ [Native] ContentView No subview!React Probably unrendered")
                    nerror("👉 [Native] Debug: Confirm Metro Started (`cd Actrium/ActriumRuntime && bun run start`);Release: Ensure generated `runtime.jsbundle`")
                } else {
                }
            }
        } else {
            nfault("❌ [Native] RootView No, it's not. RCTRootView Type")
        }

        return rootView
    }
    
    func evaluateScript(_ code: String, tabId: UUID) {
        guard let bridge = self.bridge else { return }
        
        // Send event to RuntimeApp to evaluate in the specific engine
        bridge.enqueueJSCall(
            "RCTDeviceEventEmitter",
            method: "emit",
            args: [
                "DEVTOOLS_EVAL",
                [
                    "tabId": tabId.uuidString,
                    "code": code
                ]
            ],
            completion: nil
        )
    }
}
