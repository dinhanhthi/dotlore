#!/usr/bin/env bash
#
# Cargo runner for `pnpm tauri dev` on macOS. Launch Services names a bare
# Mach-O after its filename, so the Dock hover would read dotlore.
# Executing that binary from Dotlore.app/Contents/MacOS with
# CFBundleDisplayName set registers the title as Dotlore.
set -euo pipefail

if [ "$#" -lt 1 ]; then
	echo "error: cargo runner expected a binary path" >&2
	exit 1
fi

bin="$1"
shift

if [ "$(basename "$bin")" != "dotlore" ]; then
	exec "$bin" "$@"
fi

app="$(dirname "$bin")/Dotlore.app"
macos="$app/Contents/MacOS"
mkdir -p "$macos"

rm -f "$macos/dotlore"
if ! ln "$bin" "$macos/dotlore"; then
	cp -f "$bin" "$macos/dotlore"
fi

cat > "$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDisplayName</key>
	<string>Dotlore</string>
	<key>CFBundleName</key>
	<string>Dotlore</string>
	<key>CFBundleExecutable</key>
	<string>dotlore</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
</dict>
</plist>
EOF

exec "$macos/dotlore" "$@"
