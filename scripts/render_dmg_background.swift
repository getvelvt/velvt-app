import AppKit
import Foundation

guard CommandLine.arguments.count == 3 else {
    fputs("usage: render_dmg_background.swift LOGO_PNG OUTPUT_PNG\n", stderr)
    exit(2)
}

let logoURL = URL(fileURLWithPath: CommandLine.arguments[1])
let outputURL = URL(fileURLWithPath: CommandLine.arguments[2])
guard let logo = NSImage(contentsOf: logoURL) else {
    fputs("error: unable to load Velvt logo\n", stderr)
    exit(1)
}

let canvasSize = NSSize(width: 660, height: 420)
let image = NSImage(size: canvasSize)
image.lockFocus()

let canvas = NSRect(origin: .zero, size: canvasSize)
let gradient = NSGradient(
    starting: NSColor(calibratedRed: 0.075, green: 0.055, blue: 0.10, alpha: 1),
    ending: NSColor(calibratedRed: 0.18, green: 0.055, blue: 0.14, alpha: 1)
)!
gradient.draw(in: canvas, angle: -35)

NSColor(calibratedWhite: 1, alpha: 0.07).setFill()
NSBezierPath(roundedRect: NSRect(x: 55, y: 78, width: 210, height: 230), xRadius: 26, yRadius: 26).fill()
NSBezierPath(roundedRect: NSRect(x: 395, y: 78, width: 210, height: 230), xRadius: 26, yRadius: 26).fill()

let paragraph = NSMutableParagraphStyle()
paragraph.alignment = .center
let heading: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 25, weight: .semibold),
    .foregroundColor: NSColor.white,
    .paragraphStyle: paragraph,
]
let subheading: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 14, weight: .regular),
    .foregroundColor: NSColor(calibratedWhite: 1, alpha: 0.68),
    .paragraphStyle: paragraph,
]

("Install Velvt" as NSString).draw(in: NSRect(x: 80, y: 358, width: 500, height: 34), withAttributes: heading)
("Drag Velvt into Applications" as NSString).draw(
    in: NSRect(x: 80, y: 330, width: 500, height: 24), withAttributes: subheading)

logo.draw(
    in: NSRect(x: 117, y: 142, width: 96, height: 96),
    from: .zero,
    operation: .sourceOver,
    fraction: 1
)

let arrowColor = NSColor(calibratedRed: 0.91, green: 0.15, blue: 0.49, alpha: 0.95)
arrowColor.setStroke()
arrowColor.setFill()
let arrow = NSBezierPath()
arrow.lineWidth = 5
arrow.lineCapStyle = .round
arrow.move(to: NSPoint(x: 286, y: 194))
arrow.line(to: NSPoint(x: 365, y: 194))
arrow.stroke()
let arrowHead = NSBezierPath()
arrowHead.move(to: NSPoint(x: 365, y: 194))
arrowHead.line(to: NSPoint(x: 346, y: 207))
arrowHead.line(to: NSPoint(x: 346, y: 181))
arrowHead.close()
arrowHead.fill()

image.unlockFocus()
guard
    let tiff = image.tiffRepresentation,
    let bitmap = NSBitmapImageRep(data: tiff),
    let png = bitmap.representation(using: .png, properties: [:])
else {
    fputs("error: unable to encode DMG background\n", stderr)
    exit(1)
}
try png.write(to: outputURL, options: .atomic)
