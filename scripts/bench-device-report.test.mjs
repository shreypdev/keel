// Tests of scripts/bench-device-report.mjs: `node --test scripts/bench-device-report.test.mjs`.
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  BEGIN,
  END,
  KINDS,
  NOT_A_DEVICE,
  REQUIRED_OPS,
  RESULTS_DIR,
  RESULTS_MD,
  assemble,
  finalize,
  fmtNs,
  latestPerTarget,
  loadResults,
  readTargets,
  renderBlock,
  replaceBlock,
  resultsFromAttachments,
  resultsFromInstrumentation,
  resultsFromLog,
  validateRaw,
  validateResult,
} from "./bench-device-report.mjs";

const ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..");
const targets = readTargets();

const summary = (p50) => ({ n: 10, min: p50 * 0.9, p50, p90: p50 * 1.2, p99: p50 * 1.5, max: p50 * 2, mean: p50 * 1.05 });
const op = (id, p50, extra = {}) => ({ id, mode: "each", batch: 1, samples: 100, min: p50 * 0.9, p50, p90: p50 * 1.2, p99: p50 * 1.5, max: p50 * 2, mean: p50 * 1.05, note: `${id} note`, ...extra });

/** A runner's raw result, as a platform runner would write it. */
function rawFixture(platform = "ios", scale = 1) {
  return {
    schema: "undra-device-bench-raw/1",
    platform,
    runtime: "inproc",
    device: { model: "TestPhone1,1", os: "TestOS 1", arch: "arm64", cores: 6 },
    timer: { kind: "test clock", resolution_ns: 41, overhead_ns: 10 },
    config: {},
    ops: [op("sync_call", 50 * scale, { mode: "batched", batch: 1000 }), op("record_1kb", 2000 * scale), op("keyed_insert_10k", 15000 * scale), op("changeset_100", 90000 * scale)],
    cold: { snapshot_bytes: 100_000, launches: [], reload_in_process: { load_ns: summary(1e6), restore_ns: summary(5e4) } },
    drain: {
      rows: 10_000,
      updates_per_frame: 1667,
      frames: 240,
      warmup_frames: 60,
      producer: "a test producer",
      merged: { frame_ns: summary(5e5 * scale), drain_ns: summary(5e5 * scale), change_sets_per_frame_p50: 1667, entries_per_frame_p50: 1667, applied_per_frame_p50: 1, drains_per_frame_p50: 1 },
      unmerged_estimate: { per_entry_ns: summary(1000), frame_ns: 1_667_000, frame_ns_mean: 1_750_000, method: "a test method" },
      ratio_unmerged_over_merged: 3.3,
      ratio_unmerged_over_merged_mean: 3.5,
      note: "a drain note",
    },
    notes: [],
  };
}

const meta = (kind, target = "ios-test", extra = {}) => ({
  date: "2026-10-01",
  finishedAt: "2026-10-01T10:00:00.000Z",
  target,
  kind,
  label: `The ${kind} label`,
  commit: "abc1234",
  dirty: false,
  command: "scripts/bench-device.sh --device ios",
  buildType: "release",
  core: "release (LTO fat)",
  app: "Release",
  host: kind === "device" ? null : { cpu: "Test CPU", os: "TestOS", load_before: 1.5, load_after: 2.5 },
  notes: [],
  ...extra,
});

const cold = (load, restore) => ({ schema: "undra-device-bench-cold/1", load_ns: load, restore_ns: restore, snapshot_bytes: 100_000 });

test("fmtNs picks the unit and drops trailing zeros", () => {
  assert.equal(fmtNs(60), "60 ns");
  assert.equal(fmtNs(436), "436 ns");
  assert.equal(fmtNs(3000), "3 µs");
  assert.equal(fmtNs(1234), "1.23 µs");
  assert.equal(fmtNs(12_345), "12.3 µs");
  assert.equal(fmtNs(100_000), "100 µs");
  assert.equal(fmtNs(3_000_000), "3 ms");
  assert.equal(fmtNs(2_940_000), "2.94 ms");
  assert.equal(fmtNs(Number.NaN), "n/a");
});

test("a runner's result must carry every operation, finite numbers and a drain", () => {
  assert.deepEqual(validateRaw(rawFixture()), []);
  const missing = rawFixture();
  missing.ops = missing.ops.filter((o) => o.id !== "record_1kb");
  assert.match(validateRaw(missing).join("\n"), /op record_1kb is missing/);
  const nan = rawFixture();
  nan.ops[0].p50 = null;
  assert.match(validateRaw(nan).join("\n"), /sync_call: p50 is not a finite number/);
  const inverted = rawFixture();
  inverted.ops[1].p99 = 1;
  assert.match(validateRaw(inverted).join("\n"), /p99 1 is below p50/);
  const noDrain = rawFixture();
  delete noDrain.drain;
  assert.match(validateRaw(noDrain).join("\n"), /drain experiment is missing/);
  const wrongSchema = rawFixture();
  wrongSchema.schema = "something/2";
  assert.match(validateRaw(wrongSchema).join("\n"), /schema is/);
});

test("finalize labels everything that is not a device with the not-a-device claim, and refuses what is not valid", () => {
  for (const kind of ["simulator", "emulator", "browser"]) {
    const result = finalize(rawFixture(), meta(kind));
    assert.equal(result.claim, NOT_A_DEVICE);
    assert.deepEqual(validateResult(result), []);
  }
  assert.equal(finalize(rawFixture(), meta("device")).claim, "Measured on a device.");
  assert.throws(() => finalize(rawFixture(), meta("laptop")), /--kind must be one of/);
  const broken = rawFixture();
  broken.ops = [];
  assert.throws(() => finalize(broken, meta("simulator")), /is missing/);
  // A file whose kind was edited to say device but whose claim still says it is not, or the reverse, is not valid.
  const edited = finalize(rawFixture(), meta("simulator"));
  edited.claim = "Measured on a device.";
  assert.match(validateResult(edited).join("\n"), /not-a-device claim/);
  assert.deepEqual(KINDS, ["device", "simulator", "emulator", "browser"]);
});

test("assemble puts the cold launches into the full result", () => {
  const raw = assemble([rawFixture(), cold(3e6, 6e4), cold(2.8e6, 5e4)]);
  assert.deepEqual(raw.cold.launches, [
    { load_ns: 3e6, restore_ns: 6e4 },
    { load_ns: 2.8e6, restore_ns: 5e4 },
  ]);
  assert.throws(() => assemble([cold(1, 1)]), /exactly one full result/);
  assert.throws(() => assemble([rawFixture(), rawFixture()]), /exactly one full result/);
  assert.throws(() => assemble([rawFixture(), { ...cold(1, 1), snapshot_bytes: 5 }]), /did not restore the snapshot/);
  assert.throws(() => assemble([rawFixture(), cold(1, 1), { ...cold(1, 1), snapshot_bytes: 7 }]), /different sizes/);
});

test("results are read from an xcodebuild log, an instrumentation run and exported attachments", () => {
  const full = rawFixture();
  const log = ["Test Case started", `UNDRA_BENCH_RESULT ${JSON.stringify(full)}`, "noise", `2026 UNDRA_BENCH_RESULT ${JSON.stringify(cold(1, 2))}`].join("\n");
  assert.deepEqual(resultsFromLog(log).map((r) => r.schema), ["undra-device-bench-raw/1", "undra-device-bench-cold/1"]);
  assert.deepEqual(resultsFromLog("nothing here"), []);

  const instrumentation = ["INSTRUMENTATION_STATUS: class=x", `INSTRUMENTATION_STATUS: undra_bench_json=${JSON.stringify(full)}`, "INSTRUMENTATION_STATUS_CODE: 0"].join("\n");
  assert.equal(resultsFromInstrumentation(instrumentation).length, 1);

  const dir = mkdtempSync(join(tmpdir(), "bench-att-"));
  writeFileSync(join(dir, "a.txt"), JSON.stringify(cold(1, 2)));
  writeFileSync(join(dir, "b.txt"), JSON.stringify(full));
  writeFileSync(
    join(dir, "manifest.json"),
    JSON.stringify([
      {
        attachments: [
          { exportedFileName: "a.txt", suggestedHumanReadableName: "bench-cold-1_0_UUID.json" },
          { exportedFileName: "b.txt", suggestedHumanReadableName: "bench-full-0_0_UUID.json" },
          { exportedFileName: "b.txt", suggestedHumanReadableName: "screenshot.png" },
        ],
      },
    ]),
  );
  assert.deepEqual(resultsFromAttachments(dir).map((r) => r.schema), ["undra-device-bench-raw/1", "undra-device-bench-cold/1"]);
});

const file = (name, result) => ({ file: name, result });

test("a simulator's rows are never given a verdict; a device's are", () => {
  const sim = file("sim.json", finalize(assemble([rawFixture("ios"), cold(2e6, 5e4)]), meta("simulator", "ios-sim")));
  const block = renderBlock([sim], targets);
  assert.ok(block.startsWith(BEGIN) && block.endsWith(END));
  assert.match(block, /A simulator, not a device/);
  assert.doesNotMatch(block, /\| within/);
  assert.doesNotMatch(block, /\| over,/);
  assert.match(block, /none \(not a device\)/);
  // The blueprint targets sit beside the numbers.
  assert.match(block, /≤ 60 ns/);
  assert.match(block, /≤ 3 ms/);
  assert.match(block, /Pending hardware/);

  const device = file("dev.json", finalize(assemble([rawFixture("ios", 2), cold(2e6, 5e4)]), meta("device", "ios-dev")));
  const deviceBlock = renderBlock([device], targets);
  assert.match(deviceBlock, /A device\./);
  // 50 ns x 2 = 100 ns against 60 ns: over; record 4 µs against 3 µs: over; the cold start 2.05 ms against 3 ms: within.
  assert.match(deviceBlock, /Handle method call, primitive args and return \| 100 ns .*\| over, 1\.7x \|/);
  assert.match(deviceBlock, /Core cold start with 100 KB snapshot restore \| 2\.05 ms .*\| within \|/);
  assert.doesNotMatch(deviceBlock, /iOS \| iPhone with an A15-class chip \|/, "an iOS device row exists, so iOS is no longer pending");
  assert.match(deviceBlock, /Android \| mid-range Android phone, 2022/);
});

test("a browser has targets beside its numbers and no verdict, and the in-thread row carries the web target", () => {
  const raw = rawFixture("web");
  raw.ops.push(op("sync_call_runtime", 3500, { mode: "batched", batch: 5000 }));
  raw.cold.launches = [{ load_ns: 4.4e6 }];
  const block = renderBlock([file("web.json", finalize(raw, meta("browser", "web-chromium")))], targets);
  assert.match(block, /Handle method call, primitive args and return \|.*\| n\/a \|/, "the asynchronous generated call has no target");
  assert.match(block, /Handle method call, in-thread.*\| ≤ 80 ns \(in-thread[^|]*\| none \(no reference machine\) \|/);
  assert.match(block, /load only: no restore/);
  assert.doesNotMatch(block, /\| within/);
});

test("the newest run of each target is shown, and two runs of a day are compared", () => {
  const a = file("2026-10-01-ios-sim.json", finalize(assemble([rawFixture("ios", 1), cold(2e6, 5e4)]), meta("simulator", "ios-sim", { finishedAt: "2026-10-01T10:00:00Z" })));
  const b = file("2026-10-01-ios-sim-run2.json", finalize(assemble([rawFixture("ios", 1.1), cold(2e6, 5e4)]), meta("simulator", "ios-sim", { finishedAt: "2026-10-01T11:00:00Z" })));
  const older = file("2026-09-30-ios-sim.json", finalize(assemble([rawFixture("ios", 9), cold(2e6, 5e4)]), meta("simulator", "ios-sim", { date: "2026-09-30" })));
  const groups = latestPerTarget([older, b, a]);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].map((f) => f.file), ["2026-10-01-ios-sim.json", "2026-10-01-ios-sim-run2.json"]);
  const block = renderBlock([older, b, a], targets);
  assert.match(block, /Reproducibility/);
  assert.match(block, /2 runs/);
  assert.match(block, /1\.10x/);
  assert.doesNotMatch(block, /2026-09-30-ios-sim\.json/);
});

test("replaceBlock swaps what is between the markers and nothing else", () => {
  const md = `before\n${BEGIN}\nold\n${END}\nafter\n`;
  assert.equal(replaceBlock(md, `${BEGIN}\nnew\n${END}`), `before\n${BEGIN}\nnew\n${END}\nafter\n`);
  assert.throws(() => replaceBlock("no markers", "x"), /no <!--/);
});

test("no files: the block says how to make one", () => {
  assert.match(renderBlock([], targets), /No device-bench result files yet/);
});

// ---- the repository -----------------------------------------------------------------------------------

test("every committed result file is valid", () => {
  for (const { file: name, result } of loadResults()) {
    assert.deepEqual(validateResult(result), [], name);
    assert.ok(name.startsWith(result.date), `${name} starts with its date`);
  }
});

test("bench/RESULTS.md's device block is what the committed result files say", () => {
  const md = readFileSync(RESULTS_MD, "utf8");
  assert.equal(replaceBlock(md, renderBlock(loadResults(RESULTS_DIR), targets)), md, "run `node scripts/bench-device-report.mjs render`");
});

test("the blueprint targets are the ones in docs/blueprint.html section 14", () => {
  const html = readFileSync(join(ROOT, "docs", "blueprint.html"), "utf8");
  const cell = (ns) => (ns >= 1e6 ? `${ns / 1e6} ms` : ns >= 1e3 ? `${ns / 1e3} µs` : `${ns} ns`);
  for (const [id, row] of Object.entries(targets.rows)) {
    for (const platform of ["ios", "android", "web"]) {
      const text = cell(row[`${platform}_ns`]);
      assert.ok(html.includes(`≤ ${text}`), `${id}: the blueprint has no "≤ ${text}" (${platform})`);
    }
  }
});

test("all three runners measure the same operations, and the same drain", () => {
  const sources = {
    ios: readFileSync(join(ROOT, "examples/playground/ios/PlaygroundApp/Bench/BenchRunner.swift"), "utf8"),
    android: readFileSync(join(ROOT, "examples/playground/android/app/src/main/kotlin/dev/undra/playground/bench/BenchRunner.kt"), "utf8"),
    web: readFileSync(join(ROOT, "examples/playground/web/src/bench/runner.ts"), "utf8") + readFileSync(join(ROOT, "examples/playground/web/src/bench/ops.ts"), "utf8"),
  };
  for (const [platform, source] of Object.entries(sources)) {
    for (const id of REQUIRED_OPS) assert.ok(source.includes(`"${id}"`), `${platform} runner has no "${id}"`);
    assert.ok(source.includes("1667") || source.includes("1_667"), `${platform} runner does not drain 1,667 patches a frame`);
    assert.ok(source.includes("ADR-031") || platform !== "web", `${platform} runner names its drain`);
  }
  // The result files all name the drain the same way.
  const files = readdirSync(RESULTS_DIR).filter((f) => f.endsWith(".json"));
  for (const f of files) assert.equal(JSON.parse(readFileSync(join(RESULTS_DIR, f), "utf8")).drain.updates_per_frame, 1667, f);
});
