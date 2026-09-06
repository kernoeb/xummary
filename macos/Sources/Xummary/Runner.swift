import AppKit
import Foundation
import SwiftUI

/// A briefing the CLI has stored.
struct Entry {
    var at: Date
    var posts = 0
    var text = ""
}

/// Runs the `xummary` CLI and collects its output: the briefing on stdout,
/// progress on stderr.
///
/// There is one briefing, never a stack. It comes back from the CLI's cache at
/// launch, and a refresh rewrites it in place — so nothing is lost, and there
/// is only ever one thing to read.
@MainActor
final class BriefingModel: ObservableObject {
    /// The briefing on screen, one entry per story. A refresh reconciles into
    /// this list rather than replacing it, so a section that has not changed
    /// never moves and never flickers.
    @Published private(set) var stories: [Story] = []
    @Published private(set) var updatedAt: Date?
    @Published private(set) var posts = 0
    @Published private(set) var status = "ready"
    @Published private(set) var isRunning = false
    @Published private(set) var errorMessage: String?
    @Published var hours = 24

    private var process: Process?
    private var lastError = ""
    /// Post count read off the progress line, for the footer.
    private var livePosts = 0
    /// Whether this run has put anything on screen. Until it has, the briefing
    /// you were reading stays up — a run with nothing new must not blank it.
    private var replaced = false
    private var historyLoaded = false
    /// The briefing this run has produced so far, complete or not.
    private var incoming = ""
    /// What was on screen when the first line of the new briefing arrived.
    /// Leftovers come from here, never from the reconciler's own output, or a
    /// run piles up its own half-written stories.
    private var carried: [Story] = []
    /// Whether the briefing on screen has been reported as read.
    private var markedRead = false
    private var readTimer: Timer?
    /// Tells a finished run apart from the one now on screen.
    private var generation = 0
    private var outBuffer = Data()
    private var errBuffer = Data()

    func run() {
        // A run interrupted mid-stream leaves half-written stories up. Put the
        // briefing that was there back, or the next run snapshots the debris as
        // what you were already reading.
        if isRunning, replaced { stories = carried }
        cancel()

        guard let binary = Self.locateBinary() else {
            errorMessage = "xummary not found. Build it and copy it to ~/.local/bin/xummary."
            return
        }

        generation += 1
        let token = generation
        livePosts = 0
        replaced = false
        incoming = ""
        markedRead = false
        readTimer?.invalidate()
        readTimer = nil
        lastError = ""
        errorMessage = nil
        outBuffer = Data()
        errBuffer = Data()
        status = "starting"
        isRunning = true

        let process = Process()
        process.executableURL = binary
        process.arguments = ["--print", "--hours", String(hours)]
        process.environment = Self.childEnvironment()

        let out = Pipe()
        let err = Pipe()
        process.standardOutput = out
        process.standardError = err

        do {
            try process.run()
            self.process = process
        } catch {
            isRunning = false
            errorMessage = "could not start xummary: \(error.localizedDescription)"
            return
        }

        // Each pipe gets a thread that reads to end of file, so nothing written
        // just before the process exits is lost.
        let group = DispatchGroup()
        Self.drain(out.fileHandleForReading, into: group) { [weak self] chunk in
            Task { @MainActor in self?.absorbOutput(chunk, token: token) }
        }
        Self.drain(err.fileHandleForReading, into: group) { [weak self] chunk in
            Task { @MainActor in self?.absorbProgress(chunk, token: token) }
        }

        // `notify` takes a plain closure, so Swift infers this one as @MainActor
        // from the enclosing method and then asserts that at runtime — on a
        // global queue, which traps. @Sendable keeps it off the actor.
        group.notify(queue: DispatchQueue.global(qos: .userInitiated)) { @Sendable [weak self] in
            process.waitUntilExit()
            let code = process.terminationStatus
            Task { @MainActor in self?.finish(code, token: token) }
        }
    }

    func cancel() {
        // Bumping the generation makes anything still in flight from the old run
        // arrive stale, so it cannot clobber the run that replaces it.
        generation += 1
        process?.terminate()
        process = nil
        isRunning = false
    }

    /// How long the briefing has to be in front of you before it counts as
    /// read. Long enough that alt-tabbing past the window does not count.
    private static let readAfter: TimeInterval = 4

    /// Call when the app comes to the front or goes away. A new-story mark
    /// lives until you have actually looked at the briefing carrying it, so
    /// "read" is time spent with the window active, not the text arriving.
    func activeChanged(_ isActive: Bool) {
        readTimer?.invalidate()
        readTimer = nil
        guard isActive else { return }
        guard !markedRead, !isRunning, updatedAt != nil else { return }
        readTimer = Timer.scheduledTimer(withTimeInterval: Self.readAfter, repeats: false) {
            [weak self] _ in
            Task { @MainActor in self?.markRead() }
        }
    }

    /// Guards on there being a briefing at all, not on it having stories: a
    /// briefing the model wrote without headings still has to count as read, or
    /// the baseline never advances and everything stays marked new forever.
    private func markRead() {
        guard !markedRead, !isRunning, updatedAt != nil else { return }
        markedRead = true
        guard let binary = Self.locateBinary() else { return }
        let environment = Self.childEnvironment()
        Task.detached(priority: .background) {
            let process = Process()
            process.executableURL = binary
            process.arguments = ["--mark-read"]
            process.environment = environment
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try? process.run()
            process.waitUntilExit()
        }
    }

    /// Reads the briefing back once the run is done.
    ///
    /// The CLI decides which stories are new or have moved when it stores the
    /// briefing, by comparing it with the one already read — so the badges live
    /// only in the stored copy, never in what was streamed. The text is
    /// otherwise the same, so nothing moves: the badges just appear.
    private func adoptMarks() {
        guard let binary = Self.locateBinary() else { return }
        let environment = Self.childEnvironment()
        let token = generation
        Task.detached(priority: .userInitiated) {
            let stored = Self.readLog(binary, environment)
            await MainActor.run { self.adopt(stored.first, token: token) }
        }
    }

    private func adopt(_ stored: Entry?, token: Int) {
        guard token == generation, !isRunning, let stored else { return }
        let marked = Markdown.stories(stored.text)
        // Marks are stripped out of a story's identity, so the same briefing
        // gives the same ids. Anything else is not the briefing on screen.
        guard !marked.isEmpty, marked.map(\.id) == stories.map(\.id) else { return }
        stories = marked
    }

    /// Puts the last stored briefing on screen at launch, so the window has
    /// something to read while the refresh runs.
    func loadHistory() {
        guard let binary = Self.locateBinary() else { return }
        let environment = Self.childEnvironment()
        Task.detached(priority: .userInitiated) {
            let stored = Self.readLog(binary, environment)
            await MainActor.run { self.show(stored.first) }
        }
    }

    /// Never over the run that launched alongside it: the two land in either
    /// order, and the fresher one wins.
    private func show(_ stored: Entry?) {
        guard !historyLoaded else { return }
        historyLoaded = true
        guard let stored, !replaced else { return }
        stories = Markdown.stories(stored.text)
        updatedAt = stored.at
        posts = stored.posts
        activeChanged(NSApplication.shared.isActive)
    }

    /// `xummary --log` prints the stored briefings as JSON lines, oldest first.
    private nonisolated static func readLog(_ binary: URL, _ environment: [String: String]) -> [Entry] {
        let process = Process()
        process.executableURL = binary
        process.arguments = ["--log"]
        process.environment = environment
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return []
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return [] }

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let text = try decoder.singleValueContainer().decode(String.self)
            guard let date = Self.timestamp(text) else {
                throw DecodingError.dataCorruptedError(
                    in: try decoder.singleValueContainer(),
                    debugDescription: "not an RFC 3339 timestamp: \(text)"
                )
            }
            return date
        }
        return data.split(separator: UInt8(ascii: "\n"))
            .compactMap { try? decoder.decode(Stored.self, from: Data($0)) }
            .map { Entry(at: $0.at, posts: $0.posts, text: $0.text) }
            .reversed()
    }

    private struct Stored: Decodable {
        let at: Date
        let posts: Int
        let text: String
    }

    /// chrono writes fractional seconds; ISO8601DateFormatter only reads them
    /// when told to, and only reads whole seconds when told not to.
    private nonisolated static func timestamp(_ text: String) -> Date? {
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return withFraction.date(from: text) ?? ISO8601DateFormatter().date(from: text)
    }

    private static func drain(
        _ handle: FileHandle,
        into group: DispatchGroup,
        onChunk: @escaping @Sendable (Data) -> Void
    ) {
        group.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            while true {
                let chunk = handle.availableData
                if chunk.isEmpty { break }
                onChunk(chunk)
            }
            group.leave()
        }
    }

    private func absorbOutput(_ chunk: Data, token: Int) {
        guard token == generation else { return }
        outBuffer.append(chunk)
        let text = Self.takeText(&outBuffer)
        guard !text.isEmpty else { return }
        // Whitespace does not count as a briefing arriving: a run with nothing
        // to say still closes its empty output with a newline, and that must not
        // wipe the screen.
        if !replaced {
            guard text.contains(where: { !$0.isWhitespace }) else { return }
            replaced = true
            // Snapshot now, not when the run started: at launch the run begins
            // before the stored briefing has been read back, so a snapshot taken
            // then is empty and the first section wipes the page.
            carried = stories
        }
        incoming += text
        apply(Markdown.stories(Markdown.finishedLines(incoming)), final: false)
    }

    private func apply(_ arriving: [Story], final: Bool) {
        let next = Markdown.reconcile(arriving: arriving, carried: carried, final: final)
        // Only a change in which stories show, or where, is worth animating. A
        // section rewriting its own paragraphs should just rewrite them.
        if next.map(\.id) == stories.map(\.id) {
            stories = next
        } else {
            withAnimation(.easeInOut(duration: 0.25)) { stories = next }
        }
    }

    private func absorbProgress(_ chunk: Data, token: Int) {
        guard token == generation else { return }
        errBuffer.append(chunk)
        for line in Self.takeText(&errBuffer).split(separator: "\n") {
            let line = line.trimmingCharacters(in: .whitespaces)
            guard !line.isEmpty else { continue }
            if line.hasPrefix("error:") {
                lastError = String(line.dropFirst("error:".count))
                    .trimmingCharacters(in: .whitespaces)
            } else {
                status = line
                livePosts = Self.postCount(line) ?? livePosts
            }
        }
    }

    /// Progress lines start with the post count: "48 posts · thinking · 3s".
    private static func postCount(_ line: String) -> Int? {
        let digits = line.prefix { $0.isNumber }
        guard !digits.isEmpty, line.dropFirst(digits.count).hasPrefix(" posts") else { return nil }
        return Int(digits)
    }

    /// Decodes the whole characters at the front of `buffer` and removes them.
    /// A read can end mid-character, and those bytes wait for the next one.
    private static func takeText(_ buffer: inout Data) -> String {
        let count = completeUTF8Length(buffer)
        guard count > 0, let text = String(data: buffer.prefix(count), encoding: .utf8) else {
            return ""
        }
        buffer.removeFirst(count)
        return text
    }

    /// How many leading bytes make up whole UTF-8 characters.
    private static func completeUTF8Length(_ data: Data) -> Int {
        var trailing = 0
        var index = data.count
        while index > 0, trailing < 4 {
            let byte = data[data.startIndex + index - 1]
            if byte & 0b1100_0000 != 0b1000_0000 {
                let needed: Int
                if byte & 0b1000_0000 == 0 {
                    needed = 1
                } else if byte & 0b1110_0000 == 0b1100_0000 {
                    needed = 2
                } else if byte & 0b1111_0000 == 0b1110_0000 {
                    needed = 3
                } else {
                    needed = 4
                }
                return trailing + 1 >= needed ? data.count : index - 1
            }
            trailing += 1
            index -= 1
        }
        return data.count
    }

    private func finish(_ code: Int32, token: Int) {
        guard token == generation else { return }
        isRunning = false
        process = nil

        if code != 0 {
            errorMessage = lastError.isEmpty ? "xummary exited with code \(code)" : lastError
            status = "failed"
            // A briefing that failed part way is not a briefing, and the CLI
            // stored nothing either. Put back the one you were reading.
            if replaced { stories = carried }
            return
        }
        // A run with nothing new writes nothing and says so in its status. The
        // briefing on screen is still the current one, so leave it and its time
        // alone — the status stays in the footer.
        guard replaced else {
            // Nothing was rewritten, so the briefing on screen is still the one
            // whose marks are waiting to be seen. Keep counting.
            activeChanged(NSApplication.shared.isActive)
            return
        }
        apply(Markdown.stories(incoming), final: true)
        carried = []
        updatedAt = Date()
        posts = livePosts
        adoptMarks()
        activeChanged(NSApplication.shared.isActive)
    }

    /// Prefers the copy bundled inside the .app, then the usual install spots.
    private static func locateBinary() -> URL? {
        var candidates: [URL] = []
        if let bundled = Bundle.main.url(forResource: "xummary", withExtension: nil) {
            candidates.append(bundled)
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        candidates.append(home.appending(path: ".local/bin/xummary"))
        candidates.append(URL(filePath: "/opt/homebrew/bin/xummary"))
        candidates.append(URL(filePath: "/usr/local/bin/xummary"))
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    /// A launched app inherits almost no PATH, and `xummary` has to find `claude`.
    private static func childEnvironment() -> [String: String] {
        var env = ProcessInfo.processInfo.environment
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let extras = [
            "\(home)/.local/bin",
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
        ]
        let existing = env["PATH"].map { $0.split(separator: ":").map(String.init) } ?? []
        var seen = Set<String>()
        env["PATH"] = (extras + existing).filter { seen.insert($0).inserted }.joined(separator: ":")
        return env
    }
}
