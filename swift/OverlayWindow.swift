import Foundation

struct OverlayWindow: Sendable {
    let id: String
    let kind: OverlayWindowKind
    let width: Double
    let height: Double
    var ignoresMouseEvents: Bool

    init(
        id: String,
        kind: OverlayWindowKind,
        width: Double,
        height: Double
    ) {
        self.id = id
        self.kind = kind
        self.width = width
        self.height = height

        // Safe default: input is not silently discarded.
        self.ignoresMouseEvents = false
    }
}

struct OverlayWindowSnapshot: Codable, Sendable {
    let id: String
    let kind: OverlayWindowKind
    let width: Double
    let height: Double
    let ignoresMouseEvents: Bool
}

extension OverlayWindow {
    var snapshot: OverlayWindowSnapshot {
        OverlayWindowSnapshot(
            id: id,
            kind: kind,
            width: width,
            height: height,
            ignoresMouseEvents: ignoresMouseEvents
        )
    }
}
