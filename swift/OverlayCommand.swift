import Foundation

enum OverlayWindowKind: String, Codable, Sendable {
    case status
    case control
    case diagnostic
}

enum OverlayCommandError: Error, Equatable {
    case invalidID
    case invalidDimensions
}

enum OverlayCommand: Sendable {
    case register(
        id: String,
        kind: OverlayWindowKind,
        width: Double,
        height: Double
    )
    case close(id: String)
    case setMousePassthrough(id: String, enabled: Bool)

    func validate() throws {
        func validateID(_ id: String) throws {
            let valid = id.range(
                of: "^[A-Za-z0-9_-]{1,64}$",
                options: .regularExpression
            ) != nil

            guard valid else {
                throw OverlayCommandError.invalidID
            }
        }

        switch self {
        case let .register(id, _, width, height):
            try validateID(id)

            guard width.isFinite,
                  height.isFinite,
                  (64...4096).contains(width),
                  (64...4096).contains(height) else {
                throw OverlayCommandError.invalidDimensions
            }

        case let .close(id):
            try validateID(id)

        case let .setMousePassthrough(id, _):
            try validateID(id)
        }
    }
}
