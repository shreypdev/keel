#!/usr/bin/env bash
# The web size gate (ADR-052, constitution R9: budgets are tests).
#
#   scripts/wasm-size.sh             measure and gate; the JSON goes to $UNDRA_BENCH_RESULTS_DIR
#                                    (default target/wasm-size/), the record is not touched
#   scripts/wasm-size.sh --record    measure, gate against the budgets only, and write the record:
#                                    bench/results/web-size.jsonl and `measured_gzip_bytes` of each
#                                    [size."..."] table in bench/budgets.toml (commit both; the
#                                    budgets test checks they agree). CI never records.
#
# Two artefacts, each gated by its `[size."<artifact>"]` table of bench/budgets.toml: at most
# `budget_gzip_bytes`, and at most `tolerance` over `measured_gzip_bytes` (the record), whichever is
# lower. The ceiling comes from the committed record, never from the build being measured.
#
# * web/hello-wasm: what the README and the site publish as the web core: the `undra init` template
#   (web only) built by `undra build --platform web`, which is the release-wasm profile of the
#   generated shim followed by `wasm-opt -Oz --strip-debug --strip-producers`. The wasm module
#   alone. It refuses to measure without wasm-opt (`brew install binaryen`, or binaryen's release
#   tarball; CI pins version_133): an unoptimised module is not what ships.
# * web/hello-runtime-js: what a hello-world app ships of the JavaScript runtime (`@undra/runtime`
#   tree-shaken and minified by the Vite of its own lockfile, scripts/web-size-runtime.mjs). It
#   needs the TypeScript runtime's node_modules (`npm ci` in runtimes/ts/@undra/runtime) and
#   refuses to pass without them, like the wasm without wasm-opt.
#
# Compression is zlib's deflate at level 9 through Python, the same bytes on every machine (GNU gzip
# and Node's zlib differ by up to 1%; Apple's `gzip -9 -n` agrees with zlib).
#
# Output: one JSON line per artefact on stdout and in <dir>/web-size.jsonl, the comparison on stderr.
# Exit status: 0 within the gates, 1 over one, 2 when it could not measure.
#
# Environment: UNDRA_SIZE_TARGET_DIR (cargo's target directory for the template; default
# target/wasm-size/target, kept between runs so dependencies build once; the template itself is
# created afresh and rebuilt on every run, so a stale module cannot be measured).
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
# Named after the core's namespace (ADR-044): the template's core package `hello-core` gives `hello_core`.
WASM="$PROJECT/build/web/hello_core.wasm"
[ -f "$WASM" ] || die "undra build did not write $WASM"
RAW="$(ls -t "$CARGO_TARGET_DIR"/wasm32-unknown-unknown/release-wasm/undra_core_*.wasm 2>/dev/null | head -1)"
[ -n "$RAW" ] || die "cannot find cargo's wasm output in $CARGO_TARGET_DIR"
grep -q "before wasm-opt" "$WORK/build.log" \
  || die "undra build did not run wasm-opt (see $WORK/build.log); the gate does not measure an unoptimised module"
# What ships must not name the machine it was built on: `undra build` remaps the home directory
# out of release builds (ADR-052; the panic locations of the Undra and registry crates below it).
if [ -n "${HOME:-}" ] && [ "$HOME" != "/" ] && LC_ALL=C grep -q -a -F -- "$HOME" "$WASM"; then
  echo "wasm-size.sh: $WASM contains the builder's home directory ($HOME): the release build's --remap-path-prefix is missing" >&2
  exit 1
fi

# 3. The JavaScript runtime's share (gated like the wasm; a run that cannot measure it fails).
RUNTIME_DIR="$ROOT/runtimes/ts/@undra/runtime"
JS_JSON=""
JS_WHY=""
if ! command -v node >/dev/null; then
  JS_WHY="node is not installed"
elif JS_JSON="$(node "$ROOT/scripts/web-size-runtime.mjs" "$PROJECT" "$RUNTIME_DIR" "$WORK/js" 2>"$WORK/js.log")"; then
  :
else
  JS_JSON=""
  JS_WHY="$(tail -1 "$WORK/js.log" | cut -c1-300)"
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
today = datetime.date.today().isoformat()

def gate(name, gzipped):
    """The gate of `name` for a measured size: (ceiling, ok, table)."""
    table = size_table(text, name)
    budget = table["budget_gzip_bytes"]
    recorded = gzipped if record else table.get("measured_gzip_bytes")
    tolerance = table.get("tolerance")
    ceiling = budget if recorded is None or tolerance is None else min(budget, math.floor(recorded * (1 + tolerance)))
    return int(ceiling), gzipped <= ceiling, table

nbytes, gzipped = gz(wasm)
raw_bytes, raw_gzipped = gz(raw)
ceiling, ok, table = gate("web/hello-wasm", gzipped)
wasm_line = {
    "artifact": "web/hello-wasm",
    "bytes": nbytes,
    "gzipped": gzipped,
    "budget": int(table["budget_gzip_bytes"]),
    "ceiling": ceiling,
    "tolerance": table.get("tolerance"),
    "raw_bytes": raw_bytes,
    "raw_gzipped": raw_gzipped,
    "gzip": "zlib deflate level 9",
    "what": "undra init template (web), undra build --platform web: release-wasm + wasm-opt -Oz; the wasm module alone",
    "commit": commit,
    "date": today,
    "rustc": rustc,
    "wasm_opt": wasm_opt,
}
results = [(wasm_line, ok, table)]

js_table = size_table(text, "web/hello-runtime-js")
js_line = {"artifact": "web/hello-runtime-js", "budget": int(js_table["budget_gzip_bytes"]),
      "what": "@undra/runtime as the hello app's src/undra.ts imports it, Vite production build of the runtime's lockfile; worker script excluded",
      "gzip": "zlib deflate level 9", "commit": commit, "date": today}
if js_json:
    chunks = json.loads(js_json)
    js_line["bytes"], js_line["gzipped"] = gz(f"{js_dir}/{chunks['runtime']}")
    js_ceiling, js_ok, _ = gate("web/hello-runtime-js", js_line["gzipped"])
    js_line["ceiling"] = js_ceiling
    js_line["tolerance"] = js_table.get("tolerance")
    js_line["bindings_gzipped"] = gz(f"{js_dir}/{chunks['bindings']}")[1]
    js_line["app_gzipped"] = gz(f"{js_dir}/{chunks['app']}")[1]
    results.append((js_line, js_ok, js_table))
else:
    js_line["bytes"] = js_line["gzipped"] = None
    js_line["error"] = js_why or "not measured"

lines = [wasm_line, js_line]
for line in lines:
    print(json.dumps(line, separators=(",", ":")))

kb = lambda n: f"{n / 1000:.1f} KB"
print(f"web/hello-wasm: {nbytes:,} bytes, {gzipped:,} gzipped ({kb(gzipped)}); raw {raw_bytes:,} / {raw_gzipped:,} before wasm-opt", file=sys.stderr)
failed = []
for line, passed, tbl in results:
    name, size, ceil = line["artifact"], line["gzipped"], line["ceiling"]
    if name != "web/hello-wasm":
        print(f"{name}: {line['bytes']:,} bytes, {size:,} gzipped ({kb(size)})", file=sys.stderr)
    rec, tol = (size if record else tbl.get("measured_gzip_bytes")), tbl.get("tolerance")
    print(f"  gate: <= {ceil:,} (budget {line['budget']:,}" + (f", record {int(rec):,} + {tol:.0%}" if rec and tol is not None else "") + f"): {'ok' if passed else 'OVER'}", file=sys.stderr)
    if not passed:
        over = "the budget" if size > line["budget"] else f"{tol:.0%} over the record"
        failed.append(f"{name} is {size - ceil:,} bytes over its gate ({over})")

# The JSON behind the numbers: always for a gate run (CI uploads it, pass or fail); for --record
# only when everything was measured and within its budget, so a failed run never rewrites the record.
if not record or (js_json and not failed):
    with open(out, "w") as f:
        for line in lines:
            f.write(json.dumps(line, separators=(",", ":")) + "\n")

if not js_json:
    # Gated since ADR-052's decision 2: a run that cannot measure it must not pass (or record).
    sys.stderr.write(f"wasm-size.sh: web/hello-runtime-js could not be measured: {js_line['error']}\n"
                     "  install the TypeScript runtime's dependencies: (cd runtimes/ts/@undra/runtime && npm ci)\n")
    sys.exit(2)

if record and not failed:
    new = text
    for line, _, _ in results:
        block = re.compile(r'(^\[size\."' + re.escape(line["artifact"]) + r'"\]\s*$)(.*?)(?=^\[|\Z)', re.M | re.S)
        m = block.search(new)
        body, n = re.subn(r'^(measured_gzip_bytes\s*=\s*)\d[\d_]*', lambda mm: mm.group(1) + str(line["gzipped"]), m.group(2), count=1, flags=re.M)
        if n != 1:
            sys.exit(f'wasm-size.sh: [size."{line["artifact"]}"] has no measured_gzip_bytes line to write the record into')
        new = new[:m.start(2)] + body + new[m.end(2):]
    open(budgets_path, "w").write(new)
    print(f"recorded: {out} and measured_gzip_bytes of each [size] table in bench/budgets.toml", file=sys.stderr)

if failed:
    for message in failed:
        print(message, file=sys.stderr)
    print("Find what grew (ADR-052 has the twiggy recipe for the wasm); if the growth is intended, re-record with "
          "scripts/wasm-size.sh --record in the same commit, where review sees it.", file=sys.stderr)
    sys.exit(1)
PY
