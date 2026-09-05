// Checks for the inline markdown parser, run by tools/test.sh.
//
// The app is a single executable target with nowhere to hang XCTest, so these
// compile the real Markdown.swift and assert against it directly.

import Foundation

func runs(_ s: String) -> [Inline] { Markdown.inlines(s) }

func italics(_ r: [Inline]) -> [String] {
    r.compactMap { if case .italic(let s) = $0 { return s } else { return nil } }
}

func handles(_ r: [Inline]) -> [String] {
    r.compactMap { if case .handle(let s) = $0 { return s } else { return nil } }
}

func plains(_ r: [Inline]) -> [String] {
    r.compactMap { if case .plain(let s) = $0 { return s } else { return nil } }
}

var failures = 0

func check(_ name: String, _ passed: Bool, _ detail: @autoclosure () -> String = "") {
    print((passed ? "  ok   " : "  FAIL ") + name + (passed ? "" : "  -> \(detail())"))
    if !passed { failures += 1 }
}

// Underscores are markdown emphasis and also live inside handles. A name like
// Frederic_Molas once paired with the trailing underscore of @LLCoolChris_ and
// italicised everything between them.
let underscores = runs(
    "Le streamer Frederic_Molas a interpelle @elonmusk, denonce par "
        + "@Ilangabet et @LLCoolChris_, tandis que @Bonozoo_ y voit la fin."
)
check("underscores never open an italic", italics(underscores).isEmpty, "\(italics(underscores))")
check(
    "handles survive, trailing underscore included",
    handles(underscores) == ["@elonmusk", "@Ilangabet", "@LLCoolChris_", "@Bonozoo_"],
    "\(handles(underscores))"
)
check(
    "an underscored name stays plain text",
    plains(underscores).contains { $0.contains("Frederic_Molas") }
)

check("asterisks still italicise", italics(runs("a *vraiment* b")) == ["vraiment"])
check("double asterisks are bold, not italic", italics(runs("a **fort** b")).isEmpty)
check(
    "guillemets read as an aside",
    italics(runs("il dit \u{00AB} une phrase \u{00BB} ici")) == ["\u{00AB} une phrase \u{00BB}"]
)

// A bare @ is not a handle, and a handle stops at 15 characters.
check("a lone at-sign is plain", handles(runs("prix @ 10 euros")).isEmpty)
check("handles cap at 15 characters", handles(runs("@abcdefghijklmnopqrstuvwxyz")).isEmpty)

print(failures == 0 ? "\nall passed" : "\n\(failures) failed")
exit(failures == 0 ? 0 : 1)
