import type { KeelCore } from "../../src/core.js";
import { KeelStore } from "../../src/object.js";
import { Signal } from "../../src/signal.js";
import {
  ALL_SIGNALS,
  ChangeOp,
  KeelReader,
  KeelWriter,
  applyPatch,
  codecs,
  decodePatch,
  decodeValue,
  encodeValue,
} from "../../src/wire/index.js";

/** Encodes a `u32`. */
export const u32 = (value: number): Uint8Array => encodeValue(codecs.u32, value);
/** Encodes a `String`. */
export const str = (value: string): Uint8Array => encodeValue(codecs.string, value);
/** Encodes a `Vec<u32>`. */
export const vecU32 = (value: number[]): Uint8Array => encodeValue(codecs.vec(codecs.u32), value);

/**
 * A store written the way `keel-bindgen` writes them (see the golden
 * `stores.ts`): three signals, `_apply` decoding full values and keyed patches.
 */
export class CounterStore extends KeelStore {
  readonly count = new Signal<number>(0);
  readonly label = new Signal<string>("");
  readonly items = new Signal<number[]>([]);
  resyncs = 0;

  private constructor(core: KeelCore, handle: bigint) {
    super(core, handle);
    this._signals = [this.count, this.label, this.items];
  }

  static async create(core: KeelCore, handle: bigint): Promise<CounterStore> {
    const store = new CounterStore(core, handle);
    await core.observe(handle, ALL_SIGNALS, true);
    return store;
  }

  /** Constructs without observing, so tests can drive the mirror themselves. */
  static unobserved(core: KeelCore, handle: bigint): CounterStore {
    return new CounterStore(core, handle);
  }

  protected override _apply(signalId: number, op: ChangeOp, value: Uint8Array): void {
    switch (signalId) {
      case 0:
        if (op === ChangeOp.FullValue) this.count._set(decodeValue(codecs.u32, value));
        break;
      case 1:
        if (op === ChangeOp.FullValue) this.label._set(decodeValue(codecs.string, value));
        break;
      case 2:
        if (op === ChangeOp.FullValue) {
          this.items._set(decodeValue(codecs.vec(codecs.u32), value));
        } else if (op === ChangeOp.KeyedPatch) {
          const r = new KeelReader(value);
          const ops = decodePatch(r, codecs.u32);
          r.finish();
          this.items._set(applyPatch(this.items.peek(), ops));
        }
        break;
      default:
        break;
    }
  }
}

export { KeelWriter };
