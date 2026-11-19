
import Foundation

@MainActor
final class OverlayRequestHandler {
    private let ipc: OverlayIPC
    private let manager: OverlayWindowManager
    private let token: String

    init(
        ipc: OverlayIPC,
        manager: OverlayWindowManager,
        token: String
    ) {
        self.ipc = ipc
        self.manager = manager
        self.token = token
    }

    func handle(_ data: Data) -> Data {
        ipc.handle(
            data,
            expectedToken: token,
            manager: manager
        )
    }

    // Called on MainActor during server initialization.
    // The returned closure is safe to invoke from UDS workers.
    static func makeTransportHandler(
        _ requestHandler: OverlayRequestHandler
    ) -> @Sendable (Data) -> Data {
        return { data in
            let gate = OverlayRequestGate()

            let task = Task { @MainActor in
                guard gate.begin() else {
                    return
                }

                let response = requestHandler.handle(data)
                gate.complete(response)
            }

            // UDS socket timeout is 5 seconds.
            // Leave a margin so the worker can return before socket I/O expires.
            guard gate.wait(timeout: .now() + .seconds(4)) else {
                task.cancel()
                gate.cancel()
                return Data()
            }

            return gate.result() ?? Data()
        }
    }
}

private final class OverlayRequestGate: @unchecked Sendable {
    private enum State {
        case pending
        case running
        case completed
        case cancelled
    }

    private let lock = NSLock()
    private let semaphore = DispatchSemaphore(value: 0)

    private var state: State = .pending
    private var response: Data?

    func begin() -> Bool {
        lock.lock()
        defer { lock.unlock() }

        guard case .pending = state else {
            return false
        }

        state = .running
        return true
    }

    func complete(_ data: Data) {
        lock.lock()

        guard case .running = state else {
            lock.unlock()
            return
        }

        response = data
        state = .completed

        lock.unlock()
        semaphore.signal()
    }

    func cancel() {
        lock.lock()
        defer { lock.unlock() }

        switch state {
        case .pending, .running:
            state = .cancelled
            response = nil

        case .completed, .cancelled:
            break
        }
    }

    func wait(timeout: DispatchTime) -> Bool {
        semaphore.wait(timeout: timeout) == .success
    }

    func result() -> Data? {
        lock.lock()
        defer { lock.unlock() }

        guard case .completed = state else {
            return nil
        }

        return response
    }
}
