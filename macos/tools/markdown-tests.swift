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

// The model marks a story the reader has not seen. The mark drives a badge and
// must never survive into the heading text.
func heading(_ line: String) -> (String, Block.Mark)? {
    guard case .heading(let title, let mark)? = Markdown.blocks(line).first?.kind else {
        return nil
    }
    return (title, mark)
}

func headingIs(_ line: String, _ title: String, _ mark: Block.Mark) -> Bool {
    guard let (got, gotMark) = heading(line) else { return false }
    return got == title && gotMark == mark
}

check("a new heading loses its mark", headingIs("## Astra [new]", "Astra", .new),
      String(describing: heading("## Astra [new]")))
check("an updated heading loses its mark", headingIs("## Astra [updated]", "Astra", .updated),
      String(describing: heading("## Astra [updated]")))
check("an unmarked heading is carried", headingIs("## Astra", "Astra", .carried),
      String(describing: heading("## Astra")))
check("a mark inside a sentence is left alone",
      plains(runs("le modele [new] arrive")).joined().contains("[new]"))

// A refresh reconciles story by story, so identity has to survive a reword of
// the body and stay unique when two stories share a heading.
let sample = """
## Le ZEVENT franchit les 12 millions

La cagnotte grimpe.

## Astra [updated]

Un jour apres.

## Also
- une bricole — @a
"""
let told = Markdown.stories(sample)
check("a briefing splits into its stories", told.count == 3, "\(told.count)")
check("a story keeps its heading and mark",
      told[1].heading == "Astra" && told[1].mark == .updated,
      "\(told[1].heading) \(told[1].mark)")
check("identity ignores the body", told[0].id == Markdown.stories("## Le ZEVENT franchit les 12 millions\n\nTout autre texte.")[0].id)
check("Also is a story like any other", told[2].heading == "Also" && told[2].body.count == 1)
check("text before the first heading is dropped", Markdown.stories("preamble\n\n## A\n\nx").count == 1)

let twins = Markdown.stories("## Meme titre\n\nun\n\n## Meme titre\n\ndeux")
check("two stories under one heading stay distinct", twins[0].id != twins[1].id,
      "\(twins[0].id) vs \(twins[1].id)")

// Streaming a briefing one character at a time must never show a story whose
// heading is still being typed. Identity is the heading, so `La`, `La c`,
// `La cag` would each be a story of its own and the page fills with debris.
let briefing = """
## La cagnotte du ZEVENT franchit les 13 millions

La boutique a apporte 3,7 millions.

## Astra [updated]

Un jour apres son lancement.
"""
let wanted = Set(Markdown.stories(briefing).map(\.id))
let before = Markdown.stories("## Le ZEVENT franchit les 12 millions\n\nHier soir.")

var everShown = Set<String>()
var widest = 0
for length in 0...briefing.count {
    let sofar = String(briefing.prefix(length))
    let arriving = Markdown.stories(Markdown.finishedLines(sofar))
    let shown = Markdown.reconcile(arriving: arriving, carried: before, final: false)
    everShown.formUnion(shown.map(\.id))
    widest = max(widest, shown.count)
}
let debris = everShown.subtracting(wanted).subtracting(before.map(\.id))
check("a heading being typed never becomes a story", debris.isEmpty, "\(debris.sorted())")
check("the page never grows past what is on it", widest <= wanted.count + before.count, "\(widest)")

// The carried briefing stays visible until the run ends, then goes.
let mid = Markdown.reconcile(arriving: Markdown.stories("## Astra\n\nx"), carried: before, final: false)
check("what the run has not reached yet stays below", mid.map(\.id).contains(before[0].id))
let done = Markdown.reconcile(arriving: Markdown.stories("## Astra\n\nx"), carried: before, final: true)
check("a story the new briefing dropped goes at the end", done.count == 1 && done[0].heading == "Astra")

// A section you are reading holds its finished text while the new one streams.
let half = Markdown.reconcile(
    arriving: Markdown.stories("## Le ZEVENT franchit les 12 millions\n\nCe"),
    carried: before, final: false)
check("a section being rewritten does not shrink first",
      half.count == 1 && half[0].body.count == before[0].body.count, "\(half.count)")

// A heading whose figure has moved on is the same story, so it replaces the
// block rather than arriving beside it with the old number still showing.
let thirteen = Markdown.stories("## Le ZEVENT franchit les 13 millions d'euros\n\nLa cagnotte monte.")
let fifteen = Markdown.stories("## Le ZEVENT franchit les 15 millions d'euros\n\nElle monte encore.")
let renamed = Markdown.reconcile(arriving: fifteen, carried: thirteen, final: false)
check("a renamed heading replaces the story it came from", renamed.count == 1,
      "\(renamed.map(\.heading))")
// Its own last section is still being written, so the finished old text holds
// the place until the run ends. Then the new figure takes over.
let ended = Markdown.reconcile(arriving: fifteen, carried: thirteen, final: true)
check("the new figure wins once the run ends",
      ended.count == 1 && ended[0].heading.contains("15"), "\(ended.map(\.heading))")

let other = Markdown.stories("## Le ZEvent critique pour ses invites polemiques\n\nUn autre sujet.")
let both = Markdown.reconcile(arriving: other, carried: thirteen, final: false)
check("a heading sharing a few words does not claim the story", both.count == 2,
      "\(both.map(\.heading))")

// Streaming that rename must not show the old and the new number at once.
let renaming = "## Le ZEVENT franchit les 15 millions d'euros\n\nElle monte encore."
var doubled = 0
for length in 0...renaming.count {
    let shown = Markdown.reconcile(
        arriving: Markdown.stories(Markdown.finishedLines(String(renaming.prefix(length)))),
        carried: thirteen, final: false)
    if shown.count > 1 { doubled += 1 }
}
check("the old figure never shows next to the new one", doubled == 0, "\(doubled) frames")

// A story keeps the identity of the block it replaces, so a rewritten heading
// changes the text in place instead of tearing the section down.
check("a renamed story keeps the id of the block on screen",
      ended[0].id == thirteen[0].id, "\(ended[0].id) vs \(thirteen[0].id)")
check("a story nothing on screen matches keeps its own id",
      Markdown.inherit(other, from: thirteen)[0].id == other[0].id)

// One block on screen can only become one story, or two arriving sections
// would draw over each other.
let twoClaims = Markdown.inherit(
    Markdown.stories("## Le ZEVENT franchit les 14 millions d'euros\n\nx"
        + "\n\n## Le ZEVENT franchit les 15 millions d'euros\n\ny"),
    from: thirteen)
check("one block on screen is claimed once",
      twoClaims.filter { $0.id == thirteen[0].id }.count == 1, "\(twoClaims.map(\.id))")

// An unchanged heading is the story itself, so it takes its own slot before a
// reworded one can.
let alongside = Markdown.inherit(
    Markdown.stories("## Le ZEVENT franchit les 13 millions d'euros\n\nx"
        + "\n\n## Le ZEVENT franchit les 15 millions d'euros\n\ny"),
    from: thirteen)
check("an unchanged heading keeps its own block",
      alongside[0].id == thirteen[0].id && alongside[1].id != thirteen[0].id,
      "\(alongside.map(\.id))")

// Badges are read back from the stored briefing when the run ends. They have to
// land on the blocks already drawn, whatever those blocks were once called.
let drawn = Markdown.reconcile(arriving: fifteen, carried: thirteen, final: true)
let stored = Markdown.stories(
    "## Le ZEVENT franchit les 15 millions d'euros [updated]\n\nElle monte encore.")
let badged = Markdown.inherit(stored, from: drawn)
check("a badge lands on the block already drawn",
      badged.map(\.id) == drawn.map(\.id) && badged[0].mark == .updated,
      "\(badged.map(\.id)) \(badged[0].mark)")

print(failures == 0 ? "\nall passed" : "\n\(failures) failed")
exit(failures == 0 ? 0 : 1)
