#!/usr/bin/env bash
# Builds the playground for Android, installs it on an emulator or device, and proves that it runs on the real
# platform adapters of `android-adapters` (no faked port): every tab is launched and screenshotted, a note written on the
# Notes tab survives a killed process (SQLite through the `Db` port), then the Remote tab's offline story is driven through
# uiautomator:
#
#   1. fetch      the `inbox` list is fetched and persisted to Kv (files/undra/playground_core/kv: per core namespace)
#   2. toggle     the Offline switch goes on; an added item is queued by the core (the request fails with a network error)
#   3. kill       the app process is killed with the queue unsent
#   4. airplane   REAL airplane mode goes on; the app is relaunched: the persisted query shows the cached list (and the
#                 time of its fetch before the kill) while offline, and the core, told by the Connectivity adapter that
#                 there is no network, holds the persisted queue instead of sending it
#   5. replay     airplane mode goes off: the Connectivity adapter reports online and the core replays the queue; the
#                 server receives the Idempotency-Key it generated before the kill, and the item appears
#
#   examples/playground/android/smoke.sh             everything; the log is also written to ../.proof/android-adapters-smoke.log
#                                                     (and screenshots to ../.proof/android-adapters-*.png)
#   SKIP_CORE=1 examples/playground/android/smoke.sh reuse build/android/jniLibs instead of running `undra build`
#   ANDROID_SERIAL=emulator-5554 ...                  pick the device (needed when several are attached)
#
# Needs the Android SDK and NDK (`source scripts/env.sh`), a booted emulator or device, and the undra CLI
# (`cargo build -p undra-cli`). It changes the device's airplane mode and always switches it off again on exit.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT="$(cd "$HERE/.." && pwd)"
REPO="$(cd "$PROJECT/../.." && pwd)"
PROOF="$PROJECT/.proof"
UNDRA="${UNDRA:-$REPO/target/debug/undra}"
APP="dev.undra.playground"
ACTIVITY="$APP/.MainActivity"
WORK="$(mktemp -d)"
trap 'cleanup' EXIT

if [ -z "${ANDROID_HOME:-}" ] && [ -f "$REPO/scripts/env.sh" ]; then
  # shellcheck disable=SC1091
  source "$REPO/scripts/env.sh" >/dev/null 2>&1 || true
fi
export PATH="${ANDROID_HOME:-}/platform-tools:$HOME/.cargo/bin:$PATH"
ADB=(adb)
[ -n "${ANDROID_SERIAL:-}" ] && ADB=(adb -s "$ANDROID_SERIAL")

mkdir -p "$PROOF"
exec > >(tee "$PROOF/android-adapters-smoke.log") 2>&1

cleanup() {
  "${ADB[@]}" shell cmd connectivity airplane-mode disable >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
step() { printf '\n$ %s\n' "$*"; }
run() { step "$*"; "$@"; }
adb_() { "${ADB[@]}" "$@"; }

# ---- uiautomator helpers: controls are found by their test tags, which are also resource ids ----------------------
cat > "$WORK/ui.py" <<'PY'
import re, sys, xml.etree.ElementTree as ET

def nodes(path):
    return list(ET.parse(path).getroot().iter("node"))

def by_id(path, rid):
    return [n for n in nodes(path) if n.get("resource-id", "") == rid or n.get("resource-id", "").endswith("/" + rid)]

def center(node):
    x1, y1, x2, y2 = map(int, re.findall(r"\d+", node.get("bounds")))
    return (x1 + x2) // 2, (y1 + y2) // 2

cmd, path = sys.argv[1], sys.argv[2]
if cmd == "center":
    found = by_id(path, sys.argv[3])
    index = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    if index >= len(found):
        sys.exit(1)
    print(*center(found[index]))
elif cmd == "count":
    print(len(by_id(path, sys.argv[3])))
elif cmd == "text":
    found = by_id(path, sys.argv[3])
    if not found:
        sys.exit(1)
    print(found[0].get("text", ""))
elif cmd == "has-text":
    sys.exit(0 if any(n.get("text", "") == sys.argv[3] for n in nodes(path)) else 1)
elif cmd == "descendant-texts":
    texts = [n.get("text", "") for n in nodes(path) if n.get("text", "")]
    print("\n".join(texts))
PY
dump() { adb_ shell uiautomator dump /sdcard/undra-ui.xml >/dev/null 2>&1; adb_ exec-out cat /sdcard/undra-ui.xml > "$WORK/ui.xml"; }
ui() { dump; python3 "$WORK/ui.py" "$1" "$WORK/ui.xml" "${@:2}"; }
tap() {
  local at
  at="$(ui center "$@")" || { echo "no control tagged '$1'"; exit 1; }
  step "tap $1 ($at)"
  # shellcheck disable=SC2086
  adb_ shell input tap $at
}
type_into() {
  tap "$1"
  step "type '$2'"
  adb_ shell input text "$(printf '%s' "$2" | sed 's/ /%s/g')"
  adb_ shell input keyevent 4 # hide the keyboard
}
# wait_for SECONDS DESCRIPTION COMMAND...: polls until the command succeeds.
wait_for() {
  local seconds="$1" what="$2"; shift 2
  local i
  for ((i = 0; i < seconds * 2; i++)); do
    if "$@" >/dev/null 2>&1; then echo "ok: $what"; return 0; fi
    sleep 0.5
  done
  echo "TIMED OUT waiting for: $what"; return 1
}
rows_are() { [ "$(ui count remote-row)" = "$1" ]; }
status_is() { [ "$(ui text remote-status)" = "$1" ]; }
inbox_fetched() { rows_are 3 && status_is Success; }
item_queued() { [ "$(ui text remote-pending)" = queued ]; }
cached_list_shown() { rows_are 3 && [ "$(ui text remote-updated)" = "$fetched_at" ]; }
refresh_failed() { ui text remote-error | grep -q "network"; }
item_replayed() { rows_are 4 && ui has-text "Queued while offline"; }
device_offline() { ! adb_ shell dumpsys connectivity | grep -q "Active default network: [0-9]"; }
screenshot() { adb_ exec-out screencap -p > "$PROOF/android-adapters-$1.png"; echo "screenshot .proof/android-adapters-$1.png"; }
launch() { adb_ shell am force-stop "$APP"; adb_ logcat -c; run adb_ shell am start -W -n "$ACTIVITY" --es tab "$1"; }
# The default Kv is per core namespace (ADR-044 amendment A): the playground's core is `playground_core`.
KV_DIR="files/undra/playground_core/kv"
kv_files() { adb_ shell run-as "$APP" ls "$KV_DIR" 2>/dev/null | wc -l | tr -d ' '; }
# The app's own records: the demo server's request log (tag UndraDemoServer) and the adapters' (tag Undra).
logcat_app() { adb_ logcat -d -v brief 2>/dev/null | grep -E '^[VDIWEF]/(UndraDemoServer|Undra) *\(' || true; }
show_log() { logcat_app | grep -E "$1" | head -"${2:-6}" || true; }
first_key() { logcat_app | grep -o "dropped.*Idempotency-Key [0-9a-f-]*" | head -1 | grep -o "[0-9a-f-]\{36\}" || true; }

echo "# playground Android smoke run on the real platform adapters, $(date '+%Y-%m-%d %H:%M:%S')"

# ---- 0. the device ----------------------------------------------------------------------------------------------
step "adb shell getprop (device)"
adb_ get-state >/dev/null || { echo "no device: boot an emulator (see docs/ONBOARDING.md) or set ANDROID_SERIAL"; exit 1; }
adb_ shell getprop ro.product.model
adb_ shell getprop ro.product.cpu.abi
adb_ shell getprop ro.build.version.sdk

# ---- 1. the core, the app, install ----------------------------------------------------------------------------
if [ "${SKIP_CORE:-0}" != "1" ]; then
  [ -x "$UNDRA" ] || cargo build --manifest-path "$REPO/Cargo.toml" -p undra-cli
  run "$UNDRA" build -C "$PROJECT" --platform android --release
fi
step "(cd examples/playground/android && ./gradlew :app:assembleDebug)"
(cd "$HERE" && ./gradlew :app:assembleDebug -q 2>&1 | grep -v "SDK XML" || true)
APK="$HERE/app/build/outputs/apk/debug/app-debug.apk"
[ -f "$APK" ] || { echo "no APK at $APK"; exit 1; }
ls -l "$APK" | awk '{print $5 " bytes  " $9}'
run adb_ shell cmd connectivity airplane-mode disable
adb_ uninstall "$APP" >/dev/null 2>&1 || true
run adb_ install -r "$APK"
step "permissions the app holds (INTERNET and ACCESS_NETWORK_STATE come from the manifest and from android-adapters)"
adb_ shell dumpsys package "$APP" | grep -E "android.permission.(INTERNET|ACCESS_NETWORK_STATE)" | sort -u

# ---- 2. every tab launches ----------------------------------------------------------------------------------------
failed=0
for tab in todos counter biglist remote notes library feed ticker; do
  launch "$tab"
  sleep 4
  screenshot "tab-$tab"
  if adb_ shell pidof "$APP" >/dev/null; then echo "alive: $tab"; else echo "NOT RUNNING: $tab"; failed=1; fi
done
step "the adapters the app installed (logcat, tag Undra)"
logcat_app | grep "AndroidPlatformDefaults" || { echo "no AndroidPlatformDefaults line"; failed=1; }

# ---- 2b. Notes: SQLite through the Db port, across a killed process -------------------------------------------------
echo
echo "## Notes: a note written to SQLite (AndroidDbAdapter) is still there after the process is killed"
adb_ shell pm clear "$APP" >/dev/null
launch notes
notes_ready() { ui has-text "No notes yet. They are kept in SQLite, so they are still here after a restart."; }
notes_are() { [ "$(ui count notes-row)" = "$1" ]; }
note_kept() { notes_are 1 && ui has-text "Milk from smoke"; }
wait_for 20 "the database is open (migrated, empty)" notes_ready || failed=1
type_into notes-input "Milk from smoke"
tap notes-add
wait_for 15 "the note is in the list" note_kept || failed=1
tap notes-toggle
sleep 1
screenshot notes-added
step "kill the app; relaunch on the Notes tab"
run adb_ shell am force-stop "$APP"
launch notes
wait_for 20 "the note came back from SQLite after the restart" note_kept || failed=1
echo "database files (run-as $APP ls databases): $(adb_ shell run-as "$APP" ls databases 2>/dev/null | tr '\n' ' ')"
# Per core namespace (ADR-044 amendment A): undra-<namespace>-<database>.sqlite, the core being playground_core.
DB_FILE="undra-playground_core-playground.sqlite"
adb_ shell run-as "$APP" ls databases 2>/dev/null | grep -qF "$DB_FILE" || { echo "no $DB_FILE"; failed=1; }
screenshot notes-restarted

# ---- 2c. the paging screens (ADR-043): Library (lazy lists), Feed (an infinite query), Ticker (a polled query) ---------------
echo
echo "## Library, Feed, Ticker: lazy lists, an infinite query and polling on the real core"
library_ready() { [ "$(ui text library-count)" = "10,000 books" ] && [ "$(ui count library-book)" -ge 5 ]; }
evens_ready() { ui has-text "Evens: a view of the even rows of the source list (10)" && [ "$(ui count library-even)" -ge 1 ]; }
library_grew() { [ "$(ui text library-count)" = "10,100 books" ]; }
last_book_shown() { ui has-text "Item 10100"; }
feed_rows() { ui text feed-count | tr -d ',' | cut -d' ' -f1; }
feed_first_page() { [ "$(feed_rows)" = 50 ]; }
feed_grew() { [ "$(feed_rows)" -ge 100 ]; }
feed_refreshed() { [ "$(ui text feed-status)" = "Success" ]; }
counter_value() { ui text ticker-count; }
ticker_started() { [ "$(counter_value)" -ge 1 ]; }
ticker_polled() { [ "$(counter_value)" -gt "$first_tick" ]; }
ticker_failed() { [ "$(ui count ticker-error)" -ge 1 ]; }
ticker_recovered() { [ "$(ui count ticker-error)" = 0 ]; }
ticker_on_screen() { [ "$(ui count ticker-count)" -ge 1 ]; }
ticker_after_resume() { [ "$(counter_value)" -gt "$resumed" ]; }
adb_ shell pm clear "$APP" >/dev/null
launch library
wait_for 20 "the library has 10,000 books and its first pages (rows, not placeholders)" library_ready || failed=1
wait_for 10 "the evens view has ten rows" evens_ready || failed=1
screenshot library-loaded
tap library-add
wait_for 10 "the library has 10,100 books" library_grew || failed=1
tap library-end
wait_for 10 "the last page was fetched: the last book is on screen" last_book_shown || failed=1
screenshot library-end
launch feed
wait_for 20 "the feed has its first page (50 rows)" feed_first_page || failed=1
screenshot feed-first-page
for _ in 1 2 3 4 5 6 7 8; do adb_ shell input swipe 540 1700 540 400 150; done
wait_for 20 "scrolling to the end fetched the next page (100 rows or more)" feed_grew || failed=1
screenshot feed-scrolled
tap feed-refresh
wait_for 10 "the refresh ended (status Success)" feed_refreshed || failed=1
launch ticker
wait_for 20 "the ticker has a first value" ticker_started || failed=1
first_tick="$(counter_value)"
wait_for 10 "the counter went up by itself (a poll, a second after each fetch)" ticker_polled || failed=1
echo "ticker-count: $first_tick -> $(counter_value)"
tap ticker-failing
wait_for 10 "a failing fetch shows its error and the polling goes on" ticker_failed || failed=1
tap ticker-failing
wait_for 10 "the error clears when a fetch succeeds again" ticker_recovered || failed=1
screenshot ticker
step "the app goes to the background (the core stops polling) and comes back (it polls again)"
adb_ shell input keyevent KEYCODE_HOME
sleep 4
run adb_ shell am start -W -n "$ACTIVITY" --es tab ticker
wait_for 10 "the ticker is on screen again" ticker_on_screen || failed=1
resumed="$(counter_value)"
wait_for 10 "the counter goes up again after the app came back" ticker_after_resume || failed=1

# ---- 3. the offline story on the Remote tab -----------------------------------------------------------------------
echo
echo "## Remote: persisted query, offline toggle, queue, kill, real airplane mode, replay"
adb_ shell pm clear "$APP" >/dev/null
launch remote
wait_for 20 "the inbox list is fetched (3 rows, status Success)" inbox_fetched || failed=1
fetched_at="$(ui text remote-updated || true)"
echo "remote-updated: $fetched_at"
sleep 1 # the persisted copy is written 250 ms after the fetch
screenshot remote-fetched
echo "files in the Kv directory (run-as $APP ls $KV_DIR): $(kv_files)"

step "1. the Offline switch simulates losing the network; add an item"
tap remote-offline
type_into remote-input "Queued while offline"
tap remote-add
wait_for 15 "the item shows as queued" item_queued || failed=1
screenshot remote-queued
sleep 1
echo "files in the Kv directory after queueing: $(kv_files)  (the cache entry and the offline queue)"
key_before="$(first_key)"
echo "Idempotency-Key of the dropped request: ${key_before:-NOT FOUND}"
show_log "dropped"

step "2. kill the app with the queue unsent"
run adb_ shell am force-stop "$APP"
adb_ shell pidof "$APP" >/dev/null && { echo "still running"; failed=1; } || echo "process gone"

step "3. real airplane mode on, relaunch: the persisted query answers from Kv, the persisted queue is held"
run adb_ shell cmd connectivity airplane-mode enable
wait_for 20 "the device has no network" device_offline || failed=1
adb_ logcat -c
run adb_ shell am start -W -n "$ACTIVITY" --es tab remote
wait_for 20 "the cached list is shown from Kv while offline (3 rows, the time of the fetch before the kill)" cached_list_shown || failed=1
echo "remote-updated: $(ui text remote-updated || true)   (before the kill: $fetched_at)"
sleep 3 # long enough for the core to hydrate its queue and read the real Connectivity state
posts="$(logcat_app | grep -c "POST" || true)"
echo "POST requests sent while the Connectivity adapter says offline: $posts  (the core holds the queue instead of retrying)"
[ "$posts" = "0" ] || failed=1
tap remote-refresh
wait_for 20 "Refresh fails with the Http adapter's network error" refresh_failed || failed=1
echo "remote-error:   $(ui text remote-error || true)"
screenshot remote-restarted-offline
show_log "dropped|->"

step "4. airplane mode off: the Connectivity adapter reports online and the core replays the queue"
run adb_ shell cmd connectivity airplane-mode disable
wait_for 60 "the item is on the server and in the list (4 rows)" item_replayed || failed=1
screenshot remote-replayed
show_log "POST|GET" 8
if grep -q "POST /lists/inbox/todos -> 201 (Idempotency-Key $key_before)" <<<"$(logcat_app)"; then
  echo "the server received the replayed request with the original Idempotency-Key"
else
  echo "REPLAY NOT SEEN BY THE SERVER"; failed=1
fi

# ---- 4. process health ------------------------------------------------------------------------------------------------
step "process and log health"
adb_ shell pidof "$APP" >/dev/null && echo "alive" || { echo "NOT RUNNING"; failed=1; }
crashes="$(adb_ logcat -d 2>/dev/null | grep -c -E "FATAL EXCEPTION|Fatal signal|SIGSEGV|SIGABRT|ANR in dev.undra|JNI DETECTED" || true)"
echo "crash markers: $crashes"
[ "$crashes" = "0" ] || failed=1
step "files the app keeps (run-as)"
adb_ shell run-as "$APP" ls -l "$KV_DIR" 2>/dev/null || true

echo
if [ "$failed" = 0 ]; then echo "SMOKE PASSED"; else echo "SMOKE FAILED"; exit 1; fi
