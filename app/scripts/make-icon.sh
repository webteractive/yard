#!/bin/bash
# Renders Yard's icon (app/yard-app/macos/AppIcon.svg, the D3 logo of the visual design spec §6)
# into AppIcon.icns and the 112 px PNG onboarding shows. The .icns follows Apple's icon grid: the
# tile fills 824 of 1024 px, centred, as every other app's does in the Dock. Needs swift and
# iconutil (Xcode's command line tools). Run it after changing the SVG, and commit its outputs.
set -euo pipefail

macos="$(cd "$(dirname "$0")/../yard-app/macos" && pwd)"
svg="$macos/AppIcon.svg"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"

# render <out.png> <canvas px> <tile px>: the SVG drawn at <tile px>, centred on a clear canvas.
cat >"$work/render.swift" <<'SWIFT'
import AppKit
let args = CommandLine.arguments
guard args.count == 5, let image = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: render <svg> <out.png> <canvas> <tile>\n".data(using: .utf8)!)
    exit(2)
}
let canvas = Int(args[3])!, tile = CGFloat(Int(args[4])!)
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: canvas, pixelsHigh: canvas, bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: canvas, height: canvas)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSGraphicsContext.current?.imageInterpolation = .high
let inset = (CGFloat(canvas) - tile) / 2
image.draw(in: NSRect(x: inset, y: inset, width: tile, height: tile))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
SWIFT
swiftc -O -o "$work/render" "$work/render.swift"

for size in 16 32 128 256 512; do
  for scale in 1 2; do
    px=$((size * scale))
    suffix=""
    if [ "$scale" = 2 ]; then suffix="@2x"; fi
    name="icon_${size}x${size}${suffix}.png"
    "$work/render" "$svg" "$iconset/$name" "$px" $((px * 824 / 1024))
  done
done
iconutil -c icns "$iconset" -o "$macos/AppIcon.icns"
# Onboarding's heading shows the tile alone, edge to edge.
"$work/render" "$svg" "$macos/AppIcon-112.png" 112 112
echo "wrote $macos/AppIcon.icns and $macos/AppIcon-112.png"
