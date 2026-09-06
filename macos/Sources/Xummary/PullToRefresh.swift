import AppKit
import SwiftUI

/// How far the briefing has to come down before letting go refreshes it.
let pullTrigger: CGFloat = 52

/// Watches the scroll view behind the briefing: how far it has been pulled
/// past the top, and when the trackpad is released.
///
/// SwiftUI's `refreshable` draws nothing on macOS, so the gesture is read from
/// AppKit. Put this in the background of the scroll view's content.
struct PullToRefresh: NSViewRepresentable {
    /// The stretch past the top, and whether fingers are still on the trackpad.
    let onPull: @MainActor (CGFloat, Bool) -> Void
    let onRelease: @MainActor () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onPull: onPull, onRelease: onRelease)
    }

    func makeNSView(context: Context) -> NSView {
        let probe = Probe()
        let coordinator = context.coordinator
        probe.onMove = { scrollView in coordinator.attach(scrollView) }
        return probe
    }

    func updateNSView(_ view: NSView, context: Context) {
        context.coordinator.onPull = onPull
        context.coordinator.onRelease = onRelease
        context.coordinator.attach(view.enclosingScrollView)
    }

    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        coordinator.detach()
    }

    /// An invisible view whose only job is to find the scroll view it landed in.
    final class Probe: NSView {
        var onMove: ((NSScrollView?) -> Void)?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            onMove?(enclosingScrollView)
        }
    }

    @MainActor
    final class Coordinator {
        var onPull: @MainActor (CGFloat, Bool) -> Void
        var onRelease: @MainActor () -> Void
        private weak var scrollView: NSScrollView?
        private var bounds: NSObjectProtocol?
        private var wheel: Any?
        /// Fingers on the trackpad. Momentum after a flick is not a pull: it
        /// would refresh every time you threw the briefing back to the top.
        private var dragging = false

        init(onPull: @escaping @MainActor (CGFloat, Bool) -> Void,
             onRelease: @escaping @MainActor () -> Void) {
            self.onPull = onPull
            self.onRelease = onRelease
        }

        func attach(_ scrollView: NSScrollView?) {
            guard let scrollView, scrollView !== self.scrollView else { return }
            detach()
            self.scrollView = scrollView

            // The rubber band only stretches when the content overflows, and a
            // short briefing does not. Allow it always, or the gesture goes
            // missing exactly when there is least to read.
            scrollView.verticalScrollElasticity = .allowed

            let clip = scrollView.contentView
            clip.postsBoundsChangedNotifications = true
            bounds = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification,
                object: clip,
                queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.report() }
            }

            // The phase says where the fingers are. A legacy mouse wheel sends
            // none of it, so it cannot pull to refresh.
            wheel = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
                let phase = event.phase
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if phase.contains(.began) || phase.contains(.changed) {
                        self.dragging = true
                    }
                    if phase.contains(.ended) || phase.contains(.cancelled) {
                        self.dragging = false
                        self.onRelease()
                    }
                }
                return event
            }
        }

        func detach() {
            if let bounds { NotificationCenter.default.removeObserver(bounds) }
            if let wheel { NSEvent.removeMonitor(wheel) }
            bounds = nil
            wheel = nil
            scrollView = nil
        }

        /// How far past the top the content has been dragged, in points.
        private func report() {
            guard let scrollView, let clip = scrollView.contentView as NSClipView? else { return }
            let y = clip.bounds.origin.y
            let stretch: CGFloat
            if clip.isFlipped {
                stretch = -y
            } else {
                let top = (scrollView.documentView?.bounds.height ?? 0) - clip.bounds.height
                stretch = y - max(0, top)
            }
            onPull(max(0, stretch), dragging)
        }
    }
}

/// Says what the pull will do, so the threshold is visible rather than guessed.
struct PullHint: View {
    let pull: CGFloat
    let armed: Bool
    let theme: Theme

    private var progress: Double { min(1, Double(pull / pullTrigger)) }

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: "arrow.clockwise")
                .font(.system(size: 11, weight: .medium))
                .rotationEffect(.degrees(progress * 180))
            Text(armed ? "release to refresh" : "pull to refresh")
                .font(.system(size: 11, weight: .medium))
        }
        .foregroundStyle(armed ? theme.accent : theme.dim)
        // Visible from the first few points, so the gesture announces itself.
        .opacity(min(1, Double(pull / 12)))
        .animation(.easeOut(duration: 0.15), value: armed)
    }
}
