import Foundation

enum OverlayManagerError: Error, Equatable {
    case duplicateID
    case windowNotFound
}

@MainActor
final class OverlayWindowManager {
    private var windows: [String: OverlayWindow] = [:]

    @discardableResult
    func apply(_ command: OverlayCommand) throws -> OverlayWindowSnapshot? {
        // Every command is validated before state mutation.
        try command.validate()

        switch command {
        case let .register(id, kind, width, height):
            guard windows[id] == nil else {
                throw OverlayManagerError.duplicateID
            }

            let window = OverlayWindow(
                id: id,
                kind: kind,
                width: width,
                height: height
            )

            windows[id] = window
            return window.snapshot

        case let .close(id):
            guard windows.removeValue(forKey: id) != nil else {
                throw OverlayManagerError.windowNotFound
            }

            return nil

        case let .setMousePassthrough(id, enabled):
            guard var window = windows[id] else {
                throw OverlayManagerError.windowNotFound
            }

            window.ignoresMouseEvents = enabled
            windows[id] = window
            return window.snapshot
        }
    }

    func snapshot() -> [OverlayWindowSnapshot] {
        windows.values
            .map(\.snapshot)
            .sorted { $0.id < $1.id }
    }

    var windowCount: Int {
        windows.count
    }
}
