import Foundation

enum OverlayIPCError: Error {
    case messageTooLarge
    case invalidRequest
    case unsupportedVersion
    case unauthorized
    case invalidCommand
    case invalidArguments
}

@MainActor
final class OverlayIPC {

    private let encoder: JSONEncoder
    private let decoder: JSONDecoder

    init() {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        self.encoder = encoder
        self.decoder = JSONDecoder()
    }

    func handle(
        _ data: Data,
        expectedToken: String,
        manager: OverlayWindowManager
    ) -> Data {

        var requestID: String?

        do {
            guard data.count <= OverlayProtocol.maximumMessageSize else {
                throw OverlayIPCError.messageTooLarge
            }

            let request: OverlayIPCRequest

            do {
                request = try decoder.decode(
                    OverlayIPCRequest.self,
                    from: data
                )
            } catch {
                throw OverlayIPCError.invalidRequest
            }

            requestID = request.requestID

            guard request.version == OverlayProtocol.version else {
                throw OverlayIPCError.unsupportedVersion
            }

            guard request.requestID.range(
                of: "^[A-Za-z0-9_-]{1,64}$",
                options: .regularExpression
            ) != nil else {
                throw OverlayIPCError.invalidRequest
            }

            guard Self.constantTimeEqual(
                request.token,
                expectedToken
            ) else {
                throw OverlayIPCError.unauthorized
            }

            let command = try makeCommand(request)

            let snapshot = try manager.apply(command)

            return encode(
                OverlayIPCResponse(
                    version: OverlayProtocol.version,
                    requestID: request.requestID,
                    success: true,
                    error: nil,
                    window: snapshot
                )
            )

        } catch {
            return encode(
                OverlayIPCResponse(
                    version: OverlayProtocol.version,
                    requestID: requestID,
                    success: false,
                    error: errorCode(error),
                    window: nil
                )
            )
        }
    }

    private func makeCommand(
        _ request: OverlayIPCRequest
    ) throws -> OverlayCommand {

        switch request.command {

        case "register":
            guard
                let id = request.id,
                let kindValue = request.kind,
                let kind = OverlayWindowKind(rawValue: kindValue),
                let width = request.width,
                let height = request.height
            else {
                throw OverlayIPCError.invalidArguments
            }

            return .register(
                id: id,
                kind: kind,
                width: width,
                height: height
            )

        case "close":
            guard let id = request.id else {
                throw OverlayIPCError.invalidArguments
            }

            return .close(id: id)

        case "set_mouse_passthrough":
            guard
                let id = request.id,
                let enabled = request.enabled
            else {
                throw OverlayIPCError.invalidArguments
            }

            return .setMousePassthrough(
                id: id,
                enabled: enabled
            )

        default:
            throw OverlayIPCError.invalidCommand
        }
    }

    private func encode(
        _ response: OverlayIPCResponse
    ) -> Data {
        // The response structure is internally generated and Codable.
        // A minimal fallback is retained for unexpected encoding errors.
        if let data = try? encoder.encode(response) {
            return data
        }

        return Data(
            """
            {"version":1,"requestID":null,"success":false,"error":"internal_error","window":null}
            """.utf8
        )
    }

    private func errorCode(_ error: Error) -> String {
        switch error {
        case OverlayIPCError.messageTooLarge:
            return "message_too_large"
        case OverlayIPCError.invalidRequest:
            return "invalid_request"
        case OverlayIPCError.unsupportedVersion:
            return "unsupported_version"
        case OverlayIPCError.unauthorized:
            return "unauthorized"
        case OverlayIPCError.invalidCommand:
            return "invalid_command"
        case OverlayIPCError.invalidArguments:
            return "invalid_arguments"
        case OverlayCommandError.invalidID:
            return "invalid_id"
        case OverlayCommandError.invalidDimensions:
            return "invalid_dimensions"
        case OverlayManagerError.duplicateID:
            return "duplicate_id"
        case OverlayManagerError.windowNotFound:
            return "window_not_found"
        default:
            return "internal_error"
        }
    }

    private static func constantTimeEqual(
        _ lhs: String,
        _ rhs: String
    ) -> Bool {

        let a = Array(lhs.utf8)
        let b = Array(rhs.utf8)

        // Tokens must be 64 hexadecimal characters.
        guard
            a.count == 64,
            b.count == 64,
            a.allSatisfy({ isHex($0) }),
            b.allSatisfy({ isHex($0) })
        else {
            return false
        }

        var difference: UInt8 = 0

        for index in 0..<64 {
            difference |= a[index] ^ b[index]
        }

        return difference == 0
    }

    private static func isHex(_ byte: UInt8) -> Bool {
        (48...57).contains(byte)
            || (65...70).contains(byte)
            || (97...102).contains(byte)
    }
}
