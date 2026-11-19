#if OVERLAY_UDS_SELF_TEST

import Foundation
import Darwin

private func udsCheck(_ name: String, _ condition: Bool) {
    guard condition else {
        fatalError("[FAIL] \(name)")
    }
    print("[PASS] \(name)")
}

private func udsSendAll(_ fd: Int32, _ data: Data) -> Bool {
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

        guard sent > 0 else { return false }
        offset += sent
    }

    return true
}

private func udsReadExactly(_ fd: Int32, _ count: Int) -> Data? {
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

        guard received > 0 else { return nil }
        result.append(contentsOf: buffer.prefix(received))
    }

    return result
}

private func udsConnect(_ path: String) -> Int32 {
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

    let result = withUnsafePointer(to: &address) {
        pointer in
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
struct OverlayUDSSelfTest {

    static func main() throws {
        let path = "/tmp/jvm-overlay-uds-\(UUID().uuidString).sock"
        let payload = Data("overlay-uds-roundtrip-test".utf8)

        let server = OverlayUDSServer(path: path) { data in
            data
        }

        try server.start()
        defer { server.stop() }

        print("[INFO] UDS server started")

        // Socket file permissions.
        var info = stat()
        let statResult = lstat(path, &info)

        udsCheck(
            "Socket file created",
            statResult == 0 && (info.st_mode & S_IFMT) == S_IFSOCK
        )

        udsCheck(
            "Socket permissions are 0600",
            statResult == 0 && (info.st_mode & 0o777) == 0o600
        )

        // Duplicate path must be rejected without disturbing the owner.
        let duplicate = OverlayUDSServer(path: path) { data in data }

        do {
            try duplicate.start()
            fatalError("[FAIL] Duplicate socket path was accepted")
        } catch OverlayUDSError.pathAlreadyExists {
            print("[PASS] Duplicate socket path rejected")
        }

        // Connect as the current local user.
        let client = udsConnect(path)
        udsCheck("Local client connected", client >= 0)

        guard client >= 0 else {
            throw NSError(
                domain: "OverlayUDSTest",
                code: 1,
                userInfo: [NSLocalizedDescriptionKey: "UDS connect failed"]
            )
        }

        defer { Darwin.close(client) }

        // Send 4-byte big-endian length + payload.
        let length = UInt32(payload.count)

        let header = Data([
            UInt8((length >> 24) & 0xff),
            UInt8((length >> 16) & 0xff),
            UInt8((length >> 8) & 0xff),
            UInt8(length & 0xff)
        ])

        udsCheck(
            "Request frame sent",
            udsSendAll(client, header) && udsSendAll(client, payload)
        )

        // Read response frame.
        guard let responseHeader = udsReadExactly(client, 4) else {
            fatalError("[FAIL] Response header missing")
        }

        let b = [UInt8](responseHeader)
        let responseLength =
            (UInt32(b[0]) << 24)
            | (UInt32(b[1]) << 16)
            | (UInt32(b[2]) << 8)
            | UInt32(b[3])

        guard
            responseLength > 0,
            responseLength <= OverlayProtocol.maximumMessageSize,
            let response = udsReadExactly(client, Int(responseLength))
        else {
            fatalError("[FAIL] Invalid or incomplete response")
        }

        udsCheck(
            "UDS request-response roundtrip",
            response == payload
        )

        // Stop must remove only the socket owned by this server.
        server.stop()

        var afterStop = stat()
        let afterStopResult = lstat(path, &afterStop)

        udsCheck(
            "Owned socket removed after stop",
            afterStopResult != 0 && errno == ENOENT
        )

        print("")
        print("[RESULT] All UDS transport tests passed")
    }
}

#endif
