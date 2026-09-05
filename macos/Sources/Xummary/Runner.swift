import Foundation

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
    @Published private(set) var text = ""
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
    /// Tells a finished run apart from the one now on screen.
    private var generation = 0
    private var outBuffer = Data()
    private var errBuffer = Data()

    func run() {
        cancel()

        guard let binary = Self.locateBinary() else {
            errorMessage = "xummary not found. Build it and copy it to ~/.local/bin/xummary."
            return
        }

        generation += 1
        let token = generation
        livePosts = 0
        replaced = false
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
        text = stored.text
        updatedAt = stored.at
        posts = stored.posts
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
        // The briefing you are reading is only cleared once real text arrives.
        // Whitespace does not count: a run with nothing to say still closes its
        // empty output with a newline, and that must not wipe the screen.
        if !replaced {
            guard text.contains(where: { !$0.isWhitespace }) else { return }
            replaced = true
            self.text = ""
        }
        self.text += text
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
            return
        }
        // A run with nothing new writes nothing and says so in its status. The
        // briefing on screen is still the current one, so leave it and its time
        // alone — the status stays in the footer.
        guard replaced else { return }
        updatedAt = Date()
        posts = livePosts
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
