//
//  RendererServiceEndpointRegistry.swift
//  ActriumViewService
//
//  Per-tab map of `host:service` endpoints supplied by the Host over the
//  Renderer tab config. The Renderer process cannot import the Actr runtime
//  (it only has IPC + loopback HTTP to the main process), so the endpoint
//  URL is the contract that lets `ActrServiceModule` reach the tab's Service
//  actor without hardcoding a gateway.
//

import Foundation

/// Thread-safe `tabId → serviceCallUrl` registry, written when the Host
/// applies a tab config and read by `ActrServiceModule`.
final class RendererServiceEndpointRegistry {
    static let shared = RendererServiceEndpointRegistry()

    private let lock = NSLock()
    private var endpoints: [String: URL] = [:]

    private init() {}

    /// Records the tab's `/v1/call` endpoint. Invalid URLs are dropped; the
    /// guest then fails closed with `SERVICE_NOT_FOUND` instead of silently
    /// falling back to a global gateway.
    func register(tabId: String, serviceCallUrl: String) {
        guard !tabId.isEmpty,
              let url = URL(string: serviceCallUrl),
              url.scheme == "http",
              url.host == "127.0.0.1" || url.host == "localhost"
        else { return }
        lock.lock()
        endpoints[tabId] = url
        lock.unlock()
    }

    func remove(tabId: String) {
        lock.lock()
        endpoints.removeValue(forKey: tabId)
        lock.unlock()
    }

    func endpoint(for tabId: String) -> URL? {
        lock.lock()
        defer { lock.unlock() }
        return endpoints[tabId]
    }

    /// Derives a sibling endpoint (`/v1/tell`) from the registered call URL.
    func tellEndpoint(for tabId: String) -> URL? {
        guard let callURL = endpoint(for: tabId),
              var components = URLComponents(url: callURL, resolvingAgainstBaseURL: false)
        else { return nil }
        components.path = "/v1/tell"
        return components.url
    }
}
