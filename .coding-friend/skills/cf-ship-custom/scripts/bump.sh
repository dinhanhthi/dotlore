#!/usr/bin/env bash
# bump.sh — set the app version across every file that carries it.
#
# Usage: bash bump.sh <new_version>
#   e.g. bash bump.sh 0.2.0
#
# THREE files, not two. Forgetting Cargo.lock leaves `cargo build --locked`
# and CI drifting against Cargo.toml.
#
# Plain X.Y.Z only. Prerelease suffixes (`-beta.N`, `-rc.N`) are rejected
# outright: Dotlore ships stable only, and the channel mechanism that would
# give such a tag a meaning is not being built. A tag carrying a suffix would
# be a release nothing can resolve.
#
# Leading zeros are rejected for the same reason, one step further on: `01.2.3`
# compares as (1, 2, 3) here, so it would sail past the "strictly greater" check
# and be written into the manifest — and then be refused by Cargo's parser, by
# the tag shape and by bump-info.sh's reader. Three numeric fields means exactly
# the shape `(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)`.
#
# It must also be strictly greater than the version already in
# src-tauri/Cargo.toml. The version is the one public value that is only
# fixable forward, so a downgrade or a re-run of the current version is refused
# rather than written backwards into the manifest, the lock entry and the badge.
#
# Every target is validated before the first write, so a guard that trips
# leaves the tree untouched instead of half-bumped.
#
# Nothing JavaScript carries a version, on purpose: the root package.json has
# no `version` field and the app crate's own package.json is gone, so adding
# `bump_json` back would manufacture exactly the second source of truth this
# pipeline rejects. That is also why there is no `pnpm exec prettier` step —
# the only JSON formatter call ever needed was for those two files.
#
# src-tauri/tauri.conf.json is deliberately left alone: it has no `version`
# key, and the bundler resolves the version from the [package] version in
# src-tauri/Cargo.toml.

set -euo pipefail

# scripts -> cf-ship-custom -> skills -> .coding-friend -> repo root = four levels.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
NEW_VERSION="${1:-}"

if [[ -z "$NEW_VERSION" ]]; then
  echo "Usage: bash bump.sh <new_version>    e.g. 0.2.0"
  exit 1
fi

# Core semver only, three numeric fields, no leading zeros. Deliberately narrower
# than full semver: everything downstream — the git tag, release.yml, the updater
# — treats a suffix as a different channel, and there is only one channel here.
# Leading zeros are the same class of mistake: `01.2.3` parses as (1, 2, 3) three
# lines below, so it would be accepted as strictly greater than `0.1.0`, written
# into the manifest, and then rejected by Cargo's own parser and by bump-info.sh's
# reader — a version every downstream reader refuses, tagged and unfixable except
# forward.
if ! [[ "$NEW_VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo "Error: version must be X.Y.Z, with no leading zeros and no prerelease"
  echo "suffix (got '$NEW_VERSION')"
  exit 1
fi

cd "$REPO_ROOT"

# Guard the path arithmetic above rather than letting a wrong REPO_ROOT surface
# as a bare FileNotFoundError from python three functions later.
for required in src-tauri/Cargo.toml website/index.html; do
  if [[ ! -f "$required" ]]; then
    echo "Error: $required not found under REPO_ROOT=$REPO_ROOT"
    echo "The relative path from this script to the repository root is wrong."
    exit 1
  fi
done

# ─── Pre-validation: every target, before the first write ─────────────────────
#
# bump_cargo_toml used to run before bump_web_badge, so a badge guard tripping
# left src-tauri/Cargo.toml already bumped with the lockfile and the badge stale
# — the exact silent partial bump this script exists to prevent. Everything that
# can be checked without writing is therefore checked here, first: the version
# moves strictly forward, the manifest has a [package] version line, and the
# website markers exist, are in order, and wrap the badge.

validate_targets() {
  python3 - "$NEW_VERSION" <<'PY'
import re, sys

new_version = sys.argv[1]
cargo, html = "src-tauri/Cargo.toml", "website/index.html"


def fail(message):
    sys.exit("Error: " + message)


def triple(version):
    return tuple(int(n) for n in version.split("."))


# ── src-tauri/Cargo.toml: a [package] section carrying a version line ──
src = open(cargo).read()
if "[package]" not in src:
    fail("%s has no [package] section, so there is no version to bump." % cargo)
start = src.index("[package]")
end = src.find("\n[", start + 1)
body = src[start:end if end != -1 else len(src)]
found = re.search(r'(?m)^version\s*=\s*"([^"]*)"', body)
if not found:
    fail('%s has no `version = "..."` line under [package].' % cargo)
current = found.group(1)

# The version is the one public value that is only fixable forward. A downgrade,
# or a re-run of the version already in the file, would rewrite the manifest,
# the lock entry and the badge backwards, and the next bump-info.sh run would
# report BROKEN-tag-ahead-of-file. Refuse anything that does not move forward.
#
# The same shape the guard above enforces, applied to the version already in the
# file: `01.2.3` would otherwise pass and be compared as (1, 2, 3), so a
# leading-zero manifest would silently accept a version below its own printed
# one. bump-info.sh reads the file with this shape too, so a manifest that fails
# here is one that cannot report a state at all on the next run.
if not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", current):
    fail(
        "the version in %s is %r, which is not X.Y.Z — three numeric fields, no\n"
        "leading zeros, no prerelease suffix — fix the file first." % (cargo, current)
    )
if triple(new_version) <= triple(current):
    fail(
        "%s already carries %s, and %s is not greater than it. A version only\n"
        "moves forward: pass a higher one. (Current version: %s)"
        % (cargo, current, new_version, current)
    )

# ── website/index.html: exactly one marker pair, wrapping the badge ──
html_src = open(html).read()
opens = list(re.finditer(r"(?m)^([ \t]*)<!-- dotlore:version -->[ \t]*$", html_src))
closes = list(re.finditer(r"(?m)^[ \t]*<!-- /dotlore:version -->[ \t]*$", html_src))
if len(opens) != 1 or len(closes) != 1:
    fail(
        "%s must carry exactly one <!-- dotlore:version --> and one\n"
        "<!-- /dotlore:version --> (found %d and %d). The version badge cannot\n"
        "be located, so the page would keep advertising the old release."
        % (html, len(opens), len(closes))
    )
if closes[0].start() < opens[0].end():
    fail("the /dotlore:version marker precedes its opener in %s" % html)
if "version-badge" not in html_src[opens[0].start():closes[0].end()]:
    fail(
        "the dotlore:version markers in %s do not wrap the version badge —\n"
        "refusing to rewrite an element this script does not own." % html
    )

print("  Pre-validated: %s carries %s, and %s carries one marker pair around" % (cargo, current, html))
print("                 the version badge. Nothing has been written yet.")
PY
}

# ─── TOML: src-tauri/Cargo.toml ───────────────────────────────────────────────
#
# Only the `version` under [package] — never a dependency's version. The range
# is bounded by the next section header so `tauri = { version = "2" }` further
# down is untouched.

bump_cargo_toml() {
  local file="src-tauri/Cargo.toml"
  python3 - "$file" "$NEW_VERSION" <<'PY'
import re, sys
path, version = sys.argv[1], sys.argv[2]
src = open(path).read()

start = src.index("[package]")
end = src.find("\n[", start + 1)
if end == -1:
    end = len(src)
head, body, tail = src[:start], src[start:end], src[end:]

new_body, n = re.subn(
    r'(?m)^version\s*=\s*"[^"]*"', f'version = "{version}"', body, count=1
)
if n != 1:
    sys.exit(f"Error: no version line found in [package] of {path}")

open(path, "w").write(head + new_body + tail)
print(f"  {path}: -> {version}")
PY
}

# ─── HTML: the website version badge ──────────────────────────────────────────
#
# The two markers wrap the whole <a class="version-badge"> element, not just its
# label, because the element carries the version twice: once in the
# releases/tag/vX.Y.Z href and once in the visible text. Rewriting only the text
# would leave the badge reading v0.1.1 while linking to the v0.1.0 tag, so the
# whole element is regenerated with both copies updated.
#
# A missing marker is a hard error, not a skip: a silent no-op here ships a page
# advertising the previous release, which is the failure this script exists to
# prevent. Both markers must survive the rewrite.

bump_web_badge() {
  local file="website/index.html"
  python3 - "$file" "$NEW_VERSION" <<'PY'
import re, sys

path, version = sys.argv[1], sys.argv[2]
src = open(path).read()

open_tag = re.compile(r"(?m)^([ \t]*)<!-- dotlore:version -->[ \t]*$")
close_tag = re.compile(r"(?m)^[ \t]*<!-- /dotlore:version -->[ \t]*$")

opens, closes = list(open_tag.finditer(src)), list(close_tag.finditer(src))
if len(opens) != 1 or len(closes) != 1:
    sys.exit(
        "Error: %s must carry exactly one <!-- dotlore:version --> and one\n"
        "<!-- /dotlore:version --> (found %d and %d). The version badge cannot\n"
        "be located, so the page would keep advertising the old release."
        % (path, len(opens), len(closes))
    )

start, end = opens[0], closes[0]
if end.start() < start.end():
    sys.exit("Error: the /dotlore:version marker precedes its opener in %s" % path)

# Bounds the element to the badge itself: the markers have been moved once
# already, and a pair left around some other element would quietly rewrite it.
old = src[start.start():end.end()]
if "version-badge" not in old:
    sys.exit(
        "Error: the dotlore:version markers in %s do not wrap the version badge —\n"
        "refusing to rewrite an element this script does not own." % path
    )

indent = start.group(1)
block = "\n".join([
    "{i}<!-- dotlore:version -->",
    "{i}<a",
    '{i}  class="version-badge"',
    '{i}  href="https://github.com/dinhanhthi/dotlore/releases/tag/v{v}"',
    '{i}  target="_blank"',
    '{i}  rel="noopener noreferrer"',
    "{i}>",
    "{i}  v{v}",
    "{i}</a>",
    "{i}<!-- /dotlore:version -->",
]).format(i=indent, v=version)

open(path, "w").write(src[:start.start()] + block + src[end.end():])
print("  %s: -> v%s" % (path, version))
PY
}

# Printed on any failure AFTER the first write. A mid-run abort — `cargo update`
# failing, a writer's own guard tripping — leaves the same partial state a
# verification failure does, so both paths print the same hint. Before this
# existed, only the verification path printed it, and the mid-run path printed
# nothing at all.
partial_bump_hint() {
  local status=$?
  if [[ "$status" -ne 0 ]]; then
    echo ""
    echo "Nothing was reverted — the version files may be partially bumped."
    echo "Inspect with:  git diff"
    echo "Restore with:  git checkout -- src-tauri/Cargo.toml src-tauri/Cargo.lock website/index.html"
  fi
  return 0
}

validate_targets
echo "Bumping Dotlore to ${NEW_VERSION}…"
trap partial_bump_hint EXIT
bump_cargo_toml
bump_web_badge

# ─── Cargo.lock ───────────────────────────────────────────────────────────────
#
# `cargo update -p dotlore` rewrites only this package's own entry, and it needs
# Cargo.toml already bumped, which is why it runs last. It is also the reason
# the manifest is bumped before anything else: run against the old manifest, it
# would rewrite the lock back to the old version.

echo "  src-tauri/Cargo.lock:"
cargo update -p dotlore --manifest-path src-tauri/Cargo.toml 2>&1 | sed 's/^/    /'

# ─── Verify all three agree ───────────────────────────────────────────────────
#
# A silent partial bump is the failure mode worth guarding: CI checks the tag
# against src-tauri/Cargo.toml only, so a stale Cargo.lock or a stale website
# badge would sail past it — one breaking `cargo build --locked`, the other
# shipping a page pointing at the previous release.

echo ""
echo "Verifying:"
FAILED=0
check() {
  local label="$1" actual="$2"
  if [[ "$actual" == "$NEW_VERSION" ]]; then
    echo "  ok    $label = $actual"
  else
    echo "  FAIL  $label = $actual (expected $NEW_VERSION)"
    FAILED=1
  fi
}

check "Cargo.toml" \
  "$(python3 -c '
import re
src = open("src-tauri/Cargo.toml").read()
pkg = src[src.index("[package]"):]
end = pkg.find("\n[", 1)
print(re.search(r"(?m)^version\s*=\s*\"([^\"]*)\"", pkg[:end if end != -1 else None]).group(1))
')"
check "Cargo.lock" \
  "$(python3 -c '
import re
src = open("src-tauri/Cargo.lock").read()
m = re.search(r"(?ms)^\[\[package\]\]\nname = \"dotlore\"\nversion = \"([^\"]*)\"", src)
print(m.group(1) if m else "NOT FOUND")
')"
# Prints the badge's version when the href and the label agree, so comparing the
# one value against NEW_VERSION covers both copies at once. The label pattern
# tolerates whitespace — `>v0.1.1</a>` and a version on its own line both match —
# because requiring the exact shape this script emits turns any hand-edit into a
# false mismatch. The href-vs-label agreement is the load-bearing assertion.
check "website badge" \
  "$(python3 -c '
import re
src = open("website/index.html").read()
m = re.search(r"(?ms)^[ \t]*<!-- dotlore:version -->(.*?)^[ \t]*<!-- /dotlore:version -->", src)
if not m:
    print("MARKERS MISSING")
else:
    block = m.group(1)
    href = re.search(r"releases/tag/v([0-9]+\.[0-9]+\.[0-9]+)", block)
    label = re.search(r">\s*v([0-9]+\.[0-9]+\.[0-9]+)\s*</a>", block)
    h = href.group(1) if href else "?"
    l = label.group(1) if label else "?"
    print(h if h == l else "href v%s != label v%s" % (h, l))
')"

if [[ "$FAILED" -ne 0 ]]; then
  echo ""
  echo "One or more files did not take the new version."
  exit 1
fi

echo ""
echo "Done. Next: update CHANGELOG.md (it becomes the GitHub Release body),"
echo "commit, then tag v$NEW_VERSION."
