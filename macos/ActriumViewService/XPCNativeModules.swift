//
//  XPCNativeModules.swift
//  ActriumViewService
//
//  Native modules for React Native running in XPC service.
//  Uses DistributedNotificationCenter to communicate with host process.
//

import Foundation
import React
import ActriumIPC

private enum NativeHostTransport {
    static func post(name: String, userInfo: [String: Any] = [:]) {
        if RendererProcessController.sendHostEvent(name: name, userInfo: userInfo) {
            return
        }
        DistributedNotificationCenter.default().postNotificationName(
            NSNotification.Name(name),
            object: nil,
            userInfo: userInfo.isEmpty ? nil : userInfo,
            deliverImmediately: true
        )
    }
}

@objc(RealmService)
final class RealmService: NSObject {
    @objc(call:route:payloadBase64:timeoutMs:resolver:rejecter:)
    func call(
        _ tabId: String,
        route: String,
        payloadBase64: String,
        timeoutMs: NSNumber,
        resolver resolve: @escaping RCTPromiseResolveBlock,
        rejecter reject: @escaping RCTPromiseRejectBlock
    ) {
        guard let payload = Data(base64Encoded: payloadBase64),
              (1...300_000).contains(timeoutMs.intValue) else {
            reject("invalid_service_call", "Invalid Service call arguments.", nil)
            return
        }
        let call = RealmServiceCall(id: UUID(), route: route, payload: payload, timeoutMs: timeoutMs.intValue)
        DispatchQueue.main.async {
            RendererProcessController.callRealmService(tabId: tabId, call: call) { response in
                if let payload = response.payload {
                    resolve(["payloadBase64": payload.base64EncodedString()])
                } else {
                    reject("realm_service_call_failed", response.error ?? "The Service call failed.", nil)
                }
            }
        }
    }

    @objc static func requiresMainQueueSetup() -> Bool { false }
}

// MARK: - Capability Permission Module

/// Requests a user decision from the trusted Native Front Host. The renderer
/// never persists authorization state itself.
@objc(CapabilityPermission)
class CapabilityPermission: NSObject {
    @objc
    func requestGrant(_ tabId: String, capability: String) {
        NativeHostTransport.post(
            name: "com.actrium.capabilityPermissionRequest",
            userInfo: ["tabId": tabId, "capability": capability]
        )
    }

    @objc
    static func requiresMainQueueSetup() -> Bool {
        return false
    }

    @objc
    static func moduleName() -> String! {
        return "CapabilityPermission"
    }
}

// MARK: - StartPage Module

/// Native module for Start Page actions from React Native (XPC Service version)
@objc(StartPageModule)
class StartPageModule: NSObject {

    @objc
    func openURL(_ urlString: String?) {
        guard let urlString = urlString else {
            NSLog("[StartPageModule] URL is nil")
            return
        }

        NSLog("[StartPageModule] Opening URL: \(urlString)")

        NativeHostTransport.post(
            name: "com.actro.openURLFromStartPage",
            userInfo: ["url": urlString]
        )
    }

    @objc
    static func requiresMainQueueSetup() -> Bool {
        return false
    }

    @objc
    static func moduleName() -> String! {
        return "StartPageModule"
    }
}

// MARK: - History Module

/// Native module for history actions from React Native (XPC Service version)
@objc(HistoryModule)
class HistoryModule: NSObject {

    @objc
    func openHistoryItem(_ itemId: String, _ appURL: String?) {
        guard let urlString = appURL else {
            NSLog("[HistoryModule] URL is nil")
            return
        }

        NSLog("[HistoryModule] Opening history item: \(itemId), url: \(urlString)")

        NativeHostTransport.post(
            name: "com.actro.openHistoryItem",
            userInfo: ["itemId": itemId, "url": urlString]
        )
    }

    @objc
    func deleteHistoryItem(_ itemId: String) {
        NSLog("[HistoryModule] Delete history item: \(itemId)")

        NativeHostTransport.post(
            name: "com.actro.deleteHistoryItem",
            userInfo: ["itemId": itemId]
        )
    }

    @objc
    func restoreGroup(_ groupId: String) {
        NSLog("[HistoryModule] Restore group: \(groupId)")

        NativeHostTransport.post(
            name: "com.actro.restoreHistoryGroup",
            userInfo: ["groupId": groupId]
        )
    }

    @objc
    func clearHistory() {
        NSLog("[HistoryModule] Clear all history")

        NativeHostTransport.post(name: "com.actro.clearHistory")
    }

    @objc
    static func requiresMainQueueSetup() -> Bool {
        return false
    }

    @objc
    static func moduleName() -> String! {
        return "HistoryModule"
    }
}

// MARK: - Bookmarks Module

/// Native module for bookmark actions from React Native (XPC Service version)
@objc(BookmarksModule)
class BookmarksModule: NSObject {

    @objc
    func openBookmark(_ bookmarkId: String, _ appURL: String?) {
        guard let urlString = appURL else {
            NSLog("[BookmarksModule] URL is nil")
            return
        }

        NSLog("[BookmarksModule] Opening bookmark: \(bookmarkId), url: \(urlString)")

        NativeHostTransport.post(
            name: "com.actro.openBookmark",
            userInfo: ["bookmarkId": bookmarkId, "url": urlString]
        )
    }

    @objc
    func deleteBookmark(_ bookmarkId: String) {
        NSLog("[BookmarksModule] Delete bookmark: \(bookmarkId)")

        NativeHostTransport.post(
            name: "com.actro.deleteBookmark",
            userInfo: ["bookmarkId": bookmarkId]
        )
    }

    @objc
    static func requiresMainQueueSetup() -> Bool {
        return false
    }

    @objc
    static func moduleName() -> String! {
        return "BookmarksModule"
    }
}

// MARK: - Native Logger Module

/// Native logger module for React Native (XPC Service version)
/// Logs to stderr and optionally sends to host for DevTools Console
@objc(NativeLogger)
class NativeLogger: NSObject {

    // Remote log server for development
    private static let logServerURL = "http://localhost:9999/log"

    /// Write to shared IPC debug log file for cross-process log correlation
    private static func writeToIPCLog(_ message: String) {
        if RendererProcessController.isRendererProcess {
            return
        }
        let timestamp: String = {
            let now = Date()
            let formatter = DateFormatter()
            formatter.dateFormat = "HH:mm:ss.SSS"
            return formatter.string(from: now)
        }()
        let line = "[\(timestamp)] [JS] \(message)\n"

        // Write to shared IPC log file
        let logPath = "/tmp/actrium_ipc_debug.log"
        if let handle = FileHandle(forWritingAtPath: logPath) {
            handle.seekToEndOfFile()
            if let data = line.data(using: .utf8) {
                handle.write(data)
            }
            handle.closeFile()
        } else {
            FileManager.default.createFile(atPath: logPath, contents: line.data(using: .utf8))
        }
    }

    private static func sendToHost(level: String, source: String, message: String) {
        if RendererProcessController.sendConsoleLog(
            level: level,
            source: source,
            message: message
        ) {
            return
        }
        // Send log to host process for DevTools Console
        DistributedNotificationCenter.default().postNotificationName(
            NSNotification.Name("com.actro.xpcLog"),
            object: nil,
            userInfo: [
                "level": level,
                "source": source,
                "message": message
            ],
            deliverImmediately: true
        )
    }

    private static func sendToRemote(level: String, message: String) {
        // Disabled: Remote log server causes "Connection refused" noise when not running
        // The DevTools Console receives logs via DistributedNotificationCenter (sendToHost)
        // which is more reliable and doesn't require a separate server process
    }

    @objc
    func log(_ message: String) {
        NSLog("[JS] %@", message)
        NativeLogger.writeToIPCLog(message)
        NativeLogger.sendToHost(level: "info", source: "JS", message: message)
        NativeLogger.sendToRemote(level: "log", message: message)
    }

    @objc
    func warn(_ message: String) {
        NSLog("[JS WARN] %@", message)
        NativeLogger.sendToHost(level: "warn", source: "JS", message: message)
        NativeLogger.sendToRemote(level: "warn", message: message)
    }

    @objc
    func error(_ message: String) {
        NSLog("[JS ERROR] %@", message)
        NativeLogger.sendToHost(level: "error", source: "JS", message: message)
        NativeLogger.sendToRemote(level: "error", message: message)
    }

    @objc
    static func nativeLog(_ message: String) {
        NSLog("[Native] %@", message)
        sendToHost(level: "info", source: "Native", message: message)
        sendToRemote(level: "log", message: "[Native] \(message)")
    }

    @objc
    static func nativeWarn(_ message: String) {
        NSLog("[Native WARN] %@", message)
        sendToHost(level: "warn", source: "Native", message: message)
        sendToRemote(level: "warn", message: "[Native] \(message)")
    }

    @objc
    static func nativeError(_ message: String) {
        NSLog("[Native ERROR] %@", message)
        sendToHost(level: "error", source: "Native", message: message)
        sendToRemote(level: "error", message: "[Native] \(message)")
    }

    @objc
    static func requiresMainQueueSetup() -> Bool {
        return false
    }

    @objc
    static func moduleName() -> String! {
        return "NativeLogger"
    }
}
