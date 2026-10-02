import { expect, test } from "vitest";
import {
  CallTarget,
  ChangeOp,
  Decimal,
  ReplyStatus,
  UndraReader,
  UndraReplyError,
  UndraWriter,
  applyPatch,
  codecs,
  decimalCodec,
  decodePatch,
  decodeValue,
  durationToNanos,
  encodeValue,
  type PatchOp,
} from "@undra/runtime";
import {
  type Account,
  AccountCodec,
  AccountId,
  AccountIdCodec,
  Cents,
  CentsCodec,
  Ledger,
  LedgerError,
  Price,
  PriceCodec,
  type Receipt,
  UndraIds,
  balances,
  deposit,
  echoAccount,
  echoCents,
  echoDecimal,
  echoPrice,
  echoReceipt,
  loadableStatement,
  openAccount,
  sampleReceipt,
  statement,
} from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, type SignalUpdate, args } from "../src/raw-store.js";
import { step } from "../src/wait.js";

// S31 newtypes, generic instantiations and leaf types (ADR-042): the ledger core, through the generated API only
// (the raw API where a step says so). A newtype is its inner value on the wire and a distinct type on the platform
// (a branded string, bigint or Decimal); a map and a keyed list are keyed by one; a generic instantiation is a plain
// record or enum; a Decimal is exact; the leaf types of other crates arrive as wire types.

const Ledger_ = UndraIds.Objects.Ledger;

/** The bytes `codec` writes for `value`. */
const bytesOf = <T>(codec: { encode(w: UndraWriter, v: T): void }, value: T): Uint8Array => {
  const w = new UndraWriter();
  codec.encode(w, value);
  return w.finish();
};

/** A price from its text (`Decimal.parse` keeps the scale: `1.50` has scale 2). */
const price = (text: string): Price => Price(Decimal.parse(text));

/** The one `accounts` entry of a change-set, as a keyed patch's operations. */
function accountOps(entries: readonly SignalUpdate[]): PatchOp<Account>[] {
  const found = entries.filter((e) => e.signalId === 0);
  expect(found, "entries for the accounts signal").toHaveLength(1);
  const entry = found[0] as SignalUpdate;
  expect(entry.op, "the entry is a keyed patch").toBe(ChangeOp.KeyedPatch);
  expect(entry.value.length, "a patch of one operation is a few bytes").toBeLessThan(64);
  const r = new UndraReader(entry.value);
  const ops = decodePatch(r, AccountCodec);
  r.finish();
  return ops;
}

// The platform type is a distinct type: none of these compiles (`tsc` checks this file; the function is never called).
export function distinctTypes(): void {
  // @ts-expect-error a string is not an AccountId until it is branded
  const id: AccountId = "1ed9e400-0000-0000-0000-000000000007";
  // @ts-expect-error a bigint is not Cents until it is branded
  const cents: Cents = 5n;
  // @ts-expect-error an AccountId is not a Cents, and neither is the other way round
  const mixed: Cents = AccountId("1ed9e400-0000-0000-0000-000000000007");
  // @ts-expect-error a Decimal is not a Price until it is branded
  const money: Price = Decimal.ZERO;
  void [id, cents, mixed, money];
}

test("S31 newtypes, generic instantiations and leaf types", async () => {
  const { core, runtimeErrors } = await boot();
  const ada = await openAccount("Ada", core);
  const bob = await openAccount("Bob", core);

  await step("1. a newtype is its inner value on the wire", async () => {
    const uuid = "1ed9e400-0000-0000-0000-0000000000a5";
    const id = AccountId(uuid);
    expect(bytesOf(AccountIdCodec, id)).toEqual(bytesOf(codecs.uuid, uuid));
    expect(bytesOf(AccountIdCodec, id)).toHaveLength(16);
    expect(bytesOf(CentsCodec, Cents(-5n))).toEqual(bytesOf(codecs.i64, -5n));
    expect(bytesOf(CentsCodec, Cents(-5n))).toHaveLength(8);
    const dec = Decimal.parse("-19.99");
    expect(bytesOf(PriceCodec, Price(dec))).toEqual(bytesOf(decimalCodec, dec));
    expect(bytesOf(PriceCodec, Price(dec))).toHaveLength(17);

    const echoed = await echoAccount(id, core);
    expect(echoed).toBe(id);
    // The brand is only a type: at run time the value is its inner value.
    expect(echoed === uuid).toBe(true);
    expect(typeof echoed).toBe("string");
    const cents = await echoCents(Cents(-5n), core);
    expect(cents === -5n).toBe(true);
    expect(typeof cents).toBe("bigint");
    expect(Cents(1n) < Cents(2n)).toBe(true);
    const p = await echoPrice(Price(Decimal.parse("1234.5600")), core);
    expect(p).toBeInstanceOf(Decimal);
    expect(p.equals(Decimal.parse("1234.5600"))).toBe(true);
    expect(p.scale).toBe(4);
    // The newtype of a newtype's inner type is the one codec: a decoded value is the plain value, branded.
    expect(decodeValue(AccountIdCodec, encodeValue(codecs.uuid, uuid))).toBe(uuid);
  });

  await step("2a. a newtype is a map key: balances() is a Map with exactly the two AccountId keys", async () => {
    expect(ada).not.toBe(bob);
    const all = await balances(core);
    expect(all).toBeInstanceOf(Map);
    expect([...all.keys()].sort()).toEqual([ada, bob].sort());
    // The keys are the strings the accounts were opened with: lookups by the branded value work.
    expect(all.get(ada)?.toString()).toBe("0");
    expect(all.get(bob)?.toString()).toBe("0");
    expect(all.size).toBe(2);
  });

  await step("2b. a keyed list keyed by a newtype: one Insert, one Update, one Remove, and the mirrored list follows", async () => {
    const raw = await RawStore.open(core, Ledger_);
    const store = await Ledger.create(core);
    const model: Account[] = [];
    const initial = raw.take();
    const first = initial.find((e) => e.signalId === 0) as SignalUpdate;
    expect(first.op).toBe(ChangeOp.FullValue);
    let mirror: Account[] = decodeValue(codecs.vec(AccountCodec), first.value);
    expect(mirror).toEqual([]);
    expect(store.accounts.peek()).toEqual([]);

    // open("Ada"): one Insert
    const adaId = decodeValue(AccountIdCodec, await raw.call(Ledger_.open, args((w) => w.writeStr("Ada"))));
    const adaHost = await store.open("Ada");
    model.push({ id: adaId, owner: "Ada" });
    let ops = accountOps(raw.take());
    expect(ops).toEqual([{ op: "insert", index: 0, item: { id: adaId, owner: "Ada" } }]);
    mirror = applyPatch(mirror, ops);
    expect(mirror).toEqual(model);
    expect(store.accounts.peek().map((a) => a.owner)).toEqual(["Ada"]);
    expect(store.accounts.peek()[0]?.id).toBe(adaHost);

    const bobId = decodeValue(AccountIdCodec, await raw.call(Ledger_.open, args((w) => w.writeStr("Bob"))));
    await store.open("Bob");
    model.push({ id: bobId, owner: "Bob" });
    ops = accountOps(raw.take());
    expect(ops).toHaveLength(1);
    expect(ops[0]?.op).toBe("insert");
    mirror = applyPatch(mirror, ops);
    expect(mirror).toEqual(model);

    // rename(ada, "Ada L."): one Update of the row the newtype names
    await raw.call(
      Ledger_.rename,
      args((w) => {
        AccountIdCodec.encode(w, adaId);
        w.writeStr("Ada L.");
      }),
    );
    await store.rename(adaHost, "Ada L.");
    model[0] = { id: adaId, owner: "Ada L." };
    ops = accountOps(raw.take());
    expect(ops).toEqual([{ op: "update", index: 0, item: { id: adaId, owner: "Ada L." } }]);
    mirror = applyPatch(mirror, ops);
    expect(mirror).toEqual(model);
    expect(store.accounts.peek().map((a) => a.owner)).toEqual(["Ada L.", "Bob"]);

    // close_account(ada): one Remove
    await raw.call(Ledger_.closeAccount, args((w) => AccountIdCodec.encode(w, adaId)));
    await store.closeAccount(adaHost);
    model.splice(0, 1);
    ops = accountOps(raw.take());
    expect(ops).toEqual([{ op: "remove", index: 0 }]);
    mirror = applyPatch(mirror, ops);
    expect(mirror).toEqual(model);
    expect(store.accounts.peek().map((a) => a.owner)).toEqual(["Bob"]);
    // Renaming or closing an account that is not there changes nothing and sends nothing.
    await raw.call(Ledger_.closeAccount, args((w) => AccountIdCodec.encode(w, adaId)));
    expect(raw.take()).toEqual([]);
    raw.close();
    store.close();
  });

  await step("3. generic instantiations are plain records and enums", async () => {
    const account = await openAccount("Eve", core);
    const base = 1_790_856_000_000;
    for (let i = 0; i < 5; i++) await deposit(account, price("1.50"), `deposit ${i}`, base + i, core);
    const first = await statement(account, 0, 2, core);
    expect(first.items).toHaveLength(2);
    expect(first.next).toBe(2);
    expect(first.items.map((e) => e.memo)).toEqual(["deposit 0", "deposit 1"]);
    expect(first.items.map((e) => e.at)).toEqual([base, base + 1]);
    expect(first.items.every((e) => e.account === account)).toBe(true);
    expect(first.items[0]?.amount.toString()).toBe("1.50");
    const last = await statement(account, 4, 10, core);
    expect(last.items).toHaveLength(1);
    expect(last.next).toBeNull();
    expect(last.items[0]?.memo).toBe("deposit 4");
    expect((await statement(account, 9, 10, core)).items).toEqual([]);

    expect(await loadableStatement(account, false, 3, core)).toEqual({ kind: "loading" });
    const loaded = await loadableStatement(account, true, 3, core);
    expect(loaded.kind).toBe("loaded");
    if (loaded.kind === "loaded") {
      expect(loaded.value.items).toHaveLength(3);
      expect(loaded.value.next).toBe(3);
    }
    const unknown = AccountId("1ed9e400-0000-0000-0000-0000000000ff");
    expect(await loadableStatement(unknown, true, 3, core)).toEqual({ kind: "failed", value: "no such account" });
  });

  await step("4. decimals are exact", async () => {
    const account = await openAccount("Dee", core);
    const one = await deposit(account, price("0.1"), "a", 1, core);
    expect(one.toString()).toBe("0.1");
    const two = await deposit(account, price("0.2"), "b", 2, core);
    // The point of a decimal type: 0.1 + 0.2 is 0.3 (a double says 0.30000000000000004).
    expect(two.toString()).toBe("0.3");
    expect(two.equals(Decimal.parse("0.3"))).toBe(true);
    expect((await balances(core)).get(account)?.toString()).toBe("0.3");

    const same = async (d: Decimal): Promise<void> => {
      const back = await echoDecimal(d, core);
      expect(back).toBeInstanceOf(Decimal);
      expect([back.mantissa, back.scale], d.toString()).toEqual([d.mantissa, d.scale]);
      expect(back.equals(d)).toBe(true);
    };
    for (const text of ["0", "1.10", "-1.50"]) await same(Decimal.parse(text));
    await same(new Decimal(1n, 38));
    await same(new Decimal(-(2n ** 127n), 0)); // the smallest 128-bit mantissa
    await same(new Decimal(2n ** 127n - 1n, 0)); // the largest
    expect((await echoDecimal(Decimal.parse("1.10"), core)).toString()).toBe("1.10");

    // A wire decimal with a scale of 39, through the raw API: the core refuses it as a bad request.
    const bad = new Uint8Array(17);
    bad[16] = 39;
    const refused = await core.call({ target: CallTarget.FreeFunction }, UndraIds.Functions.echoDecimal, bad).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(UndraReplyError);
    expect((refused as UndraReplyError).status).toBe(ReplyStatus.BadRequest);

    // An amount rust_decimal cannot hold fails with the ledger's typed error.
    const tooBig = await deposit(account, Price(new Decimal(2n ** 127n - 1n, 0)), "huge", 3, core).catch((e: unknown) => e);
    expect(tooBig).toBeInstanceOf(LedgerError.OutOfRange);
    const nobody = await deposit(AccountId("1ed9e400-0000-0000-0000-0000000000fe"), price("1"), "x", 4, core).catch((e: unknown) => e);
    expect(nobody).toBeInstanceOf(LedgerError.NoSuchAccount);
    // The failed deposits changed nothing.
    expect((await balances(core)).get(account)?.toString()).toBe("0.3");
  });

  await step("5. leaf types of other crates cross as wire types", async () => {
    const sample = await sampleReceipt(core);
    expect(sample.id).toBe("1ed9e400-0000-0000-0000-000000000007");
    expect(sample.issued).toBe(1_790_856_000_123);
    expect(new Date(sample.issued).toISOString()).toBe("2026-10-01T12:00:00.123Z");
    expect(durationToNanos(sample.validFor)).toBe(90_000_000_000n);
    expect(sample.total).toBeInstanceOf(Decimal);
    expect([sample.total.mantissa, sample.total.scale]).toEqual([19_990n, 3]);
    expect(sample.total.toString()).toBe("19.990");
    expect(sample.signature).toEqual(Uint8Array.of(1, 2, 3, 255));

    const mine: Receipt = {
      id: "01020304-0506-0708-090a-0b0c0d0e0f10",
      issued: 1_700_000_000_007,
      validFor: 3_600_000.5,
      total: Decimal.parse("-0.0010"),
      signature: Uint8Array.of(0, 128, 255),
    };
    const back = await echoReceipt(mine, core);
    expect(back).toEqual(mine);
    expect(back.total.scale).toBe(4);
    expect(await echoReceipt(sample, core)).toEqual(sample);
  });

  expect(runtimeErrors, "nothing was reported without a caller").toEqual([]);
});
