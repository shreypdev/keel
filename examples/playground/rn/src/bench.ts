import {
  CallTarget,
  ChangeOp,
  encodeCall,
  UndraWriter,
  codecs,
  decodeValue,
  type DrainStats,
  type UndraCore,
} from '@undra/runtime';
import { Bench, BigList, ItemCodec, Stress, UndraIds } from '@playground/core';
import { nativeCounters, type Log } from './undra';

/** One measurement. */
export interface BenchRow {
  readonly name: string;
  readonly value: number;
  readonly unit: string;
  readonly note: string;
}

const now = (): number => performance.now();
const sleep = (ms: number): Promise<void> => new Promise(resolve => setTimeout(resolve, ms));
/** Lets the JS thread render and run its timers between measurements. */
const breathe = (): Promise<void> => sleep(30);

function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? (sorted[mid] as number) : ((sorted[mid - 1] as number) + (sorted[mid] as number)) / 2;
}

function percentile(values: number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))] as number;
}

/** The time of `run` per iteration, median of `rounds` rounds of `n` iterations, in ns. */
function perCallNs(n: number, rounds: number, run: () => void): number {
  const samples: number[] = [];
  for (let round = 0; round < rounds; round++) {
    const started = now();
    for (let i = 0; i < n; i++) {
      run();
    }
    samples.push(((now() - started) * 1e6) / n);
  }
  return median(samples);
}

async function perAwaitUs(n: number, rounds: number, run: () => Promise<unknown>): Promise<number> {
  const samples: number[] = [];
  for (let round = 0; round < rounds; round++) {
    const started = now();
    for (let i = 0; i < n; i++) {
      await run();
    }
    samples.push(((now() - started) * 1e3) / n);
    await breathe();
  }
  return median(samples);
}

/** A `ChangeSet` with one keyed patch of one `Update` (docs/SPEC.md 3.5, 3.8) on signal 0 of `handle`. */
function updatePatch(txn: bigint, handle: bigint, index: number, id: number, label: string, version: number): Uint8Array {
  const patch = new UndraWriter(64);
  patch.writeU32(1); // one op
  patch.writeU8(2); // Update
  patch.writeU32(index);
  ItemCodec.encode(patch, { id, label, version });
  const value = patch.finish().slice();
  const w = new UndraWriter(value.length + 40);
  w.writeU64(txn);
  w.writeU32(1);
  w.writeU64(handle);
  w.writeU32(0); // `items`
  w.writeU8(ChangeOp.KeyedPatch);
  w.writeU32(value.length);
  w.writeRaw(value);
  return w.finish().slice();
}

/**
 * The measurements of ADR-038's consequences, on the device the app runs on: the sync call round
 * trip through JSI, a 1 KB record round trip, and 1,667 keyed patches per frame on a 10,000-row list
 * (100,000 a second at 60 Hz), plus the frame cadence the mirror drains at. Each is logged as
 * `UNDRA-RN BENCH <name> <value> <unit>`.
 */
export async function runBench(core: UndraCore, log: Log): Promise<BenchRow[]> {
  const rows: BenchRow[] = [];
  const record = (name: string, value: number, unit: string, note: string): void => {
    rows.push({ name, value, unit, note });
    log(`UNDRA-RN BENCH ${name} ${value.toFixed(unit === 'ns' ? 0 : 2)} ${unit} (${note})`);
  };

  // 1. The sync call round trip: Bench.bench_add(u32, u32) through `callSync` (one JSI host
  //    function, the core on this thread, the reply as an ArrayBuffer over the core's buffer).
  const bench = await Bench.create(core);
  const target = { target: CallTarget.ObjectMethod, handle: bench.handle } as const;
  const addArgs = new UndraWriter(8);
  addArgs.writeU32(1);
  addArgs.writeU32(2);
  const addPayload = addArgs.finish().slice();
  for (let i = 0; i < 2000; i++) {
    core.callSync(target, UndraIds.Objects.Bench.benchAdd, addPayload);
  }
  const check = decodeValue(codecs.u32, core.callSync(target, UndraIds.Objects.Bench.benchAdd, addPayload));
  if (check !== 3) {
    throw new Error(`bench_add answered ${check}`);
  }
  // Where the time of a sync call goes: a JSI host function that does nothing (the floor), the
  // transport alone (a prebuilt payload: JSI, the core, the reply ArrayBuffer), and UndraCore.
  const native = (globalThis as { __undraNative?: { abiVersion(): number; callSync(b: ArrayBuffer, o: number, l: number): ArrayBuffer } })
    .__undraNative;
  if (native !== undefined) {
    record('jsi_host_function_floor', perCallNs(20_000, 5, () => native.abiVersion()), 'ns', 'a JSI host function that returns a number');
    const call = new UndraWriter(32);
    call.writeU8(1); // target: object method
    call.writeU64(bench.handle);
    call.writeU32(UndraIds.Objects.Bench.benchAdd);
    call.writeU32(0x7fff_0001); // a call id UndraCore never uses
    call.writeRaw(addPayload);
    const prebuilt = call.finish().slice();
    record(
      'sync_call_native_only',
      perCallNs(10_000, 5, () => native.callSync(prebuilt.buffer, 0, prebuilt.byteLength)),
      'ns',
      'the same call with a prebuilt payload straight to __undraNative.callSync: JSI, the core, the reply ArrayBuffer',
    );
  }
  record(
    'js_encode_call',
    perCallNs(10_000, 5, () =>
      encodeCall({ target: CallTarget.ObjectMethod, handle: bench.handle, methodId: UndraIds.Objects.Bench.benchAdd, callId: 7, args: addPayload }),
    ),
    'ns',
    'the JavaScript half: encodeCall of that payload (a new UndraWriter each call), as UndraCore.callSync does',
  );
  record('js_mirror_flush_empty', perCallNs(10_000, 5, () => core.mirror.flush()), 'ns', 'mirror.flush() with nothing queued, as UndraCore.callSync does after every call');
  record(
    'sync_call_roundtrip_callSync',
    perCallNs(10_000, 5, () => core.callSync(target, UndraIds.Objects.Bench.benchAdd, addPayload)),
    'ns',
    'Bench.bench_add via UndraCore.callSync, median of 5 x 10,000',
  );
  await breathe();
  record(
    'sync_method_await_generated',
    await perAwaitUs(2000, 3, () => bench.benchAdd(1, 2)),
    'us',
    'await bench.benchAdd(1, 2): the generated method (call, inbox reply, promise), median of 3 x 2,000',
  );

  // 2. A 1 KB record round trip: Bench.bench_echo_bytes with 1,024 bytes, both ways.
  const kilobyte = new Uint8Array(1024);
  for (let i = 0; i < kilobyte.length; i++) {
    kilobyte[i] = i % 251;
  }
  const echoArgs = new UndraWriter(1032);
  echoArgs.writeBytes(kilobyte);
  const echoPayload = echoArgs.finish().slice();
  const echoed = decodeValue(codecs.bytes, core.callSync(target, UndraIds.Objects.Bench.benchEchoBytes, echoPayload));
  if (echoed.length !== 1024 || echoed[1000] !== 1000 % 251) {
    throw new Error('bench_echo_bytes did not echo');
  }
  record(
    'record_1kb_roundtrip_callSync',
    perCallNs(5000, 5, () => decodeValue(codecs.bytes, core.callSync(target, UndraIds.Objects.Bench.benchEchoBytes, echoPayload))),
    'ns',
    '1,024 bytes in and out of Bench.bench_echo_bytes via callSync, decoded, median of 5 x 5,000',
  );
  await breathe();
  record(
    'record_1kb_await_generated',
    await perAwaitUs(2000, 3, () => bench.benchEchoBytes(kilobyte)),
    'us',
    'await bench.benchEchoBytes(1 KB), median of 3 x 2,000',
  );
  bench.close();
  await breathe();

  // 3a. 1,667 keyed patches per frame on a 10,000-row list, the mirror alone (the web number of
  //     ADR-031, 0.33-0.45 ms per frame on V8): 1,667 one-op Update change-sets enqueued, one drain.
  const probe = await BigList.create(core);
  const drains: DrainStats[] = [];
  const stopListening = core.mirror.addDrainListener(stats => drains.push(stats));
  const frames = 30;
  const perFrame = 1667;
  const enqueueMs: number[] = [];
  const drainMs: number[] = [];
  let txn = 1n << 40n;
  for (let frame = 0; frame < frames; frame++) {
    const payloads: Uint8Array[] = [];
    for (let k = 0; k < perFrame; k++) {
      const index = (k * 6 + frame) % 10_000;
      payloads.push(updatePatch(txn++, probe.handle, index, index + 1, `Patched ${frame}.${k}`, frame + 1));
    }
    const t0 = now();
    for (const payload of payloads) {
      core.mirror.enqueue(payload);
    }
    const t1 = now();
    core.mirror.flush();
    const t2 = now();
    enqueueMs.push(t1 - t0);
    drainMs.push(t2 - t1);
    await breathe();
  }
  const rowsAfter = probe.items.get();
  if (rowsAfter.length !== 10_000 || rowsAfter[(1666 * 6 + frames - 1) % 10_000]?.version !== frames) {
    throw new Error('the probe patches were not applied');
  }
  record(
    'patches_1667_per_frame_mirror_drain',
    median(drainMs),
    'ms',
    `one drain of 1,667 one-op keyed patches on 10,000 rows, median of ${frames} frames (p90 ${percentile(drainMs, 90).toFixed(2)} ms)`,
  );
  record(
    'patches_1667_per_frame_mirror_enqueue',
    median(enqueueMs),
    'ms',
    'parsing those 1,667 change-sets on arrival (mirror.enqueue), median',
  );
  probe.close();
  await breathe();

  // 3b. The same through the native core end to end: 1,667 `updateAt` calls in one JS turn (each a
  //     JSI call that runs the core and drains its inbox), then the one drain that applies them all.
  const list = await BigList.create(core);
  const rounds: Array<{ send: number; settle: number; drain: number; applied: number }> = [];
  for (let round = 0; round < 5; round++) {
    drains.length = 0;
    const t0 = now();
    const pending: Array<Promise<void>> = [];
    for (let k = 0; k < perFrame; k++) {
      pending.push(list.updateAt((k * 6 + round) % 10_000, `Update ${round}.${k}`));
    }
    const t1 = now();
    await Promise.all(pending);
    const t2 = now();
    const drain = drains.reduce((sum, d) => sum + d.durationMs, 0);
    const applied = drains.reduce((sum, d) => sum + d.appliedEntries, 0);
    rounds.push({ send: t1 - t0, settle: t2 - t0, drain, applied });
    await breathe();
  }
  stopListening();
  record(
    'patches_1667_end_to_end_send',
    median(rounds.map(r => r.send)),
    'ms',
    `1,667 BigList.updateAt calls through JSI in one turn (${((median(rounds.map(r => r.send)) * 1000) / perFrame).toFixed(2)} us each), median of 5`,
  );
  record(
    'patches_1667_end_to_end_total',
    median(rounds.map(r => r.settle)),
    'ms',
    `until every call resolved, drains included (drain ${median(rounds.map(r => r.drain)).toFixed(2)} ms, ${median(rounds.map(r => r.applied))} entries applied after merging)`,
  );
  list.close();
  await breathe();

  // 4. Frames: requestAnimationFrame's cadence in this React Native, and the mirror's drains under a
  //    firehose the core generates on its own (Stress, 10,000 updates a second for 2 s).
  const rafIntervals: number[] = [];
  await new Promise<void>(resolve => {
    let last = now();
    let count = 0;
    const step = (): void => {
      const t = now();
      rafIntervals.push(t - last);
      last = t;
      if (++count < 60) {
        requestAnimationFrame(step);
      } else {
        resolve();
      }
    };
    requestAnimationFrame(step);
  });
  record('raf_interval', median(rafIntervals), 'ms', 'median interval of 60 chained requestAnimationFrame callbacks');
  const vsync = (globalThis as { __undraNative?: { requestFrame(): boolean } }).__undraNative?.requestFrame() === true;
  log(`UNDRA-RN frame source: ${vsync ? 'native vsync (CADisplayLink / AChoreographer)' : 'timer fallback'}`);

  const stress = await Stress.create(core);
  const drainTimes: number[] = [];
  const stressDrains: DrainStats[] = [];
  const stopStress = core.mirror.addDrainListener(stats => {
    drainTimes.push(now());
    stressDrains.push(stats);
  });
  const before = core.mirror.stats();
  const nativeBefore = nativeCounters(core);
  await stress.start('firehose', 10_000);
  await sleep(2000);
  await stress.stop();
  await sleep(100);
  stopStress();
  const after = core.mirror.stats();
  const nativeAfter = nativeCounters(core);
  const generated = Number(stress.generated.get());
  stress.close();
  const intervals = drainTimes.slice(1).map((t, i) => t - (drainTimes[i] as number));
  const received = after.entriesReceived - before.entriesReceived;
  const applied = after.entriesApplied - before.entriesApplied;
  record(
    'firehose_drain_interval',
    median(intervals),
    'ms',
    `${stressDrains.length} drains in ~2.1 s for ${generated} core-generated updates: ${received} entries received, ${applied} applied (${(received / Math.max(1, applied)).toFixed(0)}x merged); wakes ${
      nativeAfter !== null && nativeBefore !== null ? nativeAfter.wakes - nativeBefore.wakes : '?'
    }`,
  );
  return rows;
}
