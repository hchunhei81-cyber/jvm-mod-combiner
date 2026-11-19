import Foundation
import Darwin

enum OverlayUDSError: Error {
    case invalidPath
    case pathAlreadyExists
    case socketCreation
    case bindFailed
    case permissionFailed
    case listenFailed
    case alreadyRunning
}

final class OverlayUDSServer: @unchecked Sendable {

    private static let maximumClients = 8

    private let path: String
    private let expectedUID: uid_t
    private let handler: @Sendable (Data) -> Data

    private let acceptQueue = DispatchQueue(
        label: "jvm.combiner.overlay.uds.accept"
    )

    private let workerQueue = DispatchQueue(
        label: "jvm.combiner.overlay.uds.workers",
        attributes: .concurrent
    )

    private let clientSlots = DispatchSemaphore(
        value: maximumClients
    )

    private let lock = NSLock()

    // All mutable state below is protected by lock.
    private var listenerFD: Int32 = -1
    private var running = false
    private var generation: UInt64 = 0
    private var activeClients = Set<Int32>()

    private var ownedDevice: dev_t = 0
    private var ownedInode: ino_t = 0

    init(
        path: String,
        expectedUID: uid_t = getuid(),
        handler: @escaping @Sendable (Data) -> Data
    ) {
        self.path = path
        self.expectedUID = expectedUID
        self.handler = handler
    }

    func start() throws {
        lock.lock()
        defer { lock.unlock() }

        guard !running else {
            throw OverlayUDSError.alreadyRunning
        }

        let bytes = Array(path.utf8)

        guard
            !bytes.isEmpty,
            bytes.count < MemoryLayout.size(
                ofValue: sockaddr_un().sun_path
            ) - 1
        else {
            throw OverlayUDSError.invalidPath
        }

        var existing = stat()

        if lstat(path, &existing) == 0 {
            throw OverlayUDSError.pathAlreadyExists
        }

        guard errno == ENOENT else {
            throw OverlayUDSError.invalidPath
        }

        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)

        guard fd >= 0 else {
            throw OverlayUDSError.socketCreation
        }

        let socketFlags = fcntl(fd, F_GETFL)
        guard socketFlags >= 0,
              fcntl(fd, F_SETFL, socketFlags | O_NONBLOCK) == 0 else {
            Darwin.close(fd)
            throw OverlayUDSError.socketCreation
        }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)

        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            for (index, byte) in bytes.enumerated() {
                buffer[index] = byte
            }
            buffer[bytes.count] = 0
        }

        let bindResult = withUnsafePointer(to: &address) {
            pointer in
            pointer.withMemoryRebound(
                to: sockaddr.self,
                capacity: 1
            ) {
                Darwin.bind(
                    fd,
                    $0,
                    socklen_t(MemoryLayout<sockaddr_un>.size)
                )
            }
        }

        guard bindResult == 0 else {
            Darwin.close(fd)
            throw OverlayUDSError.bindFailed
        }

        var socketInfo = stat()

        guard
            lstat(path, &socketInfo) == 0,
            (socketInfo.st_mode & S_IFMT) == S_IFSOCK
        else {
            Darwin.close(fd)
            throw OverlayUDSError.invalidPath
        }

        ownedDevice = socketInfo.st_dev
        ownedInode = socketInfo.st_ino

        guard chmod(path, mode_t(S_IRUSR | S_IWUSR)) == 0 else {
            Darwin.close(fd)
            removeOwnedSocketLocked()
            throw OverlayUDSError.permissionFailed
        }

        guard // Pending connection queue; worker concurrency remains capped at 8.
        Darwin.listen(fd, 64) == 0 else {
            Darwin.close(fd)
            removeOwnedSocketLocked()
            throw OverlayUDSError.listenFailed
        }

        generation &+= 1
        let currentGeneration = generation

        listenerFD = fd
        running = true

        acceptQueue.async { [self] in
            acceptLoop(fd, generation: currentGeneration)
        }
    }

    func stop() {
        lock.lock()

        guard running || listenerFD >= 0 || ownedInode != 0 else {
            lock.unlock()
            return
        }

        running = false
        generation &+= 1

        let fd = listenerFD
        listenerFD = -1

        // Shutdown while holding the lock. Worker cleanup also needs
        // this lock, so these descriptors cannot be closed and reused
        // before shutdown reaches them.
        for client in activeClients {
            _ = Darwin.shutdown(client, SHUT_RDWR)
        }

        if fd >= 0 {
            _ = Darwin.shutdown(fd, SHUT_RDWR)
            _ = Darwin.close(fd)
        }

        removeOwnedSocketLocked()

        lock.unlock()
    }

    private func isCurrent(
        fd: Int32,
        generation expectedGeneration: UInt64
    ) -> Bool {
        lock.lock()
        defer { lock.unlock() }

        return running
            && listenerFD == fd
            && generation == expectedGeneration
    }

    private func registerClient(
        _ client: Int32,
        listener: Int32,
        generation expectedGeneration: UInt64
    ) -> Bool {
        lock.lock()
        defer { lock.unlock() }

        guard
            running,
            listenerFD == listener,
            generation == expectedGeneration
        else {
            return false
        }

        activeClients.insert(client)
        return true
    }

    private func finishClient(_ client: Int32) {
        lock.lock()
        activeClients.remove(client)
        _ = Darwin.close(client)
        lock.unlock()

        clientSlots.signal()
    }

    private func acceptLoop(
        _ fd: Int32,
        generation expectedGeneration: UInt64
    ) {
        while isCurrent(fd: fd, generation: expectedGeneration) {
            // Acquire capacity before accepting. This prevents an accepted
            // client from occupying the accept loop while waiting for a slot.
            guard clientSlots.wait(
                timeout: .now() + .milliseconds(100)
            ) == .success else {
                continue
            }

            guard isCurrent(
                fd: fd,
                generation: expectedGeneration
            ) else {
                clientSlots.signal()
                break
            }

            // acceptNonblocking() holds the lifecycle lock across the
            // nonblocking accept syscall. stop() cannot close/reuse this
            // listener descriptor during that syscall.
            let result = acceptNonblocking(
                fd,
                generation: expectedGeneration
            )

            switch result {
            case .stopped:
                clientSlots.signal()
                return

            case .failed(let code):
                clientSlots.signal()

                if code == EINTR {
                    continue
                }

                if code == EAGAIN || code == EWOULDBLOCK {
                    Thread.sleep(forTimeInterval: 0.01)
                    continue
                }

                if !isCurrent(
                    fd: fd,
                    generation: expectedGeneration
                ) {
                    return
                }

                Thread.sleep(forTimeInterval: 0.01)

            case .accepted(let client):
                guard registerClient(
                    client,
                    listener: fd,
                    generation: expectedGeneration
                ) else {
                    Darwin.close(client)
                    clientSlots.signal()
                    return
                }

                workerQueue.async { [self] in
                    defer {
                        finishClient(client)
                    }

                    handleClient(client)
                }
            }
        }
    }

    private enum AcceptResult {
        case stopped
        case failed(Int32)
        case accepted(Int32)
    }

    private func acceptNonblocking(
        _ fd: Int32,
        generation expectedGeneration: UInt64
    ) -> AcceptResult {
        lock.lock()
        defer { lock.unlock() }

        guard running,
              listenerFD == fd,
              generation == expectedGeneration else {
            return .stopped
        }

        let client = Darwin.accept(fd, nil, nil)

        if client < 0 {
            return .failed(errno)
        }

        return .accepted(client)
    }

    private func handleClient(_ fd: Int32) {
        var noSignal: Int32 = 1

        _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &noSignal,
            socklen_t(MemoryLayout<Int32>.size)
        )

        var timeout = timeval(tv_sec: 5, tv_usec: 0)

        _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        )

        _ = setsockopt(
            fd,
            SOL_SOCKET,
            SO_SNDTIMEO,
            &timeout,
            socklen_t(MemoryLayout<timeval>.size)
        )

        var peerUID: uid_t = 0
        var peerGID: gid_t = 0

        guard
            getpeereid(fd, &peerUID, &peerGID) == 0,
            peerUID == expectedUID
        else {
            return
        }

        guard let header = readExactly(fd, count: 4) else {
            return
        }

        let bytes = [UInt8](header)

        let length =
            (UInt32(bytes[0]) << 24)
            | (UInt32(bytes[1]) << 16)
            | (UInt32(bytes[2]) << 8)
            | UInt32(bytes[3])

        guard
            length > 0,
            length <= OverlayProtocol.maximumMessageSize,
            let request = readExactly(fd, count: Int(length))
        else {
            return
        }

        let response = handler(request)

        guard
            !response.isEmpty,
            response.count <= OverlayProtocol.maximumMessageSize
        else {
            return
        }

        let size = UInt32(response.count)

        let responseHeader = Data([
            UInt8((size >> 24) & 0xff),
            UInt8((size >> 16) & 0xff),
            UInt8((size >> 8) & 0xff),
            UInt8(size & 0xff)
        ])

        guard
            writeAll(fd, data: responseHeader),
            writeAll(fd, data: response)
        else {
            return
        }
    }

    private func readExactly(
        _ fd: Int32,
        count: Int
    ) -> Data? {
        var result = Data()
        result.reserveCapacity(count)

        while result.count < count {
            var buffer = [UInt8](
                repeating: 0,
                count: count - result.count
            )

            let received = buffer.withUnsafeMutableBytes {
                Darwin.recv(
                    fd,
                    $0.baseAddress,
                    $0.count,
                    0
                )
            }

            if received < 0 && errno == EINTR {
                continue
            }

            guard received > 0 else {
                return nil
            }

            result.append(contentsOf: buffer.prefix(received))
        }

        return result
    }

    private func writeAll(
        _ fd: Int32,
        data: Data
    ) -> Bool {
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

            guard sent > 0 else {
                return false
            }

            offset += sent
        }

        return true
    }

    // Caller must hold lock.
    private func removeOwnedSocketLocked() {
        guard ownedInode != 0 else {
            return
        }

        var info = stat()

        if lstat(path, &info) == 0,
           (info.st_mode & S_IFMT) == S_IFSOCK,
           info.st_dev == ownedDevice,
           info.st_ino == ownedInode {
            _ = unlink(path)
        }

        ownedInode = 0
        ownedDevice = 0
    }

    deinit {
        stop()
    }
}
