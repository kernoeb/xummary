import SwiftUI

struct ContentView: View {
    @StateObject private var model = BriefingModel()
    @Environment(\.colorScheme) private var scheme

    private var theme: Theme { .of(scheme) }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(theme.rule)
            content
            footer
        }
        // Blur behind everything, with a thin wash of the theme colour on top
        // so text keeps its contrast over a bright wallpaper.
        .background {
            Glass()
                .overlay(theme.background.opacity(theme.glassWash))
                .ignoresSafeArea()
        }
        .background(TransparentWindow())
        .onAppear {
            model.loadHistory()
            model.run()
        }
        .onReceive(NotificationCenter.default.publisher(for: .refreshBriefing)) { _ in
            model.run()
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("Xummary")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(theme.text)

            Spacer()

            Picker("", selection: $model.hours) {
                Text("12 h").tag(12)
                Text("24 h").tag(24)
                Text("48 h").tag(48)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 190)
            .onChange(of: model.hours) { _, _ in model.run() }

            Button(action: { model.run() }) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 12, weight: .medium))
            }
            .buttonStyle(.plain)
            .foregroundStyle(theme.dim)
            .disabled(model.isRunning)
            .help("Refresh (⌘R)")
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 14)
        // The title bar is hidden, so this strip is what you drag the window by.
        .background(WindowDragArea())
    }

    private var content: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if let error = model.errorMessage {
                    ErrorNote(message: error, theme: theme)
                }
                ForEach(Array(model.entries.enumerated()), id: \.element.id) { index, entry in
                    EntryView(
                        entry: entry,
                        theme: theme,
                        isFirst: index == 0,
                        showCaret: entry.isLive && model.isRunning
                    )
                }
            }
            .frame(maxWidth: 660, alignment: .leading)
            .padding(.horizontal, 44)
            .padding(.top, 30)
            .padding(.bottom, 60)
            .frame(maxWidth: .infinity, alignment: .center)
        }
        .scrollContentBackground(.hidden)
    }

    private var footer: some View {
        HStack {
            Text(footerText)
                .font(.system(size: 11))
                .foregroundStyle(theme.dim)
            Spacer()
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 9)
        .overlay(alignment: .top) { Rectangle().fill(theme.rule).frame(height: 1) }
    }

    private var footerText: String {
        if let error = model.errorMessage { return error }
        if model.isRunning { return model.status }
        guard let finished = model.finishedAt else { return model.status }
        return finished.formatted(date: .abbreviated, time: .shortened)
    }
}

/// One run's briefing, under a rule saying when it arrived.
private struct EntryView: View {
    let entry: Entry
    let theme: Theme
    let isFirst: Bool
    let showCaret: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !isFirst {
                Rectangle()
                    .fill(theme.rule)
                    .frame(height: 1)
                    .padding(.top, 40)
                    .padding(.bottom, 12)
            }
            Text(stamp)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(theme.dim)
                .padding(.bottom, 16)
            ForEach(Markdown.blocks(entry.text)) { block in
                BlockView(block: block, theme: theme, isFirst: block.id == 0)
            }
            if showCaret {
                Caret(theme: theme)
            }
        }
    }

    private var stamp: String {
        guard let at = entry.at else { return "now" }
        let time = at.formatted(date: .omitted, time: .shortened)
        let day = Calendar.current.isDateInToday(at)
            ? time
            : at.formatted(date: .abbreviated, time: .shortened)
        return entry.posts > 0 ? "\(day) · \(entry.posts) posts" : day
    }
}

private struct BlockView: View {
    let block: Block
    let theme: Theme
    let isFirst: Bool

    var body: some View {
        switch block.kind {
        case .heading(let title):
            Text(title)
                .font(.system(size: 21, weight: .semibold))
                .foregroundStyle(theme.text)
                .padding(.top, isFirst ? 0 : 34)
                .padding(.bottom, 12)

        case .paragraph(let runs):
            styled(runs)
                .font(.system(size: 15))
                .foregroundStyle(theme.text)
                .lineSpacing(7)
                .padding(.bottom, 10)

        case .bullet(let runs):
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Circle()
                    .fill(theme.accent.opacity(0.55))
                    .frame(width: 4, height: 4)
                    .offset(y: -3)
                styled(runs)
                    .font(.system(size: 14))
                    .foregroundStyle(theme.text)
                    .lineSpacing(5)
            }
            .padding(.bottom, 9)
        }
    }

    private func styled(_ runs: [Inline]) -> Text {
        runs.reduce(Text(verbatim: "")) { $0 + $1.text(theme) }
    }
}

/// A soft pulse while the text is still arriving.
private struct Caret: View {
    let theme: Theme
    @State private var on = false

    var body: some View {
        RoundedRectangle(cornerRadius: 1)
            .fill(theme.accent)
            .frame(width: 7, height: 15)
            .opacity(on ? 0.15 : 0.85)
            .animation(.easeInOut(duration: 0.7).repeatForever(), value: on)
            .onAppear { on = true }
            .padding(.top, 4)
    }
}

private struct ErrorNote: View {
    let message: String
    let theme: Theme

    var body: some View {
        Text(message)
            .font(.system(size: 13))
            .foregroundStyle(Color(hex: 0xE05561))
            .textSelection(.enabled)
            .padding(.bottom, 18)
    }
}

/// Lets the header strip drag the window now that the title bar is hidden.
private struct WindowDragArea: NSViewRepresentable {
    final class DragView: NSView {
        override var mouseDownCanMoveWindow: Bool { true }
    }

    func makeNSView(context: Context) -> NSView { DragView() }
    func updateNSView(_ view: NSView, context: Context) {}
}

extension Notification.Name {
    static let refreshBriefing = Notification.Name("refreshBriefing")
}
