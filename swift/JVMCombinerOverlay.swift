
import AppKit
import Foundation
import Darwin

private enum OverlayTokenError: Error {
    case invalidFileDescriptor
    case readFailed
    case unexpectedEOF
    case invalidToken
    case trailingData
}

@main
@MainActor
struct JVMCombinerOverlay {
    private static let tokenFDEnvironment =
        "JVM_COMBINER_OVERLAY_TOKEN_FD"

    private static let socketEnvironment =
        "JVM_COMBINER_OVERLAY_SOCKET"

    static func main() {
        do {
            let token = try readTokenFromInheritedFD()

            guard let socketPath = ProcessInfo.processInfo.environment[
                socketEnvironment
            ], !socketPath.isEmpty else {
                fail("Missing overlay socket path")
            }

            let manager = OverlayWindowManager()
           let ipc = OverlayIPC()

           let requestHandler = OverlayRequestHandler(
               ipc: ipc,
               manager: manager,
               token: token
           )

           let transportHandler =
               OverlayRequestHandler.makeTransportHandler(requestHandler)

           let server = OverlayUDSServer(
               path: socketPath,
               handler: transportHandler
           )

           try server.start()

            print("[OK] JVM Combiner Swift backend initialized")
            print("[OK] Overlay IPC authenticated handler ready")
            print("[OK] Overlay UDS server started")
            print("[INFO] Registered windows: \(manager.windowCount)")
            print("[SAFE] No native window operations were performed")

            // Keep the AppKit main thread and MainActor alive.
            RunLoop.main.run()

            server.stop()
        } catch {
            fail("Overlay initialization failed: \(error)")
        }
    }

    private static func readTokenFromInheritedFD() throws -> String {
        guard
            let rawFD = ProcessInfo.processInfo.environment[
                tokenFDEnvironment
            ],
            let fd = Int32(rawFD),
            fd >= 0
        else {
            throw OverlayTokenError.invalidFileDescriptor
        }

        defer {
            _ = Darwin.close(fd)
        }

        var bytes = [UInt8](repeating: 0, count: 64)
        let byteCount = bytes.count
        var offset = 0

        while offset < byteCount {
            let result = bytes.withUnsafeMutableBytes { buffer in
                Darwin.read(
                    fd,
                    buffer.baseAddress!.advanced(by: offset),
                    byteCount - offset
                )
            }

            if result < 0 {
                if errno == EINTR {
                    continue
                }

                throw OverlayTokenError.readFailed
            }

            guard result > 0 else {
                throw OverlayTokenError.unexpectedEOF
            }

            offset += result
        }

        // Require EOF immediately after the 64-byte token.
        var extraByte: UInt8 = 0
        let extraCount = Darwin.read(fd, &extraByte, 1)

        guard extraCount == 0 else {
            throw OverlayTokenError.trailingData
        }

        guard bytes.allSatisfy({
            (48...57).contains($0)
                || (65...70).contains($0)
                || (97...102).contains($0)
        }) else {
            throw OverlayTokenError.invalidToken
        }

        return String(decoding: bytes, as: UTF8.self)
    }

    private static func fail(_ message: String) -> Never {
        FileHandle.standardError.write(
            Data("[FATAL] \(message)\n".utf8)
        )
        Darwin.exit(EXIT_FAILURE)
    }
}
