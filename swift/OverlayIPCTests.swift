#if OVERLAY_IPC_SELF_TEST

import Foundation

private let ipcTestToken = String(repeating: "a", count: 64)

@main
@MainActor
struct OverlayIPCSelfTest {

    static func makeRequest(
        command: String = "register",
        token: String = ipcTestToken,
        version: Int = OverlayProtocol.version,
        id: String? = "ipc-test",
        kind: String? = "status",
        width: Double? = 640,
        height: Double? = 360,
        enabled: Bool? = nil
    ) throws -> Data {

        let request = OverlayIPCRequest(
            version: version,
            requestID: "test-request-001",
            token: token,
            command: command,
            id: id,
            kind: kind,
            width: width,
            height: height,
            enabled: enabled
        )

        return try JSONEncoder().encode(request)
    }

    static func decode(_ data: Data) throws -> OverlayIPCResponse {
        try JSONDecoder().decode(OverlayIPCResponse.self, from: data)
    }

    static func check(
        _ name: String,
        _ condition: Bool
    ) {
        guard condition else {
            fatalError("[FAIL] \(name)")
        }

        print("[PASS] \(name)")
    }

    static func main() throws {

        let ipc = OverlayIPC()
        let manager = OverlayWindowManager()

        // 1. Valid authorization and registration.
        let valid = try makeRequest()

        let validResponse = try decode(
            ipc.handle(
                valid,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Authorized registration",
            validResponse.success
                && validResponse.window?.id == "ipc-test"
                && manager.windowCount == 1
        )

        // 2. Invalid token must not mutate state.
        let unauthorized = try makeRequest(
            token: String(repeating: "b", count: 64),
            id: "unauthorized-window"
        )

        let unauthorizedResponse = try decode(
            ipc.handle(
                unauthorized,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Invalid token rejected",
            !unauthorizedResponse.success
                && unauthorizedResponse.error == "unauthorized"
                && manager.windowCount == 1
        )

        // 3. Unsupported protocol version.
        let wrongVersion = try makeRequest(
            version: 999,
            id: "wrong-version-window"
        )

        let versionResponse = try decode(
            ipc.handle(
                wrongVersion,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Unsupported protocol version rejected",
            !versionResponse.success
                && versionResponse.error == "unsupported_version"
                && manager.windowCount == 1
        )

        // 4. Oversized message.
        let oversized = Data(
            repeating: 0x41,
            count: OverlayProtocol.maximumMessageSize + 1
        )

        let oversizedResponse = try decode(
            ipc.handle(
                oversized,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Oversized message rejected",
            !oversizedResponse.success
                && oversizedResponse.error == "message_too_large"
                && manager.windowCount == 1
        )

        // 5. Malformed JSON.
        let malformed = Data("{invalid-json".utf8)

        let malformedResponse = try decode(
            ipc.handle(
                malformed,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Malformed JSON rejected",
            !malformedResponse.success
                && malformedResponse.error == "invalid_request"
                && manager.windowCount == 1
        )

        // 6. Unknown command.
        let unknown = try makeRequest(
            command: "execute_arbitrary_code",
            id: "unknown-command-window"
        )

        let unknownResponse = try decode(
            ipc.handle(
                unknown,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Unknown command rejected",
            !unknownResponse.success
                && unknownResponse.error == "invalid_command"
                && manager.windowCount == 1
        )

        // 7. Duplicate ID must not replace the existing window.
        let duplicate = try makeRequest(
            id: "ipc-test",
            width: 320,
            height: 200
        )

        let duplicateResponse = try decode(
            ipc.handle(
                duplicate,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        let existing = manager.snapshot().first

        check(
            "Duplicate ID rejected without state mutation",
            !duplicateResponse.success
                && duplicateResponse.error == "duplicate_id"
                && manager.windowCount == 1
                && existing?.width == 640
                && existing?.height == 360
        )

        // 8. Invalid dimensions.
        let invalidDimensions = try makeRequest(
            id: "invalid-dimensions",
            width: 10,
            height: 20
        )

        let dimensionsResponse = try decode(
            ipc.handle(
                invalidDimensions,
                expectedToken: ipcTestToken,
                manager: manager
            )
        )

        check(
            "Invalid dimensions rejected",
            !dimensionsResponse.success
                && dimensionsResponse.error == "invalid_dimensions"
                && manager.windowCount == 1
        )

        print("")
        print("[RESULT] All Overlay IPC tests passed")
    }
}

#endif
