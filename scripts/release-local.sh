#!/usr/bin/env bash
#
# Release Dotlore from this Mac: build a signed + notarized universal bundle
# through scripts/build.sh, check every artifact, push the tag, publish the
# GitHub release (draft, upload, publish) and dispatch the website deploy.
# Nothing is tagged or published unless the build and every check pass.
#
# Secrets come from the login keychain and ~/.tauri, travel only as an env
# prefix on the scripts/build.sh call, and are never printed.
set -euo pipefail

repo="dinhanhthi/dotlore"
identity="Developer ID Application: Anh-Thi Dinh (86H6CNLN4C)"
api_key_id="Q76HXMGCWL"
key_path="$HOME/.appstoreconnect/private_keys/AuthKey_$api_key_id.p8"
updater_key="$HOME/.tauri/dotlore.key"
notary_service="dotlore-notary-issuer"
updater_service="dotlore-updater-signing"

usage() {
	cat <<EOF
Usage: scripts/release-local.sh --expect-version <X.Y.Z> [--dry-run | --build-only | --publish-only]

  --expect-version <v>  the version to release as tag v<v> (must match src-tauri/Cargo.toml)
  --dry-run             run the preflight checks only (no build, tag or publish)
  --build-only          build and check every artifact, no tag and no publish
  --publish-only        resume after the tag push: reuse the built artifacts and publish
  -h, --help            show this help
EOF
}

fail() {
	echo "error: $*" >&2
	exit 1
}

ver=""
mode="release"
while [ $# -gt 0 ]; do
	case "$1" in
		--expect-version)
			[ $# -ge 2 ] || fail "--expect-version requires a value"
			ver="$2"
			shift
			;;
		--dry-run | --build-only | --publish-only)
			[ "$mode" = "release" ] || fail "--dry-run, --build-only and --publish-only are mutually exclusive"
			mode="${1#--}"
			;;
		-h | --help)
			usage
			exit 0
			;;
		*)
			usage >&2
			fail "unknown argument: $1"
			;;
	esac
	shift
done

[ -n "$ver" ] || {
	usage >&2
	fail "--expect-version is required"
}
[[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "--expect-version must be X.Y.Z (got $ver)"
tag="v$ver"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

export GH_REPO="$repo"
# Only the App Store Connect API key path may notarize; an ambient Apple ID or
# certificate would make Tauri pick another path or run its own keychain import.
unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD

tmp="$(mktemp -d)"
tag_pushed=0
cleanup() {
	local status=$?
	rm -rf "$tmp"
	if [ "$status" -ne 0 ] && [ "$tag_pushed" -eq 1 ]; then
		echo "tag $tag is pushed; never delete it. Resume: pnpm release --expect-version $ver --publish-only" >&2
	fi
}
trap cleanup EXIT

echo "==> preflight for $tag ($mode)"

# --- every mode -----------------------------------------------------------------
gh auth status >/dev/null 2>&1 || fail "gh is not authenticated. Run: gh auth login"
case "$(git remote get-url origin)" in
	*"$repo"*) ;;
	*) fail "origin is not $repo" ;;
esac

# src-tauri/Cargo.toml [package] version is the single source of truth.
cargo_ver="$(python3 -c 'import tomllib;print(tomllib.load(open("src-tauri/Cargo.toml","rb"))["package"]["version"])')"
[ "$cargo_ver" = "$ver" ] ||
	fail "src-tauri/Cargo.toml version is $cargo_ver, expected $ver. Bump the version files (/cf-ship does this) first"

# Literal string comparison, not a regex: awk processes backslash escapes in -v
# assignments, so an escaped "1\.0\.0" would arrive as "1.0.0" with wildcard dots.
# The body is written to a file for --notes-file and never printed.
[ -f CHANGELOG.md ] || fail "CHANGELOG.md not found"
awk -v v="$ver" '
	function is_version_heading(line, ver,   rest, nxt) {
		if (substr(line, 1, 3) != "## ") return 0
		rest = substr(line, 4)
		if (substr(rest, 1, 1) == "v") rest = substr(rest, 2)
		if (substr(rest, 1, length(ver)) != ver) return 0
		nxt = substr(rest, length(ver) + 1, 1)
		return (nxt == "" || nxt == " " || nxt == "(")
	}
	is_version_heading($0, v) { found = 1; next }
	found && substr($0, 1, 3) == "## " { exit }
	found { print }
' CHANGELOG.md >"$tmp/notes.md"
grep -q '[^[:space:]]' "$tmp/notes.md" || fail "CHANGELOG.md has no non-empty '## $tag' section"

# --- repository state -------------------------------------------------------------
if [ "$mode" = "release" ] || [ "$mode" = "dry-run" ]; then
	[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || fail "must release from main"
	[ -z "$(git status --porcelain)" ] || fail "working tree is not clean"
	git fetch -q origin
	release_sha="$(git rev-parse HEAD)"
	[ "$release_sha" = "$(git rev-parse origin/main)" ] || fail "HEAD is not origin/main; push or pull first"
	git rev-parse -q --verify "refs/tags/$tag" >/dev/null && fail "tag $tag already exists locally"
	if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
		fail "tag $tag already exists on origin"
	fi
fi

if [ "$mode" = "publish-only" ]; then
	[ -z "$(git status --porcelain)" ] || fail "working tree is not clean"
	# Lightweight tags have no peeled ^{} entry, so fall back to the tag itself.
	remote_sha="$(git ls-remote origin "refs/tags/$tag^{}" | cut -f1)"
	[ -n "$remote_sha" ] || remote_sha="$(git ls-remote origin "refs/tags/$tag" | cut -f1)"
	[ -n "$remote_sha" ] || fail "tag $tag is not on origin; release without --publish-only"
	[ "$remote_sha" = "$(git rev-parse HEAD)" ] || fail "tag $tag on origin is not HEAD; check out $tag first"
	# A published release is never touched: clients may already have its latest.json.
	if [ "$(gh release view "$tag" --json isDraft -q .isDraft 2>/dev/null || echo none)" = "false" ]; then
		fail "release $tag is already published. If the website is stale, run: gh workflow run pages.yml --ref main"
	fi
	tag_pushed=1
fi

# --- build setup ------------------------------------------------------------------
if [ "$mode" != "publish-only" ]; then
	# Fail, not warn: scripts/build.sh degrades to a single-arch bundle.
	installed="$(rustup target list --installed)"
	for t in aarch64-apple-darwin x86_64-apple-darwin; do
		grep -qx "$t" <<<"$installed" || fail "rust target $t is not installed. Run once: rustup target add $t"
	done
	identities="$(security find-identity -v -p codesigning)"
	grep -qF "$identity" <<<"$identities" || fail "signing identity not in the login keychain: $identity"
	[ -s "$key_path" ] ||
		fail "no App Store Connect key at $key_path. Run once: install -d -m 700 \"$(dirname "$key_path")\" && install -m 600 <path to AuthKey_$api_key_id.p8> \"$key_path\""
	[ -s "$updater_key" ] || fail "no updater signing key at $updater_key. Restore it from your backup; it can never be regenerated"
	for svc in "$notary_service" "$updater_service"; do
		security find-generic-password -s "$svc" >/dev/null 2>&1 ||
			fail "keychain item $svc not found. Run once: security add-generic-password -a \"\$USER\" -s $svc -w"
	done
	# notarytool echoes an invalid --issuer back in its error, so check the shape
	# here, before the build, without ever printing the value.
	[[ "$(security find-generic-password -s "$notary_service" -w)" =~ ^[0-9A-Fa-f]{8}(-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$ ]] ||
		fail "keychain item $notary_service does not hold an App Store Connect Issuer ID (a UUID). Replace it: security add-generic-password -U -a \"\$USER\" -s $notary_service -w"
fi

if [ "$mode" = "dry-run" ]; then
	echo "dry run: preflight passed for $tag"
	exit 0
fi

# --- build ------------------------------------------------------------------------
if [ "$mode" != "publish-only" ]; then
	echo "==> building $tag"
	pnpm install --frozen-lockfile
	# Plain assignments, so set -e catches a failed read.
	issuer="$(security find-generic-password -s "$notary_service" -w)"
	updater_pw="$(security find-generic-password -s "$updater_service" -w)"
	updater_key_text="$(<"$updater_key")"
	APPLE_SIGNING_IDENTITY="$identity" \
		APPLE_API_KEY="$api_key_id" \
		APPLE_API_ISSUER="$issuer" \
		APPLE_API_KEY_PATH="$key_path" \
		TAURI_SIGNING_PRIVATE_KEY="$updater_key_text" \
		TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$updater_pw" \
		bash scripts/build.sh
	unset issuer updater_pw updater_key_text
fi

# --- artifact checks --------------------------------------------------------------
echo "==> checking the artifacts"
# Same resolution as scripts/build.sh: Cargo's config decides the target dir.
unset CARGO_TARGET_DIR
target_dir="$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/src-tauri/Cargo.toml" |
	python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
bundle="$target_dir/universal-apple-darwin/release/bundle"
app="$bundle/macos/Dotlore.app"
[ -d "$app" ] || fail "no bundle at $app"

app_ver="$(plutil -extract CFBundleShortVersionString raw -o - "$app/Contents/Info.plist")"
[ "$app_ver" = "$ver" ] || fail "$app is version $app_ver, expected $ver"

archs="$(lipo -archs "$app/Contents/MacOS/dotlore")"
for arch in x86_64 arm64; do
	grep -qw "$arch" <<<"$archs" || fail "not a universal binary (archs: $archs); the $arch slice is missing"
done

sign_info="$(codesign -dv --verbose=2 "$app" 2>&1)" || fail "codesign cannot read the signature of $app"
grep -qxF "Authority=$identity" <<<"$sign_info" || fail "$app is not signed by $identity"
xcrun stapler validate "$app" || fail "$app has no valid stapled notarization ticket"
gatekeeper="$(spctl --assess --type execute -vv "$app" 2>&1)" || fail "Gatekeeper rejects $app: $gatekeeper"
grep -qF "Notarized Developer ID" <<<"$gatekeeper" || fail "$app is not notarized: $gatekeeper"

for f in "$app.tar.gz" "$app.tar.gz.sig"; do
	[ -s "$f" ] || fail "missing or empty updater artifact: $f (is bundle.createUpdaterArtifacts still true?)"
	# The target dir persists between builds, so an old tarball would still pass -s.
	[ "$f" -nt "$app/Contents/Info.plist" ] || fail "$f is older than $app; it is from an earlier build (is bundle.createUpdaterArtifacts still true?)"
done

shopt -s nullglob
dmgs=("$bundle"/dmg/*.dmg)
shopt -u nullglob
[ "${#dmgs[@]}" -eq 1 ] || fail "expected exactly one dmg in $bundle/dmg, found ${#dmgs[@]}"
dmg="${dmgs[0]}"
[[ "$(basename "$dmg")" == *"_${ver}_"* ]] || fail "$dmg does not carry version $ver"

# The updater manifest. Version and tag come from --expect-version, which the
# Cargo.toml check above already matched.
python3 - "$ver" "$tag" "$repo" "$app.tar.gz.sig" >"$tmp/latest.json" <<'PY'
import datetime, json, sys

version, tag, repo, sig_path = sys.argv[1:5]
signature = open(sig_path).read().strip()
url = f"https://github.com/{repo}/releases/download/{tag}/Dotlore.app.tar.gz"

# The updater looks up "{target}-{arch}", so BOTH darwin keys must be
# present even though one universal tarball serves them. darwin-universal
# is harmless and is what newer clients prefer.
entry = {"signature": signature, "url": url}
print(json.dumps({
    "version": version,
    "pub_date": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "platforms": {
        "darwin-aarch64": entry,
        "darwin-x86_64": entry,
        "darwin-universal": entry,
    },
}, indent=2))
PY

python3 - "$tmp/latest.json" "$ver" <<'PY'
import base64, json, sys

manifest = json.load(open(sys.argv[1]))
expected = sys.argv[2]
if manifest["version"] != expected:
    sys.exit(f"error: latest.json version {manifest['version']!r} != tag version {expected!r}")
for key in ("darwin-aarch64", "darwin-x86_64"):
    platform = manifest["platforms"].get(key)
    if not platform:
        sys.exit(f"error: latest.json has no {key} entry — the updater would never resolve an update for it.")
    signature = platform.get("signature", "").strip()
    if not signature:
        sys.exit(f"error: latest.json {key} has an empty signature — every client would silently refuse the update.")
    # The client base64-decodes this string and parses the result as a
    # minisign signature file. Putting the raw file in instead of its
    # base64 is the shape mistake that still LOOKS like a signature in
    # the manifest, and it fails on every client, not in CI.
    try:
        decoded = base64.b64decode(signature, validate=True).decode()
    except Exception as e:
        sys.exit(f"error: latest.json {key} signature is not valid base64 ({e}) — it must be the .sig file's contents verbatim.")
    if not decoded.lstrip().startswith("untrusted comment:"):
        sys.exit(f"error: latest.json {key} signature does not decode to a minisign signature file.")
    if not platform.get("url", "").startswith("https://"):
        sys.exit(f"error: latest.json {key} has no https url.")

# Informational, never fatal. plugins.updater.requireSignedVersion is
# left unset (defaults false) until this line proves the bundler stamps
# a version into the signature's trusted comment on this toolchain.
# Turning it on without that would strand every install, exactly the way
# a wrong pubkey would. Read this in the first release's log, then decide.
trusted = base64.b64decode(
    manifest["platforms"]["darwin-aarch64"]["signature"]).decode()
stamped = [ln for ln in trusted.splitlines() if ln.startswith("trusted comment:")]
print(f"trusted comment: {stamped[0] if stamped else '(none)'}")
print("requireSignedVersion is safe to enable"
      if stamped and "version:" in stamped[0]
      else "requireSignedVersion must stay off — no version: in the trusted comment")

print("latest.json asserted: both darwin keys present, signatures decode as minisign.")
PY

if [ "$mode" = "build-only" ]; then
	cp "$tmp/latest.json" "$bundle/latest.json"
	cat "$bundle/latest.json"
	echo "build only: every check passed for $tag"
	for f in "$app" "$app.tar.gz" "$app.tar.gz.sig" "$dmg" "$bundle/latest.json"; do
		echo "artifact: $f"
	done
	exit 0
fi

# --- tag --------------------------------------------------------------------------
if [ "$mode" = "release" ]; then
	[ "$(git rev-parse HEAD)" = "$release_sha" ] && [ -z "$(git status --porcelain)" ] ||
		fail "source changed during the build; nothing was published"
	git fetch -q origin
	[ "$(git rev-parse origin/main)" = "$release_sha" ] || fail "origin/main moved during the build; nothing was published"
	if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
		fail "tag $tag appeared on origin during the build; nothing was published"
	fi
	echo "==> tagging $tag at $release_sha"
	git tag "$tag" "$release_sha"
	git push origin "refs/tags/$tag"
	tag_pushed=1
fi

# --- publish ----------------------------------------------------------------------
# Draft first, upload, publish last: the release is never visible half-uploaded,
# so latest.json never lands before the tarball it points at.
echo "==> publishing $tag"
# A leftover DRAFT from an earlier failed run is cleared. The tag is never
# deleted: --cleanup-tag is deliberately not passed.
if [ "$(gh release view "$tag" --json isDraft -q .isDraft 2>/dev/null || echo none)" = "true" ]; then
	echo "deleting a leftover draft release of $tag"
	gh release delete "$tag" --yes
fi
gh release create "$tag" --draft --verify-tag --title "Dotlore v$ver" --notes-file "$tmp/notes.md"
# The version-less alias the website's Download button links.
cp "$dmg" "$tmp/Dotlore-universal.dmg"
gh release upload "$tag" "$dmg" "$tmp/Dotlore-universal.dmg" "$app.tar.gz" "$app.tar.gz.sig" "$tmp/latest.json"
gh release edit "$tag" --draft=false

# /releases/latest serves neither drafts nor prereleases.
state="$(gh release view "$tag" --json isDraft,isPrerelease -q '"\(.isDraft) \(.isPrerelease)"')"
[ "$state" = "false false" ] || fail "release $tag is not a published stable release (isDraft isPrerelease: $state)"
latest="$(gh release view --json tagName -q .tagName)"
[ "$latest" = "$tag" ] || fail "GitHub's latest release is $latest, not $tag; clients would not see the update"

gh workflow run pages.yml --ref main ||
	echo "warning: could not dispatch the website deploy. Run: gh workflow run pages.yml --ref main" >&2

echo "release: $(gh release view "$tag" --json url -q .url)"
