import {
  CallTarget,
  UndraCallError,
  UndraSchemaMismatchError,
  UndraTransportError,
  UndraWriter,
  codecs,
  decodeValue,
  type UndraCore,
} from '@undra/runtime';
import { NativeTransport, installNative } from '@undra/react-native';
import {
  BigList,
  Counter,
  LabError,
  Probe,
  Stress,
  UndraIds,
  UndraPlaygroundCore,
  add,
  addLater,
  explode,
  greet,
  parseCount,
} from '@playground/core';
import { nativeCounters, type Log, type Playground } from './undra';

/** One self-check's outcome. */
export interface CheckResult {
  readonly id: string;
  readonly title: string;
  readonly pass: boolean;
  readonly detail: string;
}

type Check = (core: UndraCore, playground: Playground) => Promise<string>;

function expect(condition: boolean, what: string): void {
  if (!condition) {
    throw new Error(what);
  }
}

async function rejects(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error('expected a rejection');
}

const sleep = (ms: number): Promise<void> => new Promise(resolve => setTimeout(resolve, ms));

/**
 * The boundary behaviours a Node host cannot show for @undra/react-native, checked in the app on the
 * device against the real native core (docs/REACT_NATIVE.md, "What is tested where").
 */
const CHECKS: ReadonlyArray<readonly [string, string, Check]> = [
  [
    'RN01',
    'sync call through JSI (raw callSync and a generated method)',
    async core => {
      const w = new UndraWriter(8);
      w.writeI32(20);
      w.writeI32(22);
      const body = core.callSync(CallTarget.FreeFunction, UndraIds.Functions.add, w.finish());
      expect(decodeValue(codecs.i32, body) === 42, `callSync add = ${decodeValue(codecs.i32, body)}`);
      expect((await add(2, 3, core)) === 5, 'add(2, 3) === 5');
      // loadNative attached through the generated entry: its core is the bindings' default (ADR-044).
      expect(UndraPlaygroundCore.core === core, 'UndraPlaygroundCore.core is the native core');
      expect((await add(3, 4)) === 7, 'add(3, 4) on the default core === 7');
      expect((await greet('Hermes', core)).includes('Hermes'), 'greet');
      return 'add(20, 22) = 42 synchronously; add(2, 3) = 5; add(3, 4) = 7 on UndraPlaygroundCore.core';
    },
  ],
  [
    'RN02',
    'async call answered from the core thread',
    async core => {
      const before = nativeCounters(core)?.wakes ?? 0;
      const sum = await addLater(40, 2, 20, core);
      expect(sum === 42, `addLater = ${sum}`);
      const wakes = (nativeCounters(core)?.wakes ?? 0) - before;
      expect(wakes >= 1, 'the reply woke the JS thread through invokeAsync');
      return `addLater(40, 2, 20 ms) = 42; ${wakes} wake(s)`;
    },
  ],
  [
    'RN03',
    'typed error',
    async core => {
      const error = await rejects(() => parseCount('twelve', core));
      expect(error instanceof LabError, `LabError, got ${String(error)}`);
      return `parseCount("twelve") rejected with ${(error as LabError).kind}`;
    },
  ],
  [
    'RN04',
    'panic containment: status 2, the core keeps working',
    async core => {
      const error = await rejects(() => explode('on purpose', core));
      // The core answers status 2 on the wire; the generated binding maps it onto the closed set (ADR-032, amendment A).
      expect(error instanceof UndraCallError.Panicked, `UndraCallError.Panicked (status 2), got ${String(error)}`);
      expect((await add(1, 1, core)) === 2, 'the core still answers');
      return 'explode() -> UndraCallError.Panicked (status 2); add(1, 1) = 2 afterwards';
    },
  ],
  [
    'RN05',
    'cancellation of an in-flight call',
    async core => {
      const probe = await Probe.create(core);
      const abort = new AbortController();
      const pending = probe.hang(abort.signal);
      await sleep(20);
      abort.abort();
      const error = await rejects(() => pending);
      expect(error !== undefined, 'hang() rejected');
      const counters = await probe.counters();
      probe.close();
      return `hang() aborted; probe counters ${JSON.stringify(counters, (_k, v) => (typeof v === 'bigint' ? Number(v) : v))}`;
    },
  ],
  [
    'RN06',
    'stream with backpressure (credit 16, top-up)',
    async core => {
      const probe = await Probe.create(core);
      const seen: number[] = [];
      for await (const tick of probe.ticks(100)) {
        seen.push(tick);
      }
      probe.close();
      expect(seen.length === 100, `100 items, got ${seen.length}`);
      expect(seen.every((value, index) => index === 0 || value > (seen[index - 1] as number)), 'in order');
      return `100 items in order (${seen[0]}..${seen[99]})`;
    },
  ],
  [
    'RN07',
    'store observe, transaction, read-your-writes',
    async core => {
      const counter = await Counter.create(core);
      expect(counter.count.get() === 0, 'initial value applied by create()');
      await counter.increment();
      expect(counter.count.get() === 1, 'applied before the await resumed');
      await counter.add(4);
      const parity = counter.parity.get();
      counter.close();
      expect(parity === 'odd', `computed parity, got ${parity}`);
      return 'count 0 -> 1 -> 5, parity odd, each visible right after its await';
    },
  ],
  [
    'RN08',
    'keyed patch on a 10,000-row list',
    async core => {
      const list = await BigList.create(core);
      const before = core.mirror.stats().entriesApplied;
      const id = await list.insertAt(0, 'Inserted by RN08');
      const items = list.items.get();
      expect(items.length === 10001 && items[0]?.label === 'Inserted by RN08', 'row 0 is the new one');
      expect(items[0]?.id === id, 'with the id the call returned');
      expect(list.count.get() === 10001, 'computed count');
      const applied = core.mirror.stats().entriesApplied - before;
      list.close();
      return `insertAt(0) -> 10,001 rows, ${applied} entries applied (a one-op patch + the count)`;
    },
  ],
  [
    'RN09',
    'native Clock and Timer drive the core on its own',
    async core => {
      const stress = await Stress.create(core);
      await stress.start('firehose', 2000);
      await sleep(300);
      await stress.stop();
      const generated = Number(stress.generated.get());
      stress.close();
      expect(generated > 0, 'the generator ran');
      const native = nativeCounters(core);
      return `generated ${generated} updates in 300 ms (Timer + Clock answered natively: ${native?.nativePortCalls ?? '?'} calls)`;
    },
  ],
  [
    'RN10',
    'schema gate refuses another schema before undra_init',
    async core => {
      const native = installNative(UndraPlaygroundCore.namespace);
      expect(native.namespace === UndraPlaygroundCore.namespace, `the module is the core ${native.namespace}`);
      const transport = new NativeTransport({ namespace: native.namespace, native, expectedSchemaHash: 0x1234n });
      const error = await rejects(() =>
        transport.start({
          reply() {},
          changeSet() {},
          streamItem() {},
          portCall: () => ({ kind: 'unavailable' }),
          log() {},
          closed() {},
        }),
      );
      expect(error instanceof UndraSchemaMismatchError, `UndraSchemaMismatchError, got ${String(error)}`);
      expect((await add(3, 4, core)) === 7, 'the running core is untouched');
      // A namespace the app has no core of: the module's lookup (the class on iOS, lib<ns>.so on Android) says so.
      let unknown: unknown;
      try {
        installNative('no_such_core');
      } catch (failure) {
        unknown = failure;
      }
      expect(
        unknown instanceof UndraTransportError && unknown.reason === 'unsupported' && unknown.message.includes('no_such_core'),
        `an unknown core is refused, got ${String(unknown)}`,
      );
      return 'expected 0x1234, refused; the running core still answers; no_such_core refused by the module';
    },
  ],
];

/** Runs every check in order and logs `UNDRA-RN CHECK <id> PASS|FAIL <detail>`. */
export async function runChecks(playground: Playground, log: Log): Promise<CheckResult[]> {
  const results: CheckResult[] = [];
  for (const [id, title, check] of CHECKS) {
    let result: CheckResult;
    try {
      const detail = await check(playground.core, playground);
      result = { id, title, pass: true, detail };
    } catch (error) {
      result = { id, title, pass: false, detail: error instanceof Error ? error.message : String(error) };
    }
    results.push(result);
    log(`UNDRA-RN CHECK ${id} ${result.pass ? 'PASS' : 'FAIL'} ${title}: ${result.detail}`);
  }
  const passed = results.filter(r => r.pass).length;
  log(`UNDRA-RN CHECKS ${passed}/${results.length} passed`);
  return results;
}
