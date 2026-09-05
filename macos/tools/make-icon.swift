// Draws the Xummary app icon and writes an .iconset.
//
//   swift tools/make-icon.swift AppIcon.iconset [stack|distil|bold]
//
// The default mark is an X over three shortening bars: the post, then the
// summary. Everything is laid out on a 1024 grid and scaled, so it stays sharp
// at 16 px.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// macOS icons sit inside their canvas: an 824 pt body on a 1024 pt grid.
let grid: CGFloat = 1024
let bodyInset: CGFloat = 100
let bodyRadius: CGFloat = 185
let centre = grid / 2

/// Two palettes. `paper` is the default: a printed briefing rather than
/// another dark glassy app icon.
let palette = ProcessInfo.processInfo.environment["ICON_PALETTE"] ?? "paper"
let isPaper = palette == "paper"

let groundTop = isPaper
    ? CGColor(red: 0.996, green: 0.984, blue: 0.965, alpha: 1)  // #FEFBF6
    : CGColor(red: 0.106, green: 0.129, blue: 0.188, alpha: 1)  // #1B2130
let groundBottom = isPaper
    ? CGColor(red: 0.949, green: 0.925, blue: 0.882, alpha: 1)  // #F2ECE1
    : CGColor(red: 0.047, green: 0.055, blue: 0.078, alpha: 1)  // #0C0E14
let accent = isPaper
    ? CGColor(red: 0.851, green: 0.294, blue: 0.153, alpha: 1)  // #D94B27
    : CGColor(red: 0.478, green: 0.635, blue: 0.969, alpha: 1)  // #7AA2F7

func paper(_ alpha: CGFloat) -> CGColor {
    isPaper
        ? CGColor(red: 0.106, green: 0.094, blue: 0.078, alpha: alpha)  // #1B1814 ink
        : CGColor(red: 0.902, green: 0.910, blue: 0.925, alpha: alpha)  // #E6E8EC
}

enum Variant: String {
    case stack, distil, bold
}

let arguments = CommandLine.arguments
let outputPath = arguments.count > 1 ? arguments[1] : "AppIcon.iconset"
let variant = Variant(rawValue: arguments.count > 2 ? arguments[2] : "stack") ?? .stack

// MARK: - The stack mark

/// An X over three shortening bars. Every position derives from the group's own
/// height, so the mark is centred on the body by construction rather than by
/// hand-tuned offsets.
enum Stack {
    static let xSize: CGFloat = 260
    static let xStroke: CGFloat = 62
    static let barHeight: CGFloat = 58
    static let bars: [(width: CGFloat, alpha: CGFloat)] = [
        (420, 0.92),
        (330, 0.62),
        (225, 0.40),
    ]
    static let gapUnderX: CGFloat = 70
    static let gapBetweenBars: CGFloat = 34

    static var height: CGFloat {
        let barsHeight = CGFloat(bars.count) * barHeight
        let barGaps = CGFloat(bars.count - 1) * gapBetweenBars
        return xSize + gapUnderX + barsHeight + barGaps
    }

    /// Top of the group, placed so its midpoint lands on the body's midpoint.
    static var top: CGFloat { centre + height / 2 }

    static var xBox: CGRect {
        CGRect(x: centre - xSize / 2, y: top - xSize, width: xSize, height: xSize)
    }

    static func barRect(_ index: Int) -> CGRect {
        let width = bars[index].width
        let barTop = top - xSize - gapUnderX - CGFloat(index) * (barHeight + gapBetweenBars)
        return CGRect(x: centre - width / 2, y: barTop - barHeight, width: width, height: barHeight)
    }
}

// MARK: - Drawing

func render(_ pixels: Int) -> CGImage {
    let context = CGContext(
        data: nil,
        width: pixels,
        height: pixels,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    context.interpolationQuality = .high
    context.setAllowsAntialiasing(true)
    context.scaleBy(x: CGFloat(pixels) / grid, y: CGFloat(pixels) / grid)

    drawGround(context)

    switch variant {
    case .stack:
        drawX(context, in: Stack.xBox, stroke: Stack.xStroke)
        for index in Stack.bars.indices {
            fill(context, Stack.barRect(index), paper(Stack.bars[index].alpha))
        }
    case .bold:
        let size: CGFloat = 400
        let box = CGRect(x: centre - size / 2, y: centre - size / 2, width: size, height: size)
        drawX(context, in: box, stroke: 96)
    case .distil:
        drawFunnel(context)
    }

    return context.makeImage()!
}

func drawGround(_ context: CGContext) {
    let body = CGRect(
        x: bodyInset,
        y: bodyInset,
        width: grid - bodyInset * 2,
        height: grid - bodyInset * 2
    )
    let squircle = CGPath(
        roundedRect: body,
        cornerWidth: bodyRadius,
        cornerHeight: bodyRadius,
        transform: nil
    )

    context.saveGState()
    context.addPath(squircle)
    context.clip()
    let gradient = CGGradient(
        colorsSpace: CGColorSpaceCreateDeviceRGB(),
        colors: [groundTop, groundBottom] as CFArray,
        locations: [0, 1]
    )!
    context.drawLinearGradient(
        gradient,
        start: CGPoint(x: 0, y: body.maxY),
        end: CGPoint(x: 0, y: body.minY),
        options: []
    )
    context.restoreGState()

    // A hairline of light along the edge, the way macOS icons catch it.
    context.saveGState()
    context.addPath(squircle)
    context.setStrokeColor(isPaper
        ? CGColor(red: 0, green: 0, blue: 0, alpha: 0.10)
        : CGColor(red: 1, green: 1, blue: 1, alpha: 0.09))
    context.setLineWidth(3)
    context.strokePath()
    context.restoreGState()
}

/// Two round-capped strokes corner to corner. The caps are inset by half the
/// stroke so the mark stays inside its box.
func drawX(_ context: CGContext, in box: CGRect, stroke: CGFloat) {
    let inset = stroke / 2
    context.saveGState()
    context.setStrokeColor(accent)
    context.setLineWidth(stroke)
    context.setLineCap(.round)

    context.move(to: CGPoint(x: box.minX + inset, y: box.minY + inset))
    context.addLine(to: CGPoint(x: box.maxX - inset, y: box.maxY - inset))
    context.strokePath()

    context.move(to: CGPoint(x: box.minX + inset, y: box.maxY - inset))
    context.addLine(to: CGPoint(x: box.maxX - inset, y: box.minY + inset))
    context.strokePath()
    context.restoreGState()
}

/// Five bars narrowing to one bright line: a timeline distilled to a briefing.
func drawFunnel(_ context: CGContext) {
    let height: CGFloat = 58
    let rows: [(width: CGFloat, y: CGFloat, color: CGColor)] = [
        (500, 667, paper(0.85)),
        (420, 575, paper(0.68)),
        (330, 483, paper(0.54)),
        (230, 391, paper(0.42)),
        (130, 299, accent),
    ]
    for row in rows {
        let rect = CGRect(x: centre - row.width / 2, y: row.y, width: row.width, height: height)
        fill(context, rect, row.color)
    }
}

func fill(_ context: CGContext, _ rect: CGRect, _ color: CGColor) {
    context.saveGState()
    context.setFillColor(color)
    context.addPath(
        CGPath(
            roundedRect: rect,
            cornerWidth: rect.height / 2,
            cornerHeight: rect.height / 2,
            transform: nil
        )
    )
    context.fillPath()
    context.restoreGState()
}

// MARK: - Output

func write(_ image: CGImage, to url: URL) {
    guard let destination = CGImageDestinationCreateWithURL(
        url as CFURL, UTType.png.identifier as CFString, 1, nil
    ) else {
        fatalError("cannot write \(url.path)")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        fatalError("cannot finalize \(url.path)")
    }
}

let output = URL(filePath: outputPath)
try? FileManager.default.removeItem(at: output)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

// The set iconutil expects: each point size at 1x and 2x.
for points in [16, 32, 128, 256, 512] {
    write(render(points), to: output.appending(path: "icon_\(points)x\(points).png"))
    write(render(points * 2), to: output.appending(path: "icon_\(points)x\(points)@2x.png"))
}

print("wrote \(output.path) (\(variant.rawValue))")
