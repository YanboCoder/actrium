import Foundation

public struct RealmServiceCall: Codable, Sendable {
    public static let requestEvent = "com.actrium.realmService.request"
    public static let responseEvent = "com.actrium.realmService.response"

    public let id: UUID
    public let route: String
    public let payload: Data
    public let timeoutMs: Int

    public init(id: UUID, route: String, payload: Data, timeoutMs: Int) {
        self.id = id
        self.route = route
        self.payload = payload
        self.timeoutMs = timeoutMs
    }
}

public struct RealmServiceReply: Codable, Sendable {
    public let id: UUID
    public let payload: Data?
    public let error: String?

    public init(id: UUID, payload: Data? = nil, error: String? = nil) {
        self.id = id
        self.payload = payload
        self.error = error
    }
}
