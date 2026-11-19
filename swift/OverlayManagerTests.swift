#if OVERLAY_SELF_TEST
import Foundation

@main
@MainActor
struct OverlayManagerSelfTest {
    static func main() throws {
        let manager = OverlayWindowManager()

        // Registration.
        let first = try manager.apply(
            .register(
                id: "fabric-main",
                kind: .status,
                width: 640,
                height: 360
            )
        )

        precondition(first?.id == "fabric-main")
        precondition(manager.windowCount == 1)

        // Duplicate IDs must fail closed.
        do {
            _ = try manager.apply(
                .register(
                    id: "fabric-main",
                    kind: .control,
                    width: 320,
                    height: 200
                )
            )
            fatalError("Duplicate registration was accepted")
        } catch OverlayManagerError.duplicateID {
            print("[PASS] Duplicate ID rejected")
        }

        // Mouse passthrough state.
        let updated = try manager.apply(
            .setMousePassthrough(id: "fabric-main", enabled: true)
        )

        precondition(updated?.ignoresMouseEvents == true)
        print("[PASS] Mouse passthrough state updated")

        // Invalid dimensions must be rejected before mutation.
        do {
            _ = try manager.apply(
                .register(
                    id: "invalid-size",
                    kind: .diagnostic,
                    width: .infinity,
                    height: 200
                )
            )
            fatalError("Invalid dimensions were accepted")
        } catch OverlayCommandError.invalidDimensions {
            print("[PASS] Invalid dimensions rejected")
        }

        // Unknown window operations must fail.
        do {
            _ = try manager.apply(.close(id: "missing-window"))
            fatalError("Unknown window close was accepted")
        } catch OverlayManagerError.windowNotFound {
            print("[PASS] Unknown window rejected")
        }

        // Close and lifecycle cleanup.
        _ = try manager.apply(.close(id: "fabric-main"))
        precondition(manager.windowCount == 0)
        precondition(manager.snapshot().isEmpty)

        print("[PASS] Window close and registry cleanup")
        print("[RESULT] All Overlay Manager tests passed")
    }
}
#endif
