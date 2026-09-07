import SwiftUI

/// A line of the briefing, once its shape is known.
struct Block: Identifiable {
    /// How much of a story the reader has already read. The model marks it in
    /// the text; the mark itself never reaches the screen.
    enum Mark {
        /// Already in the briefing they read, unchanged since.
        case carried
        /// Not in the briefing they read.
        case new
        /// In the briefing they read, but the story has moved.
        case updated
    }

    enum Kind {
        case heading(String, mark: Mark)
        case paragraph([Inline])
        case bullet([Inline])
    }

    /// The line's position in the briefing. Stable while text streams in, so
    /// SwiftUI keeps the views it already built instead of rebuilding them all.
    let id: Int
    let kind: Kind
}

/// One story of the briefing, and the unit a refresh updates.
///
/// Its id starts out as its heading, then sticks: a refresh hands each arriving
/// story the id of the block it replaces, so a heading can be rewritten without
/// the section being torn down and rebuilt somewhere else on the page.
struct Story: Identifiable {
    let id: String
    var heading: String
    var mark: Block.Mark
    var body: [Block]
}

/// A styled run inside one line.
enum Inline: Hashable {
    case plain(String)
    case bold(String)
    case italic(String)
    case handle(String)
}

enum Markdown {
    /// Splits the briefing into blocks. Blank lines carry no meaning here:
    /// spacing comes from the layout, not from the model's line breaks.
    static func blocks(_ raw: String) -> [Block] {
        var id = 0
        return raw.split(separator: "\n", omittingEmptySubsequences: false).compactMap { line in
            let line = line.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { return nil }
            defer { id += 1 }
            if let heading = line.dropPrefix("## ") ?? line.dropPrefix("# ") {
                let (title, mark) = splitMark(heading)
                return Block(id: id, kind: .heading(title, mark: mark))
            }
            if let bullet = line.dropPrefix("- ") ?? line.dropPrefix("• ") {
                return Block(id: id, kind: .bullet(inlines(bullet)))
            }
            return Block(id: id, kind: .paragraph(inlines(line)))
        }
    }

    /// The marks the model puts on a heading. They are written in English
    /// whatever the language of the briefing, so a suffix match is enough.
    static let marks: [(String, Block.Mark)] = [("[new]", .new), ("[updated]", .updated)]

    static func splitMark(_ heading: String) -> (String, Block.Mark) {
        let trimmed = heading.trimmingCharacters(in: .whitespaces)
        for (suffix, mark) in marks where trimmed.hasSuffix(suffix) {
            let title = String(trimmed.dropLast(suffix.count))
            return (title.trimmingCharacters(in: .whitespaces), mark)
        }
        return (heading, .carried)
    }

    /// Splits the briefing into its stories. Anything before the first heading
    /// is dropped: the prompt asks for none, and a stray preamble is not a story.
    static func stories(_ raw: String) -> [Story] {
        var out: [Story] = []
        var heading: (String, Block.Mark)?
        var body: [String] = []
        var used: [String: Int] = [:]

        func flush() {
            guard let (title, mark) = heading else { return }
            // Two stories can end up under one heading. Numbering the repeats
            // keeps every identity unique, which ForEach needs.
            let base = title.lowercased().split(separator: " ").joined(separator: " ")
            let seen = used[base, default: 0]
            used[base] = seen + 1
            out.append(Story(
                id: seen == 0 ? base : "\(base)#\(seen)",
                heading: title,
                mark: mark,
                body: blocks(body.joined(separator: "\n"))
            ))
        }

        for line in raw.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = line.trimmingCharacters(in: .whitespaces)
            if let title = line.dropPrefix("## ") ?? line.dropPrefix("# ") {
                flush()
                heading = splitMark(title)
                body = []
            } else if heading != nil {
                body.append(line)
            }
        }
        flush()
        return out
    }

    /// Everything up to the last finished line. A story is identified by its
    /// heading, and a heading still being typed is a different heading on every
    /// keystroke — parsing one would mint a new story per character, and the
    /// screen fills with `La`, `La c`, `La cag`.
    static func finishedLines(_ text: String) -> String {
        guard let end = text.lastIndex(of: "\n") else { return "" }
        return String(text[..<end])
    }

    /// Lays the briefing arriving over the one already on screen.
    ///
    /// `carried` is what was on screen when the run began, and must never be
    /// the reconciler's own previous output: feeding it back lets a run pile up
    /// its own half-written stories. Until the run ends, a carried story the
    /// briefing has not reached yet stays below — only then do we know it is
    /// really gone.
    static func reconcile(arriving: [Story], carried: [Story], final: Bool) -> [Story] {
        var next = inherit(arriving, from: carried)
        // The last story is still being written unless the run is over. If you
        // are already reading that section, hold the finished version in its new
        // place rather than let it shrink to one sentence and grow back.
        if !final, let partial = next.last,
           let complete = carried.first(where: { $0.id == partial.id }) {
            next[next.count - 1] = complete
        }
        if !final {
            let shown = Set(next.map(\.id))
            next += carried.filter { !shown.contains($0.id) }
        }
        return next
    }

    /// Gives an arriving story the identity of the one it replaces.
    ///
    /// A heading is what names a story, not what it is. Keeping the id of the
    /// block already on screen lets the heading itself be rewritten in place,
    /// instead of the block being torn down and a new one inserted elsewhere.
    static func inherit(_ arriving: [Story], from carried: [Story]) -> [Story] {
        var next = arriving
        // An unchanged heading is the story itself, so those claim their slot
        // first. Otherwise a reworded heading could take the slot of a story
        // still named further down the briefing.
        var taken = Set(next.map(\.id).filter { id in carried.contains { $0.id == id } })
        for i in next.indices where !taken.contains(next[i].id) {
            let match = carried
                .filter { !taken.contains($0.id) }
                .map { ($0.id, similarity($0.heading, next[i].heading)) }
                .filter { $0.1 >= renamedStory }
                .max { $0.1 < $1.1 }
            guard let id = match?.0 else { continue }
            taken.insert(id)
            next[i] = Story(id: id, heading: next[i].heading, mark: next[i].mark, body: next[i].body)
        }
        return next
    }

    /// A heading sharing this much of its wording with one on screen is that
    /// story renamed rather than a story of its own. "les 13 millions" becomes
    /// "les 15 millions" and everything else in the line stays.
    private static let renamedStory = 0.5

    /// How much of two headings is the same words, ignoring case and order.
    static func similarity(_ a: String, _ b: String) -> Double {
        let left = a.lowercased().split(separator: " ")
        let right = b.lowercased().split(separator: " ")
        let longest = max(left.count, right.count)
        guard longest > 0 else { return 1 }
        var pool: [Substring: Int] = [:]
        for word in left { pool[word, default: 0] += 1 }
        var shared = 0
        for word in right where (pool[word] ?? 0) > 0 {
            pool[word]! -= 1
            shared += 1
        }
        return Double(shared) / Double(longest)
    }

    /// Splits one line into styled runs: `**bold**`, `*italic*`, quoted phrases,
    /// and @handles.
    static func inlines(_ line: String) -> [Inline] {
        var runs: [Inline] = []
        var plain = ""
        let chars = Array(line)
        var i = 0

        func flush() {
            if !plain.isEmpty {
                runs.append(.plain(plain))
                plain = ""
            }
        }

        while i < chars.count {
            let c = chars[i]

            if c == "*", i + 1 < chars.count, chars[i + 1] == "*",
               let end = find(chars, open: i + 2, close: "**") {
                flush()
                runs.append(.bold(String(chars[(i + 2)..<end])))
                i = end + 2
                continue
            }

            // Only `*` opens an italic. `_` is markdown emphasis too, but it is
            // far more often part of a handle — `Frederic_Molas` paired with a
            // trailing `_` in `@LLCoolChris_` and italicised the paragraph
            // between them.
            if c == "*", let end = find(chars, open: i + 1, close: "*") {
                flush()
                runs.append(.italic(String(chars[(i + 1)..<end])))
                i = end + 1
                continue
            }

            // A quoted phrase reads as an aside, so it gets the same italic.
            if let (closing, width) = quotePair(c),
               let end = find(chars, open: i + 1, close: closing) {
                flush()
                runs.append(.italic(String(chars[i...(end + width - 1)])))
                i = end + width
                continue
            }

            if c == "@", let end = handleEnd(chars, from: i) {
                flush()
                runs.append(.handle(String(chars[i..<end])))
                i = end
                continue
            }

            plain.append(c)
            i += 1
        }
        flush()
        return runs
    }

    /// The closing mark for an opening quote, and how many characters it takes.
    private static func quotePair(_ c: Character) -> (String, Int)? {
        switch c {
        case "«": return ("»", 1)
        case "“": return ("”", 1)
        default: return nil
        }
    }

    private static func find(_ chars: [Character], open: Int, close: String) -> Int? {
        let needle = Array(close)
        guard open < chars.count, open + needle.count <= chars.count else { return nil }
        var i = open
        while i + needle.count <= chars.count {
            if Array(chars[i..<(i + needle.count)]) == needle, i > open {
                return i
            }
            i += 1
        }
        return nil
    }

    /// X handles are 1 to 15 characters of letters, digits and underscores.
    private static func handleEnd(_ chars: [Character], from start: Int) -> Int? {
        var i = start + 1
        while i < chars.count, chars[i].isLetter || chars[i].isNumber || chars[i] == "_" {
            i += 1
        }
        let length = i - start - 1
        return (1...15).contains(length) ? i : nil
    }
}

extension Inline {
    /// A styled `Text` run. Concatenating these keeps normal text flow and wrapping.
    func text(_ theme: Theme) -> Text {
        switch self {
        case .plain(let s):
            return Text(s)
        case .bold(let s):
            return Text(s).fontWeight(.semibold)
        case .italic(let s):
            return Text(s).italic().foregroundColor(theme.dim)
        case .handle(let s):
            return Text(s).fontWeight(.medium).foregroundColor(theme.accent)
        }
    }
}

extension StringProtocol {
    func dropPrefix(_ prefix: String) -> String? {
        hasPrefix(prefix) ? String(dropFirst(prefix.count)) : nil
    }
}
