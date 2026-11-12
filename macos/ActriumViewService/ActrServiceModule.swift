//
//  ActrServiceModule.swift
//  ActriumViewService
//
//  React Native native module that carries `host:service` calls from the Keel
//  guest across the renderer boundary.
//
//  The Renderer process cannot import the Actr runtime, so every call is
//  forwarded over loopback HTTP to the per-tab endpoint the Host registered in
//  the tab config. When no endpoint is registered the call fails closed with
//  `SERVICE_NOT_FOUND` — there is no implicit gateway fallback.
//

import Foundation

@objc(ActrServiceModule)
final class ActrServiceModule: NSObject {
    @objc
    static func requiresMainQueueSetup() -> Bool {
        return false
    }

    /// Executes one service call and resolves with the base64 response payload.
    ///
    /// The guest already validated the request envelope against the Keel
    /// contract; this method must not trust it beyond parsing.
    @objc
    func call(
        _ tabId: String,
        route: String,
        payloadBase64: String,
        timeoutMs: Double,
        resolve: @escaping (Any?) -> Void,
        reject: @escaping (String?, String?, Error?) -> Void
    ) {
        guard let endpoint = RendererServiceEndpointRegistry.shared.endpoint(for: tabId) else {
            reject(
                "SERVICE_NOT_FOUND",
                "No Service actor endpoint is attached to this tab.",
                nil
            )
            return
        }
        perform(
            endpoint: endpoint,
            route: route,
            payloadBase64: payloadBase64,
            timeoutMs: timeoutMs,
            expectsPayload: true,
            resolve: resolve,
            reject: reject
        )
    }

    /// Fire-and-forget variant. Resolves once the endpoint accepted the tell.
    @objc
    func tell(
        _ tabId: String,
        route: String,
        payloadBase64: String,
        resolve: @escaping (Any?) -> Void,
        reject: @escaping (String?, String?, Error?) -> Void
    ) {
        guard let endpoint = RendererServiceEndpointRegistry.shared.tellEndpoint(for: tabId) else {
            reject(
                "SERVICE_NOT_FOUND",
                "No Service actor endpoint is attached to this tab.",
                nil
            )
            return
        }
        perform(
            endpoint: endpoint,
            route: route,
            payloadBase64: payloadBase64,
            // A tell has no reply to wait for; the Host still bounds the
            // loopback round trip.
            timeoutMs: 30_000,
            expectsPayload: false,
            resolve: resolve,
            reject: reject
        )
    }

    private func perform(
        endpoint: URL,
        route: String,
        payloadBase64: String,
        timeoutMs: Double,
        expectsPayload: Bool,
        resolve: @escaping (Any?) -> Void,
        reject: @escaping (String?, String?, Error?) -> Void
    ) {
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.timeoutInterval = max(timeoutMs / 1000.0, 1.0)

        let body: [String: Any] = [
            "routeKey": route,
            "payloadBase64": payloadBase64,
            "timeoutMs": Int(timeoutMs),
        ]
        do {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
        } catch {
            reject(
                "INVALID_RESPONSE",
                "Unable to encode the service request.",
                error
            )
            return
        }

        URLSession.shared.dataTask(with: request) { data, response, error in
            if let error = error {
                let nsError = error as NSError
                if nsError.domain == NSURLErrorDomain,
                   nsError.code == NSURLErrorTimedOut {
                    reject("TIMED_OUT", error.localizedDescription, error)
                    return
                }
                reject("TRANSPORT_ERROR", error.localizedDescription, error)
                return
            }
            guard let http = response as? HTTPURLResponse else {
                reject(
                    "TRANSPORT_ERROR",
                    "The Service actor returned no HTTP response.",
                    nil
                )
                return
            }
            guard
                let data = data,
                let json = try? JSONSerialization.jsonObject(with: data)
                    as? [String: Any]
            else {
                reject(
                    "INVALID_RESPONSE",
                    "The Service actor returned a malformed response.",
                    nil
                )
                return
            }
            guard (200..<300).contains(http.statusCode) else {
                let status = http.statusCode
                if status == 404 {
                    reject(
                        "SERVICE_NOT_FOUND",
                        json["error"] as? String ?? "Service actor endpoint was not found.",
                        nil
                    )
                } else {
                    reject(
                        "TRANSPORT_ERROR",
                        json["error"] as? String ?? "Service actor request failed.",
                        nil
                    )
                }
                return
            }
            guard json["success"] as? Bool == true else {
                reject(
                    "INTERNAL",
                    json["error"] as? String ?? "Service call failed.",
                    nil
                )
                return
            }
            guard expectsPayload else {
                resolve(nil)
                return
            }
            // A protobuf message may legitimately encode to zero bytes, so an
            // empty base64 string is a valid response; only a missing field is
            // malformed.
            guard let payload = json["payloadBase64"] as? String else {
                reject(
                    "INVALID_RESPONSE",
                    "The Service actor returned no payload field.",
                    nil
                )
                return
            }
            resolve(payload)
        }.resume()
    }
}
