//
//  TabSurfaceManager.swift
//  Actrium
//
//  Manages tab rendering lifecycle independently of SwiftUI views.
//  Provides contextId for display views to reference.
//
//  Architecture:
//  - Rendering is decoupled from display
//  - All tabs render at fixed viewportSize
//  - Display views only reference contextId and apply scaleEffect
//

import SwiftUI
import Combine

// MARK: - Tab Surface Info

/// Information about a tab's rendering surface
struct TabSurfaceInfo: Identifiable {
    let id: UUID  // Tab ID
    var contextId: UInt32 = 0
    var isLoading: Bool = true
    var errorMessage: String?
    var crashReason: String?
}

// MARK: - Tab Surface Manager

/// Manages tab rendering lifecycle independently of SwiftUI view hierarchy.
///
/// Key responsibilities:
/// - Start/stop renderers for tabs
/// - Maintain fixed viewportSize for all renderers
/// - Provide contextId for display views
/// - Handle renderer crashes and recovery
@MainActor
class TabSurfaceManager: ObservableObject {

    // MARK: - Published State

    /// Surface info for all managed tabs
    @Published private(set) var surfaces: [UUID: TabSurfaceInfo] = [:]

    /// The fixed viewport size for rendering (set once, rarely changes)
    @Published private(set) var viewportSize: CGSize = .zero

    // MARK: - Private State

    private let processManager = LegacyRendererProcessManager.shared
    private var cancellables = Set<AnyCancellable>()
    private var startingTabs = Set<UUID>()

    // MARK: - Initialization

    init() {
        setupNotificationObservers()
    }

    // MARK: - Public API

    /// Update the viewport size for all renderers
    /// Call this when the main viewport size changes (e.g., window resize)
    func updateViewportSize(_ size: CGSize) {
        guard size.width > 100 && size.height > 100 else { return }
        guard abs(size.width - viewportSize.width) > 2 || abs(size.height - viewportSize.height) > 2 else { return }

        NSLog("📐 [TabSurfaceManager] Viewport size updated: %.0fx%.0f -> %.0fx%.0f",
              viewportSize.width, viewportSize.height, size.width, size.height)

        viewportSize = size

        // Update all active renderers with new viewport size
        for (tabId, _) in surfaces {
            processManager.updateViewport(width: size.width, height: size.height, for: tabId)
        }
    }

    /// Ensure a tab has an active renderer
    /// If already rendering, returns immediately. Otherwise starts the renderer.
    func ensureRendering(tab: Tab) {
        let tabId = tab.id

        // Already have a valid surface
        if let existing = surfaces[tabId], existing.contextId != 0, existing.crashReason == nil {
            return
        }

        // Already starting
        if startingTabs.contains(tabId) {
            return
        }

        // Start the renderer
        startRenderer(for: tab)
    }

    /// Stop rendering for a tab (when tab is closed)
    func stopRendering(for tabId: UUID) {
        NSLog("🔴 [TabSurfaceManager] Stopping renderer for tab \(tabId)")

        processManager.terminateProcess(for: tabId)
        surfaces.removeValue(forKey: tabId)
        startingTabs.remove(tabId)
        CALayerHostViewCache.shared.remove(for: tabId)
    }

    /// Get the contextId for a tab (returns 0 if not available)
    func contextId(for tabId: UUID) -> UInt32 {
        return surfaces[tabId]?.contextId ?? 0
    }

    /// Check if a tab is currently loading
    func isLoading(for tabId: UUID) -> Bool {
        return surfaces[tabId]?.isLoading ?? true
    }

    /// Check if a tab has crashed
    func crashReason(for tabId: UUID) -> String? {
        return surfaces[tabId]?.crashReason
    }

    /// Reload a crashed tab
    func reloadCrashedTab(_ tab: Tab) {
        let tabId = tab.id
        NSLog("🔄 [TabSurfaceManager] Reloading crashed tab \(tabId)")

        // Clear state
        surfaces[tabId] = nil
        startingTabs.remove(tabId)
        RendererStartupTracker.shared.reset(tabId)

        // Restart
        startRenderer(for: tab)
    }

    // MARK: - Private Methods

    private func setupNotificationObservers() {
        // Surface ID updates
        NotificationCenter.default.publisher(for: .tabSurfaceUpdated)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] notification in
                guard let tabId = notification.userInfo?["tabId"] as? UUID,
                      let contextId = notification.userInfo?["surfaceID"] as? UInt32,
                      contextId != 0 else { return }

                self?.handleSurfaceUpdate(tabId: tabId, contextId: contextId)
            }
            .store(in: &cancellables)

        // Renderer crashes
        NotificationCenter.default.publisher(for: .tabRendererCrashed)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] notification in
                guard let tabId = notification.userInfo?["tabId"] as? UUID else { return }
                let reason = notification.userInfo?["reason"] as? String ?? "Unknown error"

                self?.handleCrash(tabId: tabId, reason: reason)
            }
            .store(in: &cancellables)
    }

    private func handleSurfaceUpdate(tabId: UUID, contextId: UInt32) {
        NSLog("📐 [TabSurfaceManager] Surface updated: tabId=\(tabId), contextId=\(contextId)")

        var info = surfaces[tabId] ?? TabSurfaceInfo(id: tabId)
        info.contextId = contextId
        info.isLoading = false
        info.crashReason = nil
        surfaces[tabId] = info

        startingTabs.remove(tabId)
    }

    private func handleCrash(tabId: UUID, reason: String) {
        NSLog("💥 [TabSurfaceManager] Renderer crashed: tabId=\(tabId), reason=\(reason)")

        var info = surfaces[tabId] ?? TabSurfaceInfo(id: tabId)
        info.contextId = 0
        info.isLoading = false
        info.crashReason = reason
        surfaces[tabId] = info

        startingTabs.remove(tabId)
    }

    private func startRenderer(for tab: Tab) {
        let tabId = tab.id

        // Mark as starting
        startingTabs.insert(tabId)
        surfaces[tabId] = TabSurfaceInfo(id: tabId, isLoading: true)

        NSLog("🟢 [TabSurfaceManager] Starting renderer for tab \(tabId)")

        Task {
            do {
                // Check if already running
                if let existing = processManager.processes[tabId], existing.isRunning {
                    if let id = await processManager.getIOSurfaceID(for: tabId), id != 0 {
                        await MainActor.run {
                            handleSurfaceUpdate(tabId: tabId, contextId: id)
                        }
                        return
                    }
                }

                // Build config
                let config = TabXPCConfig(
                    tabId: tabId.uuidString,
                    title: tab.title.isEmpty ? "New Tab" : tab.title,
                    url: tab.url?.absoluteString,
                    htmlContent: tab.htmlContent,
                    unifiedScript: tab.unifiedScript,
                    capabilityManifestJSON: tab.capabilityManifestJSON,
                    leftPanelScript: tab.leftPanelScript,
                    rightPanelScript: tab.rightPanelScript,
                    backgroundColor: tab.backgroundColor,
                    leftPanelVisible: tab.leftPanelDefaultVisible,
                    rightPanelVisible: tab.rightPanelDefaultVisible,
                    reloadKey: tab.reloadNonce,
                    contractName: tab.contractName,
                    contractVersion: tab.contractVersion,
                    permissions: tab.permissions
                )

                // Start renderer with current viewport size
                let renderSize = viewportSize.width > 100 ? viewportSize : CGSize(width: 800, height: 600)
                _ = try await processManager.getRenderer(for: tabId, config: config, viewportSize: renderSize)

                // Get the surface ID
                if let id = await processManager.getIOSurfaceID(for: tabId), id != 0 {
                    await MainActor.run {
                        handleSurfaceUpdate(tabId: tabId, contextId: id)
                    }
                }
            } catch {
                await MainActor.run {
                    NSLog("🔴 [TabSurfaceManager] Failed to start renderer: \(error)")
                    var info = surfaces[tabId] ?? TabSurfaceInfo(id: tabId)
                    info.isLoading = false
                    info.errorMessage = error.localizedDescription
                    surfaces[tabId] = info
                    startingTabs.remove(tabId)
                }
            }
        }
    }
}

// MARK: - CALayerHost View Cache (moved here for access)

/// Cache for CALayerHostDisplayView instances
class CALayerHostViewCache {
    static let shared = CALayerHostViewCache()

    private var cache: [UUID: CALayerHostDisplayView] = [:]
    private let lock = NSLock()

    func getOrCreate(for tabId: UUID, frame: NSRect) -> CALayerHostDisplayView {
        lock.lock()
        defer { lock.unlock() }

        if let existing = cache[tabId] {
            return existing
        }

        let view = CALayerHostDisplayView(frame: frame)
        cache[tabId] = view
        return view
    }

    func remove(for tabId: UUID) {
        lock.lock()
        defer { lock.unlock() }
        cache.removeValue(forKey: tabId)
    }
}
