#!/usr/bin/env bash
#
# Tests for scripts/reset-dev.sh. Isolates $HOME so WebKit / LaunchAgents
# never touch the real user directories, and stubs launchctl so --yes
# cannot bootout the live Dotlore LaunchAgent.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$root/scripts/reset-dev.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
assert_eq() {
	local got="$1" want="$2" msg="$3"
	if [ "$got" != "$want" ]; then
		fail "$msg: got $(printf %q "$got"), want $(printf %q "$want")"
	fi
}
assert_file() { [ -e "$1" ] || fail "expected to exist: $1"; }
assert_gone() { [ ! -e "$1" ] || fail "expected gone: $1"; }
# One output line must mention this path (removed or skipped / would-*).
assert_mentions() {
	local out="$1" path="$2"
	grep -F -q -- "$path" <<<"$out" || fail "output did not mention $path"$'\n'"$out"
}

[ -x "$script" ] || [ -f "$script" ] || fail "missing $script"

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

# --yes always calls `launchctl bootout gui/$(id -u)/…` against the host
# login domain. A stub on PATH is the only thing that keeps `pnpm test`
# from unloading a live agent.
bindir="$workdir/bin"
launchctl_log="$workdir/launchctl.log"
mkdir -p "$bindir"
cat >"$bindir/launchctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${LAUNCHCTL_LOG:?}"
exit 0
EOF
chmod +x "$bindir/launchctl"
export PATH="$bindir:$PATH"
export LAUNCHCTL_LOG="$launchctl_log"
: >"$launchctl_log"
case "$(command -v launchctl)" in
	"$bindir/launchctl") ;;
	*) fail "launchctl is not the stub: $(command -v launchctl)" ;;
esac

# ---------------------------------------------------------------------------
# dry-run without --yes prints what would be deleted and exits 0
# ---------------------------------------------------------------------------
home1="$workdir/home1"
state1="$workdir/state1"
cloud1="$workdir/cloud1"
mkdir -p "$home1/Library/WebKit/dotlore"
mkdir -p "$home1/Library/LaunchAgents"
mkdir -p "$state1/repos/demo" "$state1/tmp" "$state1/recovery"
printf 'x' >"$state1/config.json"
printf 'x' >"$state1/lock"
printf 'x' >"$state1/app.lock"
printf 'x' >"$state1/repos/demo/HEAD"
printf 'x' >"$home1/Library/WebKit/dotlore/localstorage"
printf 'x' >"$home1/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"
mkdir -p "$cloud1/dotlore/demo"
printf '{"slug":"demo"}' >"$cloud1/dotlore/demo/manifest.json"
cat >"$state1/config.json" <<EOF
{
  "device_id": "ab",
  "device_name": "m",
  "provider_dir": "$cloud1",
  "roots": []
}
EOF

set +e
out1=$(HOME="$home1" DOTLORE_HOME="$state1" bash "$script" 2>&1)
ec1=$?
set -e
assert_eq "$ec1" "0" "dry-run exit status"
assert_file "$state1/config.json"
assert_file "$state1/lock"
assert_file "$state1/app.lock"
assert_file "$state1/repos/demo/HEAD"
assert_file "$state1/tmp"
assert_file "$state1/recovery"
assert_file "$cloud1/dotlore/demo/manifest.json"
assert_file "$home1/Library/WebKit/dotlore/localstorage"
assert_file "$home1/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"
assert_mentions "$out1" "$cloud1/dotlore"
assert_mentions "$out1" "$state1/config.json"
assert_mentions "$out1" "$state1/lock"
assert_mentions "$out1" "$state1/app.lock"
assert_mentions "$out1" "$state1/tmp"
assert_mentions "$out1" "$state1/repos"
assert_mentions "$out1" "$state1/recovery"
assert_mentions "$out1" "$home1/Library/WebKit/dotlore"
assert_mentions "$out1" "$home1/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"
if grep -Eiq 'clean reset|reset complete|reset succeeded' <<<"$out1"; then
	fail "dry-run claimed a reset it did not perform"$'\n'"$out1"
fi
# Dry-run must not say it removed anything.
if grep -E '^removed ' <<<"$out1" >/dev/null; then
	fail "dry-run printed a 'removed' line"$'\n'"$out1"
fi
[ ! -s "$launchctl_log" ] || fail "dry-run called launchctl"$'\n'"$(cat "$launchctl_log")"
echo "ok: dry-run prints paths and deletes nothing"

# ---------------------------------------------------------------------------
# --yes deletes listed paths, one line per actually-removed path
# ---------------------------------------------------------------------------
set +e
out2=$(HOME="$home1" DOTLORE_HOME="$state1" bash "$script" --yes 2>&1)
ec2=$?
set -e
assert_eq "$ec2" "0" "--yes exit status"
assert_gone "$state1/config.json"
assert_gone "$state1/lock"
assert_gone "$state1/app.lock"
assert_gone "$state1/tmp"
assert_gone "$state1/repos"
assert_gone "$state1/recovery"
assert_gone "$cloud1/dotlore"
assert_file "$cloud1" # never the provider folder itself
assert_gone "$home1/Library/WebKit/dotlore"
assert_gone "$home1/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"
for p in \
	"$cloud1/dotlore" \
	"$state1/config.json" \
	"$state1/lock" \
	"$state1/app.lock" \
	"$state1/tmp" \
	"$state1/repos" \
	"$state1/recovery" \
	"$home1/Library/WebKit/dotlore" \
	"$home1/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"; do
	line=$(grep -F -- "$p" <<<"$out2" || true)
	[ -n "$line" ] || fail "--yes output missing line for $p"$'\n'"$out2"
	grep -Eq '^removed ' <<<"$line" || fail "expected 'removed' line for $p"$'\n'"$line"
done
# One line per path — no extras claiming a clean reset.
if grep -Eiq 'clean reset' <<<"$out2"; then
	fail "--yes claimed a clean reset"$'\n'"$out2"
fi
# Must not invent a WebKit path under DOTLORE_HOME.
if grep -F -- "$state1/Library/WebKit" <<<"$out2" >/dev/null; then
	fail "invented WebKit path under DOTLORE_HOME"$'\n'"$out2"
fi
grep -Eq '^bootout gui/[0-9]+/dev\.dinhanhthi\.dotlore$' "$launchctl_log" \
	|| fail "--yes did not call the stub launchctl bootout"$'\n'"$(cat "$launchctl_log")"
echo "ok: --yes removes each listed path"

# ---------------------------------------------------------------------------
# provider_dir null: skip cloud, do not guess, still reset home state
# ---------------------------------------------------------------------------
home3="$workdir/home3"
state3="$workdir/state3"
mkdir -p "$home3" "$state3"
cat >"$state3/config.json" <<'EOF'
{
  "device_id": "ab",
  "device_name": "m",
  "provider_dir": null,
  "roots": []
}
EOF
set +e
out3=$(HOME="$home3" DOTLORE_HOME="$state3" bash "$script" --yes 2>&1)
ec3=$?
set -e
assert_eq "$ec3" "0" "null provider_dir exit status"
assert_gone "$state3/config.json"
grep -Eiq 'skip' <<<"$out3" || fail "null provider_dir did not print a skip"$'\n'"$out3"
grep -Eiq 'provider_dir' <<<"$out3" || fail "null provider_dir skip did not name the field"$'\n'"$out3"
if grep -Eiq 'clean reset' <<<"$out3"; then
	fail "null provider_dir claimed a clean reset"$'\n'"$out3"
fi
echo "ok: null provider_dir is skipped, not guessed"

# ---------------------------------------------------------------------------
# provider_dir absent: same refusal
# ---------------------------------------------------------------------------
home4="$workdir/home4"
state4="$workdir/state4"
mkdir -p "$home4" "$state4"
cat >"$state4/config.json" <<'EOF'
{
  "device_id": "ab",
  "device_name": "m",
  "roots": []
}
EOF
set +e
out4=$(HOME="$home4" DOTLORE_HOME="$state4" bash "$script" --yes 2>&1)
ec4=$?
set -e
assert_eq "$ec4" "0" "absent provider_dir exit status"
assert_gone "$state4/config.json"
grep -Eiq 'skip' <<<"$out4" || fail "absent provider_dir did not print a skip"$'\n'"$out4"
grep -Eiq 'provider_dir' <<<"$out4" || fail "absent provider_dir skip did not name the field"$'\n'"$out4"
echo "ok: absent provider_dir is skipped, not guessed"

# ---------------------------------------------------------------------------
# missing config.json: skip cloud, skip the missing home paths
# ---------------------------------------------------------------------------
home5="$workdir/home5"
state5="$workdir/state5"
mkdir -p "$home5" "$state5"
set +e
out5=$(HOME="$home5" DOTLORE_HOME="$state5" bash "$script" --yes 2>&1)
ec5=$?
set -e
assert_eq "$ec5" "0" "missing config exit status"
grep -Eiq 'skip' <<<"$out5" || fail "missing config did not print a skip"$'\n'"$out5"
if grep -Eiq 'clean reset' <<<"$out5"; then
	fail "missing config claimed a clean reset"$'\n'"$out5"
fi
# Every candidate home path should have a skipped line (nothing existed).
for p in config.json lock app.lock tmp repos recovery; do
	grep -F -q -- "$state5/$p" <<<"$out5" || fail "missing a skip line for $state5/$p"$'\n'"$out5"
done
echo "ok: missing config.json skips cloud and absent home paths"

# ---------------------------------------------------------------------------
# WebKit is always under \$HOME, never under DOTLORE_HOME
# ---------------------------------------------------------------------------
home6="$workdir/home6"
state6="$workdir/state6"
mkdir -p "$home6/Library/WebKit/dotlore"
mkdir -p "$state6/Library/WebKit/dotlore"
printf 'real' >"$home6/Library/WebKit/dotlore/store"
printf 'fake' >"$state6/Library/WebKit/dotlore/store"
cat >"$state6/config.json" <<'EOF'
{"device_id":"ab","device_name":"m","provider_dir":null,"roots":[]}
EOF
set +e
out6=$(HOME="$home6" DOTLORE_HOME="$state6" bash "$script" --yes 2>&1)
ec6=$?
set -e
assert_eq "$ec6" "0" "webkit path exit status"
assert_gone "$home6/Library/WebKit/dotlore"
assert_file "$state6/Library/WebKit/dotlore/store"
if grep -F -- "$state6/Library/WebKit" <<<"$out6" >/dev/null; then
	fail "mentioned WebKit under DOTLORE_HOME"$'\n'"$out6"
fi
echo "ok: WebKit path is \$HOME, not DOTLORE_HOME"

echo "all tests passed"
