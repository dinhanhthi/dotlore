#!/usr/bin/env bash
#
# Regenerate the two committed icon artifacts from src-tauri/icons/logo.icon,
# the Icon Composer document that is the single source for Dotlore's app icon.
# It sits beside its outputs rather than under assets/, which vite.config.ts
# names as publicDir: everything there is copied into the built frontend and
# embedded in the binary, and a build-only source has no business shipping
# inside the app.
#
#   src-tauri/icons/Assets.car  — what macOS 26 draws, via CFBundleIconName
#                                 (src-tauri/Info.plist) and bundle.resources.
#   src-tauri/icons/icon.icns   — what macOS 13-15 falls back to through
#                                 CFBundleIconFile, and what `pnpm tauri dev`
#                                 hands to NSApp.setApplicationIconImage. Tauri
#                                 only does that in dev builds, unmasked, so a
#                                 dev run shows these bytes exactly as they are.
#
# Both come out of one source so the Dock cannot show one icon in dev and
# another in the installed app, which is what happened while icon.icns still
# held the pre-macOS-26 artwork.
#
# macOS 26 composites a legacy .icns onto its own grey backdrop before masking
# it, which nested Dotlore's rounded square inside a second one. A layered
# Icon Composer document compiled into an Assets.car is what avoids that.
#
# Committed rather than compiled during `pnpm build`: actool 26+ — so Xcode 26 —
# is only needed to run THIS script, not to build the app, and the bundler's own
# actool step crashes at random under its Node parent process.
#
# actool's own Icon.icns stops at 256px, too small for icon.icns. So the icns is
# rendered instead from a throwaway bundle carrying the fresh Assets.car: asking
# NSWorkspace for that bundle's icon returns exactly what the Dock draws in the
# installed app, at every size. Launch Services produces the large renditions
# asynchronously and hands out a grey dashed placeholder until they are ready,
# so the renderer waits for a 1024px frame with real colour in it rather than
# writing whatever came back first.
#
# If actool dies with "attempt to insert nil object", its daemon is wedged:
# `pkill -f ibtoold` and run this again.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
icons="$root/src-tauri/icons"
source_icon="$icons/logo.icon"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/out"

# --- Assets.car --------------------------------------------------------------
# Copied under the name actool derives the asset from, so the document can be
# called logo.icon while CFBundleIconName stays the constant "Icon" that
# src-tauri/Info.plist and scripts/build.sh both spell out.
if [ ! -f "$source_icon/icon.json" ]; then
	echo "error: $source_icon is not an Icon Composer document" >&2
	exit 1
fi
cp -R "$source_icon" "$work/Icon.icon"

xcrun actool "$work/Icon.icon" \
	--compile "$work/out" \
	--platform macosx \
	--minimum-deployment-target 26.0 \
	--app-icon Icon \
	--output-partial-info-plist "$work/out/partial.plist"

if [ ! -s "$work/out/Assets.car" ]; then
	echo "error: actool produced no Assets.car — Xcode 26 or newer is required" >&2
	exit 1
fi

# --- icon.icns ---------------------------------------------------------------
# A throwaway bundle, never launched, that exists only so Launch Services will
# render the new Assets.car for us.
app="$work/IconSource.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$work/out/Assets.car" "$app/Contents/Resources/Assets.car"
printf '#!/bin/sh\n' >"$app/Contents/MacOS/IconSource"
chmod +x "$app/Contents/MacOS/IconSource"
cat >"$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>IconSource</string>
	<key>CFBundleExecutable</key><string>IconSource</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleIconName</key><string>Icon</string>
</dict>
</plist>
PLIST
touch "$app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" >/dev/null 2>&1 || true

cat >"$work/render.swift" <<'SWIFT'
import AppKit

// argv: <bundle path> <output .iconset directory>
let bundle = CommandLine.arguments[1]
let out = URL(fileURLWithPath: CommandLine.arguments[2])
try! FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

func render(_ image: NSImage, _ px: Int) -> NSBitmapImageRep {
  let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  NSGraphicsContext.current!.imageInterpolation = .high
  image.draw(in: NSRect(x: 0, y: 0, width: px, height: px))
  NSGraphicsContext.restoreGraphicsState()
  return rep
}

// Launch Services' "not rendered yet" placeholder is a grey dashed outline with
// no saturated pixel anywhere in it; the real icon is mostly colour.
func colourFraction(_ rep: NSBitmapImageRep) -> Double {
  var coloured = 0, total = 0
  let step = max(1, rep.pixelsWide / 64)
  for y in stride(from: 0, to: rep.pixelsHigh, by: step) {
    for x in stride(from: 0, to: rep.pixelsWide, by: step) {
      guard let c = rep.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
      total += 1
      guard c.alphaComponent > 0.5 else { continue }
      let chans = [c.redComponent, c.greenComponent, c.blueComponent]
      if chans.max()! - chans.min()! > 0.08 { coloured += 1 }
    }
  }
  return total == 0 ? 0 : Double(coloured) / Double(total)
}

var icon = NSWorkspace.shared.icon(forFile: bundle)
var attempt = 0
while colourFraction(render(icon, 1024)) < 0.2 {
  attempt += 1
  if attempt > 30 {
    FileHandle.standardError.write(Data(
      "error: Launch Services never rendered a 1024px icon — still the placeholder\n".utf8))
    exit(1)
  }
  Thread.sleep(forTimeInterval: 0.5)
  icon = NSWorkspace.shared.icon(forFile: bundle)
}

// Every name an .icns carries, paired with the pixel size it must hold.
let wanted: [(String, Int)] = [
  ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
  ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
  ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
  ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
  ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
]

for (name, px) in wanted {
  try! render(icon, px).representation(using: .png, properties: [:])!
    .write(to: out.appendingPathComponent(name))
}
SWIFT

xcrun swiftc -O "$work/render.swift" -o "$work/render"
"$work/render" "$app" "$work/icon.iconset"
iconutil -c icns "$work/icon.iconset" -o "$work/icon.icns"

# --- install -----------------------------------------------------------------
cp "$work/out/Assets.car" "$icons/Assets.car"
cp "$work/icon.icns" "$icons/icon.icns"
echo "wrote $icons/Assets.car"
echo "wrote $icons/icon.icns"
