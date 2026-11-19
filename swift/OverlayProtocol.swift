import Foundation

enum OverlayProtocol {
    static let version = 1
    static let maximumMessageSize = 16 * 1024
}

struct OverlayIPCRequest: Codable, Sendable {
    let version: Int
    let requestID: String
    let token: String
    let command: String

    let id: String?
    let kind: String?
    let width: Double?
    let height: Double?
    let enabled: Bool?
}

struct OverlayIPCResponse: Codable, Sendable {
    let version: Int
    let requestID: String?
    let success: Bool
    let error: String?
    let window: OverlayWindowSnapshot?
}
