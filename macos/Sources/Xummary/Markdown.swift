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
