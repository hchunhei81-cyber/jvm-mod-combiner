#if OVERLAY_UDS_SATURATED_STOP_SELF_TEST

import Foundation
import Darwin

private final class SaturatedTestState: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var descriptors: [Int32] = []

    func recordHandler() {
        lock.lock()
        count += 1
        lock.unlock()
    }

    func handlerCount() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func addClient(_ fd: Int32) {
        lock.lock()
        descriptors.append(fd)
        lock.unlock()
    }

    func takeClients() -> [Int32] {
        lock.lock()
        defer { lock.unlock() }
        let result = descriptors
        descriptors.removeAll()
        return result
    }
}

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
    guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) - 1 else {
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

private func sendAll(_ fd: Int32, _ bytes: [UInt8]) -> Bool {
    var offset = 0

    while offset < bytes.count {
        let result = bytes.withUnsafeBytes { rawBuffer -> Int in
            guard let base = rawBuffer.baseAddress else { return -1 }
            return Darwin.send(
                fd,
                base.advanced(by: offset),
                bytes.count - offset,
                0
            )
        }

        if result > 0 {
            offset += result
        } else if result < 0 && errno == EINTR {
            continue
        } else {
            return false
        }
    }

    return true
}

@main
struct OverlayUDSSaturatedStopSelfTest {
    static func main() throws {
        let path = "/tmp/jvm-overlay-saturated-\(UUID().uuidString).sock"
        let workerLimit = 8

        let entered = DispatchSemaphore(value: 0)
        let state = SaturatedTestState()

        let server = OverlayUDSServer(path: path) { data in
            state.recordHandler()
            entered.signal()

            // Keep all workers occupied deliberately.
            Thread.sleep(forTimeInterval: 5)
            return data
        }

        try server.start()
        print("[INFO] Server started; opening \(workerLimit) clients")

        for index in 0..<workerLimit {
            DispatchQueue.global().async {
                let fd = connectClient(path: path)
                guard fd >= 0 else {
                    print("[DIAG] Client \(index) failed to connect")
                    return
                }

                state.addClient(fd)

                // Four-byte big-endian length = 1, followed by one byte.
                _ = sendAll(fd, [0, 0, 0, 1, UInt8(index)])

                var response: UInt8 = 0
                _ = Darwin.recv(fd, &response, 1, 0)
            }
        }

        var allWorkersEntered = true

        for _ in 0..<workerLimit {
            if entered.wait(timeout: .now() + .seconds(8)) != .success {
                allWorkersEntered = false
                break
            }
        }

        let observedHandlers = state.handlerCount()

        check(
            "All 8 handlers entered simultaneously",
            allWorkersEntered && observedHandlers == workerLimit
        )

        let start = DispatchTime.now()
        server.stop()

        let elapsed = Double(
            DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds
        ) / 1_000_000_000

        print(String(format: "[INFO] stop() returned in %.3f seconds", elapsed))

        check(
            "stop() returns within 1 second under full worker load",
            elapsed < 1.0
        )

        for fd in state.takeClients() {
            Darwin.close(fd)
        }

        // Let the deliberately sleeping handlers finish.
        Thread.sleep(forTimeInterval: 5.5)

        var info = stat()
        let result = lstat(path, &info)

        check(
            "Socket removed after saturated shutdown",
            result != 0 && errno == ENOENT
        )

        print("[RESULT] Saturated stop test passed")
    }
}

#endif
