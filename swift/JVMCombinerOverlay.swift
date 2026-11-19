import AppKit

@main
@MainActor
struct JVMCombinerOverlay {
    static func main() {
        // Bootstrap only. No NSWindow is created or displayed here.
        let manager = OverlayWindowManager()

        print("[OK] JVM Combiner Swift backend initialized")
        print("[OK] Overlay window manager ready")
        print("[INFO] Registered windows: \(manager.windowCount)")
        print("[SAFE] No native window operations were performed")
    }
}
