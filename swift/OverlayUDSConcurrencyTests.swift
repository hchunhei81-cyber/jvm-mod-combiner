#if OVERLAY_UDS_CONCURRENCY_SELF_TEST

import Foundation
import Darwin

private final class UDSProbe: @unchecked Sendable {
    private let lock = NSLock()
    private var active = 0
    private var peak = 0

    func begin() {
        lock.lock()
        active += 1
        peak = max(peak, active)
        lock.unlock()
    }

    func end() {
        lock.lock()
        active -= 1
        lock.unlock()
    }

    var maximum: Int {
        lock.lock()
        defer { lock.unlock() }
        return peak
    }
}

private final class UDSResults: @unchecked Sendable {
    private let lock = NSLock()
    private var successes = 0
    private var failures: [Int] = []

    func record(_ success: Bool, index: Int) {
        lock.lock()

        if success {
            successes += 1
        } else {
            failures.append(index)
        }

        let total = successes
        lock.unlock()

        if !success {
            print("[DIAG] Client \(index) failed; successes so far: \(total)")
        }
    }

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return successes
    }

    var failedClients: [Int] {
        lock.lock()
        defer { lock.unlock() }
        return failures.sorted()
    }
}

private func check(_ name: String, _ condition: Bool) {
    guard condition else {
        fatalError("[FAIL] \(name)")
    }
    print("[PASS] \(name)")
}

private func sendAll(_ fd: Int32, _ data: Data) -> Bool {
    var offset = 0

    while offset < data.count {
        let sent = data.withUnsafeBytes {
            Darwin.send(
                fd,
                $0.baseAddress!.advanced(by: offset),
                data.count - offset,
                0
            )
        }

        if sent < 0 && errno == EINTR {
            continue
        }

        guard sent > 0 else { return false }
        offset += sent
    }

    return true
}

private func readExactly(_ fd: Int32, _ count: Int) -> Data? {
    var result = Data()
    result.reserveCapacity(count)

    while result.count < count {
        var buffer = [UInt8](
            repeating: 0,
            count: count - result.count
        )

        let received = buffer.withUnsafeMutableBytes {
            Darwin.recv(fd, $0.baseAddress, $0.count, 0)
        }

        if received < 0 && errno == EINTR {
            continue
        }

        guard received > 0 else { return nil }
        result.append(contentsOf: buffer.prefix(received))
    }

    return result
}

private func roundTrip(path: String, index: Int) -> Bool {
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { Darwin.close(fd) }

    var timeout = timeval(tv_sec: 10, tv_usec: 0)
    _ = setsockopt(
        fd,
        SOL_SOCKET,
        SO_RCVTIMEO,
        &timeout,
        socklen_t(MemoryLayout<timeval>.size)
    )

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)

    let pathBytes = Array(path.utf8)

    guard pathBytes.count < MemoryLayout.size(
        ofValue: address.sun_path
    ) - 1 else {
        return false
    }

    withUnsafeMutableBytes(of: &address.sun_path) { buffer in
        for (position, byte) in pathBytes.enumerated() {
            buffer[position] = byte
        }
        buffer[pathBytes.count] = 0
    }

    let connected = withUnsafePointer(to: &address) {
        pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(
                fd,
                $0,
                socklen_t(MemoryLayout<sockaddr_un>.size)
            )
        }
    }

    guard connected == 0 else { return false }

    let payload = Data("parallel-client-\(index)".utf8)
    let size = UInt32(payload.count)

    let header = Data([
        UInt8((size >> 24) & 0xff),
        UInt8((size >> 16) & 0xff),
        UInt8((size >> 8) & 0xff),
        UInt8(size & 0xff)
    ])

    guard
        sendAll(fd, header),
        sendAll(fd, payload),
        let responseHeader = readExactly(fd, 4)
    else {
        return false
    }

    let bytes = [UInt8](responseHeader)
    let responseSize =
        (UInt32(bytes[0]) << 24)
        | (UInt32(bytes[1]) << 16)
        | (UInt32(bytes[2]) << 8)
        | UInt32(bytes[3])

    guard
        responseSize > 0,
        responseSize <= OverlayProtocol.maximumMessageSize,
        let response = readExactly(fd, Int(responseSize))
    else {
        return false
    }

    return response == payload
}

@main
struct OverlayUDSConcurrencySelfTest {
    static func main() throws {
        let path = "/tmp/jvm-overlay-concurrency-\(UUID().uuidString).sock"
        let probe = UDSProbe()
        let results = UDSResults()

        let server = OverlayUDSServer(path: path) { data in
            probe.begin()
            defer { probe.end() }

            // Deliberately slow handler to expose serial processing.
            Thread.sleep(forTimeInterval: 0.25)
            return data
        }

        try server.start()
        defer { server.stop() }

        let clientCount = 12

        DispatchQueue.concurrentPerform(iterations: clientCount) {
            index in
            results.record(
                roundTrip(path: path, index: index),
                index: index
            )
        }

        check(
            "All \(clientCount) concurrent clients completed",
            results.count == clientCount
        )

        check(
            "Multiple clients processed concurrently",
            probe.maximum >= 2
        )

        check(
            "Worker concurrency stayed within 8-client limit",
            probe.maximum <= 8
        )

        server.stop()

        var info = stat()
        let result = lstat(path, &info)

        check(
            "Socket removed after concurrent shutdown",
            result != 0 && errno == ENOENT
        )

        print("")
        print("[INFO] Peak simultaneous handlers: \(probe.maximum)")
        print("[RESULT] UDS concurrency tests passed")
    }
}

#endif
