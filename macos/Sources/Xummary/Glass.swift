import AppKit
import SwiftUI

/// The system blur behind the whole window. macOS 26 has Liquid Glass proper;
/// on 15 the same effect comes from `NSVisualEffectView` sampling what is
/// behind the window, which is what the glass look is built on.
struct Glass: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .underWindowBackground

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = .behindWindow
        // `.active` keeps the blur alive when the window loses focus; the
        // default dulls it to grey the moment you click elsewhere.
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {
        view.material = material
    }
}

/// Makes the window itself transparent. Without this the blur has nothing to
/// show through: SwiftUI paints an opaque window background over it.
struct TransparentWindow: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let probe = NSView()
        // The view has no window until it is in the hierarchy.
        DispatchQueue.main.async {
            guard let window = probe.window else { return }
            window.isOpaque = false
            window.backgroundColor = .clear
            window.titlebarAppearsTransparent = true
        }
        return probe
    }

    func updateNSView(_ view: NSView, context: Context) {}
}
