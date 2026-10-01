#!/usr/bin/env bash
# The web size gate (ADR-052, constitution R9: budgets are tests).
#
#   scripts/wasm-size.sh             measure and gate; the JSON goes to $UNDRA_BENCH_RESULTS_DIR
#                                    (default target/wasm-size/), the record is not touched
#   scripts/wasm-size.sh --record    measure, gate against the budget only, and write the record:
#                                    bench/results/web-size.jsonl and `measured_gzip_bytes` in
#                                    bench/budgets.toml (commit both; the budgets test checks they agree)
#
# What it measures is what the README and the site publish as the web core: the `undra init`
# template (web only) built by `undra build --platform web`, which is the release-wasm profile of
# the generated shim followed by `wasm-opt -Oz --strip-debug --strip-producers`. The wasm module
# alone. It refuses to measure without wasm-opt (`brew install binaryen`, or binaryen's release
# tarball; CI pins version_133): an unoptimised module is not what ships. Compression is zlib's
# deflate at level 9 through Python, the same bytes on every machine (GNU gzip and Node's zlib
# differ by up to 1%; Apple's `gzip -9 -n` agrees with zlib).
#
# The gate is `[size."web/hello-wasm"]` of bench/budgets.toml: at most `budget_gzip_bytes`, and at
# most `tolerance` over `measured_gzip_bytes` (the record), whichever is lower.
#
# It also records, ungated, what a hello-world app ships of the JavaScript runtime (`@undra/runtime`
# tree-shaken and minified by the Vite of its own lockfile, scripts/web-size-runtime.mjs), when the
# TypeScript runtime's node_modules are installed; otherwise that line says why it is missing.
#
# Output: one JSON line per artefact on stdout and in <dir>/web-size.jsonl, the comparison on stderr.
# Exit status: 0 within the gate, 1 over it, 2 when it could not measure.
#
# Environment: UNDRA_SIZE_TARGET_DIR (cargo's target directory for the template; default
# target/wasm-size/target, kept between runs so dependencies build once).
set -euo pipefail

die() { echo "wasm-size.sh: $*" >&2; exit 2; }

RECORD=0
case "${1:-}" in
  --record) RECORD=1 ;;
  "") ;;
  *) die "usage: scripts/wasm-size.sh [--record]" ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/target/wasm-size"
PROJECT="$WORK/hello"
export CARGO_TARGET_DIR="${UNDRA_SIZE_TARGET_DIR:-$WORK/target}"
if [ "$RECORD" = 1 ]; then
  OUT_DIR="$ROOT/bench/results"
else
  OUT_DIR="${UNDRA_BENCH_RESULTS_DIR:-$WORK}"
fi
mkdir -p "$WORK" "$OUT_DIR"

command -v cargo >/dev/null || die "cargo is not on PATH (source scripts/env.sh)"
command -v python3 >/dev/null || die "python3 is needed to compress with zlib"
WASM_OPT="$(command -v wasm-opt || true)"
[ -n "$WASM_OPT" ] || die "wasm-opt is not installed: the gate measures the optimised module that ships. Install binaryen (brew install binaryen, or https://github.com/WebAssembly/binaryen/releases)"

# 1. The CLI of this checkout, and a fresh template project that takes Undra from it.
echo "==> building undra-cli" >&2
cargo build -q -p undra-cli --manifest-path "$ROOT/Cargo.toml" --target-dir "$ROOT/target"
UNDRA="$ROOT/target/debug/undra"
rm -rf "$PROJECT"
echo "==> undra init hello --platforms web" >&2
"$UNDRA" init hello --platforms web --undra-path "$ROOT" --dir "$WORK" >/dev/null

# 2. The web build, as an app developer runs it.
echo "==> undra build --platform web" >&2
"$UNDRA" build --platform web -C "$PROJECT" >"$WORK/build.log" 2>&1 \
  || { tail -40 "$WORK/build.log" >&2; die "undra build --platform web failed (log: $WORK/build.log)"; }
WASM="$PROJECT/build/web/undra_core.wasm"
[ -f "$WASM" ] || die "undra build did not write $WASM"
RAW="$(ls -t "$CARGO_TARGET_DIR"/wasm32-unknown-unknown/release-wasm/undra_core_*.wasm 2>/dev/null | head -1)"
[ -n "$RAW" ] || die "cannot find cargo's wasm output in $CARGO_TARGET_DIR"
grep -q "before wasm-opt" "$WORK/build.log" \
  || die "undra build did not run wasm-opt (see $WORK/build.log); the gate does not measure an unoptimised module"

# 3. The JavaScript runtime's share (recorded, not gated).
RUNTIME_DIR="$ROOT/runtimes/ts/@undra/runtime"
JS_JSON=""
JS_WHY=""
if ! command -v node >/dev/null; then
  JS_WHY="node is not installed"
elif JS_JSON="$(node "$ROOT/scripts/web-size-runtime.mjs" "$PROJECT" "$RUNTIME_DIR" "$WORK/js" 2>"$WORK/js.log")"; then
  :
else
  JS_JSON=""
  JS_WHY="$(tail -1 "$WORK/js.log" | cut -c1-200)"
fi

# 4. Sizes, the gate and the JSON lines.
# "-dirty" when what the module is built from differs from the commit (the record itself, the
# budgets file and this script do not count).
COMMIT="$(git -C "$ROOT" rev-parse --short HEAD)"
[ -z "$(git -C "$ROOT" status --porcelain --untracked-files=no -- crates runtimes/ts Cargo.toml Cargo.lock)" ] \
  || COMMIT="$COMMIT-dirty"
RUSTC="$(rustc --version | awk '{print $2}')"
WASM_OPT_VERSION="$("$WASM_OPT" --version | awk '{print $NF}' | tr -d '()')"

python3 - "$ROOT/bench/budgets.toml" "$WASM" "$RAW" "$WORK/js" "$JS_JSON" "$JS_WHY" "$OUT_DIR/web-size.jsonl" \
  "$RECORD" "$COMMIT" "$RUSTC" "$WASM_OPT_VERSION" <<'PY'
import datetime, gzip, json, math, re, sys

(budgets_path, wasm, raw, js_dir, js_json, js_why, out, record, commit, rustc, wasm_opt) = sys.argv[1:12]
record = record == "1"

def gz(path):
    data = open(path, "rb").read()
    return len(data), len(gzip.compress(data, 9, mtime=0))

def size_table(text, name):
    """The keys of `[size."name"]` (the budgets file's strict subset: `key = number` lines)."""
    m = re.search(r'^\[size\."' + re.escape(name) + r'"\]\s*$(.*?)(?=^\[|\Z)', text, re.M | re.S)
    if not m:
        sys.exit(f'wasm-size.sh: bench/budgets.toml has no [size."{name}"] table')
    table = {}
    for line in m.group(1).splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            key, value = (part.strip() for part in line.split("=", 1))
            table[key] = float(value.replace("_", ""))
    return table

text = open(budgets_path).read()
table = size_table(text, "web/hello-wasm")
budget = table["budget_gzip_bytes"]
recorded = table.get("measured_gzip_bytes")
tolerance = table.get("tolerance")

nbytes, gzipped = gz(wasm)
raw_bytes, raw_gzipped = gz(raw)
if record:
    # A new record: only the budget gates it, and the tolerance is measured from it.
    recorded = gzipped
ceiling = budget if recorded is None or tolerance is None else min(budget, math.floor(recorded * (1 + tolerance)))
ok = gzipped <= ceiling
today = datetime.date.today().isoformat()

lines = [{
    "artifact": "web/hello-wasm",
    "bytes": nbytes,
    "gzipped": gzipped,
    "budget": int(budget),
    "ceiling": int(ceiling),
    "tolerance": tolerance,
    "raw_bytes": raw_bytes,
    "raw_gzipped": raw_gzipped,
    "gzip": "zlib deflate level 9",
    "what": "undra init template (web), undra build --platform web: release-wasm + wasm-opt -Oz; the wasm module alone",
    "commit": commit,
    "date": today,
    "rustc": rustc,
    "wasm_opt": wasm_opt,
}]
js = {"artifact": "web/hello-runtime-js", "gated": False, "budget": 8000,
      "what": "@undra/runtime as the hello app's src/undra.ts imports it, Vite production build of the runtime's lockfile; worker script excluded",
      "gzip": "zlib deflate level 9", "commit": commit, "date": today}
if js_json:
    chunks = json.loads(js_json)
    js["bytes"], js["gzipped"] = gz(f"{js_dir}/{chunks['runtime']}")
    js["bindings_gzipped"] = gz(f"{js_dir}/{chunks['bindings']}")[1]
    js["app_gzipped"] = gz(f"{js_dir}/{chunks['app']}")[1]
else:
    js["bytes"] = js["gzipped"] = None
    js["error"] = js_why or "not measured"
lines.append(js)

with open(out, "w") as f:
    for line in lines:
        f.write(json.dumps(line, separators=(",", ":")) + "\n")
for line in lines:
    print(json.dumps(line, separators=(",", ":")))

if record:
    new = re.sub(r'(^\[size\."web/hello-wasm"\]\s*$.*?^measured_gzip_bytes\s*=\s*)\d[\d_]*',
                 lambda m: m.group(1) + str(gzipped), text, count=1, flags=re.M | re.S)
    if new == text and recorded != table.get("measured_gzip_bytes"):
        sys.exit('wasm-size.sh: could not write measured_gzip_bytes into [size."web/hello-wasm"]')
    open(budgets_path, "w").write(new)

kb = lambda n: f"{n / 1000:.1f} KB"
print(f"web/hello-wasm: {nbytes:,} bytes, {gzipped:,} gzipped ({kb(gzipped)}); raw {raw_bytes:,} / {raw_gzipped:,} before wasm-opt", file=sys.stderr)
print(f"  gate: <= {int(ceiling):,} (budget {int(budget):,}" + (f", record {int(recorded):,} + {tolerance:.0%}" if recorded and tolerance is not None else "") + f"): {'ok' if ok else 'OVER'}", file=sys.stderr)
if js_json:
    print(f"web/hello-runtime-js: {js['bytes']:,} bytes, {js['gzipped']:,} gzipped ({kb(js['gzipped'])}), not gated (blueprint 8 KB; ADR-052 open decision 1)", file=sys.stderr)
else:
    print(f"web/hello-runtime-js: not measured ({js['error']})", file=sys.stderr)
if record:
    print(f"recorded: {out} and measured_gzip_bytes = {gzipped} in bench/budgets.toml", file=sys.stderr)
if not ok:
    over = "the budget" if gzipped > budget else f"{tolerance:.0%} over the record"
    print(f"web/hello-wasm is {gzipped - int(ceiling):,} bytes over the gate ({over}). Find what grew (ADR-052 has the "
          "twiggy recipe); if the growth is intended, re-record with scripts/wasm-size.sh --record in the same commit.",
          file=sys.stderr)
    sys.exit(1)
PY
