// SPDX-License-Identifier: GPL-3.0-only
// Draws the AZ-EMU app icon (code only, no image assets).
import AppKit

let size = 1024.0
let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocus()

// Background: dark panel with a rounded square + subtle border, matching the
// app's own DJ-mixer dark theme.
let bgRect = NSRect(x: 0, y: 0, width: size, height: size)
let bgPath = NSBezierPath(roundedRect: bgRect, xRadius: size * 0.22, yRadius: size * 0.22)
NSColor(calibratedRed: 0.12, green: 0.12, blue: 0.14, alpha: 1.0).setFill()
bgPath.fill()

let innerRect = bgRect.insetBy(dx: size * 0.04, dy: size * 0.04)
let innerPath = NSBezierPath(roundedRect: innerRect, xRadius: size * 0.19, yRadius: size * 0.19)
NSColor(calibratedRed: 0.0, green: 0.4, blue: 1.0, alpha: 1.0).setStroke()
innerPath.lineWidth = size * 0.02
innerPath.stroke()

// "AZ" big, bold, white; "EMU" smaller, blue accent, below it.
let paragraph = NSMutableParagraphStyle()
paragraph.alignment = .center

let azAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: size * 0.34, weight: .heavy),
    .foregroundColor: NSColor(calibratedWhite: 0.92, alpha: 1.0),
    .paragraphStyle: paragraph,
]
let emuAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: size * 0.13, weight: .bold),
    .foregroundColor: NSColor(calibratedRed: 0.0, green: 0.55, blue: 1.0, alpha: 1.0),
    .paragraphStyle: paragraph,
]

let az = NSAttributedString(string: "AZ", attributes: azAttrs)
let emu = NSAttributedString(string: "EMU", attributes: emuAttrs)

az.draw(in: NSRect(x: 0, y: size * 0.36, width: size, height: size * 0.42))
emu.draw(in: NSRect(x: 0, y: size * 0.22, width: size, height: size * 0.18))

image.unlockFocus()

guard let tiff = image.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:]) else {
    fatalError("failed to render icon")
}
let outPath = CommandLine.arguments[1]
try! png.write(to: URL(fileURLWithPath: outPath))
