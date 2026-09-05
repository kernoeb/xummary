// Draws the Xummary app icon and writes an .iconset.
//
//   swift tools/make-icon.swift AppIcon.iconset
//
// The mark is an X above three shortening bars: the post, then the summary.
// Everything is laid out on a 1024 grid and scaled, so it stays sharp at 16 px.

import AppKit
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// macOS icons sit inside their canvas: a 824 pt body on a 1024 pt grid.
let grid: CGFloat = 1024
let bodyInset: CGFloat = 100
let bodyRadius: CGFloat = 185

let groundTop = CGColor(red: 0.106, green: 0.129, blue: 0.188, alpha: 1)   // #1B2130
let groundBottom = CGColor(red: 0.047, green: 0.055, blue: 0.078, alpha: 1) // #0C0E14
let accent = CGColor(red: 0.478, green: 0.635, blue: 0.969, alpha: 1)       // #7AA2F7
let paper = (red: 0.902, green: 0.910, blue: 0.925)                         // #E6E8EC

func render(_ pixels: Int) -> CGImage {
    let scale = CGFloat(pixels) / grid
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
    context.scaleBy(x: scale, y: scale)

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

    // A hairline of light along the top edge, the way macOS icons catch it.
    context.saveGState()
    context.addPath(squircle)
    context.setStrokeColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.09))
    context.setLineWidth(3)
    context.strokePath()
    context.restoreGState()

    if variant != "distil" { drawX(context) }
    drawBars(context)

    return context.makeImage()!
}

/// Which mark to draw. Set with the second argument: `bold` or `stack`.
let variant = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "bold"

/// The X, drawn as two round-capped strokes.
func drawX(_ context: CGContext) {
    let box = variant == "bold"
        ? CGRect(x: 312, y: 312, width: 400, height: 400)
        : CGRect(x: 382, y: 566, width: 260, height: 260)
    let stroke: CGFloat = variant == "bold" ? 96 : 62
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

/// Three centred bars, each shorter and fainter: a summary settling down.
func drawBars(_ context: CGContext) {
    if variant == "bold" { return }
    if variant == "distil" {
        drawFunnel(context)
        return
    }
    let bars: [(width: CGFloat, y: CGFloat, alpha: CGFloat)] = [
        (420, 448, 0.92),
        (330, 360, 0.62),
        (225, 272, 0.40),
    ]
    let height: CGFloat = 58

    context.saveGState()
    for bar in bars {
        let rect = CGRect(x: 512 - bar.width / 2, y: bar.y, width: bar.width, height: height)
        context.setFillColor(
            CGColor(red: paper.red, green: paper.green, blue: paper.blue, alpha: bar.alpha)
        )
        context.addPath(
            CGPath(
                roundedRect: rect,
                cornerWidth: height / 2,
                cornerHeight: height / 2,
                transform: nil
            )
        )
        context.fillPath()
    }
    context.restoreGState()
}

/// Five bars narrowing to one bright line: a timeline distilled to a briefing.
func drawFunnel(_ context: CGContext) {
    let height: CGFloat = 58
    let rows: [(width: CGFloat, y: CGFloat, color: CGColor)] = [
        (500, 667, paperColor(0.85)),
        (420, 575, paperColor(0.68)),
        (330, 483, paperColor(0.54)),
        (230, 391, paperColor(0.42)),
        (130, 299, accent),
    ]
    context.saveGState()
    for row in rows {
        let rect = CGRect(x: 512 - row.width / 2, y: row.y, width: row.width, height: height)
        context.setFillColor(row.color)
        context.addPath(
            CGPath(roundedRect: rect, cornerWidth: height / 2, cornerHeight: height / 2, transform: nil)
        )
        context.fillPath()
    }
    context.restoreGState()
}

func paperColor(_ alpha: CGFloat) -> CGColor {
    CGColor(red: paper.red, green: paper.green, blue: paper.blue, alpha: alpha)
}

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

let outputPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "AppIcon.iconset"
let output = URL(filePath: outputPath)
try? FileManager.default.removeItem(at: output)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

// The set iconutil expects: each point size at 1x and 2x.
for points in [16, 32, 128, 256, 512] {
    write(render(points), to: output.appending(path: "icon_\(points)x\(points).png"))
    write(render(points * 2), to: output.appending(path: "icon_\(points)x\(points)@2x.png"))
}

print("wrote \(output.path)")
