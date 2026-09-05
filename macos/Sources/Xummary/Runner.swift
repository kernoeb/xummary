import Foundation

/// Runs the `xummary` CLI and collects its output: the briefing on stdout,
/// progress on stderr.
@MainActor
final class BriefingModel: ObservableObject {
    @Published private(set) var text = ""
    @Published private(set) var status = "ready"
    @Published private(set) var isRunning = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var finishedAt: Date?
    @Published var hours = 24

    private var process: Process?
    private var lastError = ""
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
        text = ""
        lastError = ""
        errorMessage = nil
        finishedAt = nil
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

        group.notify(queue: DispatchQueue.global(qos: .userInitiated)) { [weak self] in
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
        text += Self.takeText(&outBuffer)
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
            }
        }
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
        } else {
            finishedAt = Date()
            status = ""
        }
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
