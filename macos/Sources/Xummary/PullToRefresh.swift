import AppKit
import SwiftUI

/// How far the briefing has to come down before letting go refreshes it.
let pullTrigger: CGFloat = 70

/// Watches the scroll view behind the briefing: how far it has been pulled
/// past the top, and when the trackpad is released.
///
/// SwiftUI's `refreshable` draws nothing on macOS, so the gesture is read from
/// AppKit. Put this in the background of the scroll view's content.
struct PullToRefresh: NSViewRepresentable {
    let onPull: @MainActor (CGFloat) -> Void
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
        var onPull: @MainActor (CGFloat) -> Void
        var onRelease: @MainActor () -> Void
        private weak var scrollView: NSScrollView?
        private var bounds: NSObjectProtocol?
        private var wheel: Any?

        init(onPull: @escaping @MainActor (CGFloat) -> Void,
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

            // Fingers leaving the trackpad, which is the moment to fire. A
            // legacy mouse wheel sends no phase, so it cannot pull to refresh.
            wheel = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
                let phase = event.phase
                if phase.contains(.ended) || phase.contains(.cancelled) {
                    MainActor.assumeIsolated { self?.onRelease() }
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
            onPull(max(0, stretch))
        }
    }
}

/// Shows how far you have pulled, and turns solid once letting go will refresh.
struct PullHint: View {
    let pull: CGFloat
    let armed: Bool
    let theme: Theme

    private var progress: Double { min(1, Double(pull / pullTrigger)) }

    var body: some View {
        Image(systemName: "arrow.clockwise")
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(armed ? theme.accent : theme.dim)
            .rotationEffect(.degrees(progress * 300))
            .scaleEffect(0.75 + progress * 0.25)
            .opacity(progress)
            .animation(.easeOut(duration: 0.15), value: armed)
    }
}
