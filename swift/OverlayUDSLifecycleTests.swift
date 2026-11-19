#if OVERLAY_UDS_LIFECYCLE_SELF_TEST

import Foundation
import Darwin

private func check(_ name: String, _ condition: Bool) {
    guard condition else {
        fatalError("[FAIL] \(name)")
    }
    print("[PASS] \(name)")
}

private func connectClient(path: String) -> Int32 {
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return -1 }

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)

    let bytes = Array(path.utf8)
    guard bytes.count < MemoryLayout.size(
        ofValue: address.sun_path
    ) - 1 else {
        Darwin.close(fd)
        return -1
    }

    withUnsafeMutableBytes(of: &address.sun_path) { buffer in
        for (index, byte) in bytes.enumerated() {
            buffer[index] = byte
        }
        buffer[bytes.count] = 0
    }

    let result = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(
                fd,
                $0,
                socklen_t(MemoryLayout<sockaddr_un>.size)
            )
        }
    }

    guard result == 0 else {
        Darwin.close(fd)
        return -1
    }

    return fd
}

@main
struct OverlayUDSLifecycleSelfTest {
    static func main() throws {
        let path = "/tmp/jvm-overlay-lifecycle-\(UUID().uuidString).sock"

        let server = OverlayUDSServer(path: path) { data in
            data
        }

        let iterations = 50
        var successfulConnections = 0

        print("[INFO] Starting \(iterations) rapid start/stop cycles")

        for index in 1...iterations {
            try server.start()

            let client = connectClient(path: path)
            if client >= 0 {
                successfulConnections += 1
            }

            // Stop while a connection may still be pending or active.
            server.stop()

            if client >= 0 {
                Darwin.close(client)
            }

            var info = stat()
            let result = lstat(path, &info)

            check(
                "Cycle \(index): owned socket removed",
                result != 0 && errno == ENOENT
            )
        }

        check(
            "All \(iterations) start/stop cycles completed",
            successfulConnections == iterations
        )

        // Verify the same server instance remains restartable.
        try server.start()
        let finalClient = connectClient(path: path)
        check("Server accepts connection after repeated restarts", finalClient >= 0)

        server.stop()

        if finalClient >= 0 {
            Darwin.close(finalClient)
        }

        var finalInfo = stat()
        let finalResult = lstat(path, &finalInfo)

        check(
            "Final restart socket cleaned up",
            finalResult != 0 && errno == ENOENT
        )

        print("[RESULT] UDS lifecycle tests passed")
    }
}

#endif
