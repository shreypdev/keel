import { describe, expect, it } from "vitest";
import { PortIds } from "../src/adapters/ids.js";
import * as literals from "../src/adapters/port-literals.js";
import { fnv1a32 } from "../src/fnv.js";

/*
 * The first chunk of a page spells nine standard-port ids as numbers (`adapters/port-literals.ts`, ADR-057): `PortIds` computes
 * every id from its name, and a hello page would otherwise ship the hash and the name of every method to do it at load. The
 * name stays the source (constitution R1): each literal is derived here from it, twice (through `PortIds` and through the
 * rule in SPEC 1.1), so a literal that drifts, or a rule that changes, fails this test.
 */

const pinned: ReadonlyArray<readonly [string, number, number]> = [
  ["HTTP_PORT", literals.HTTP_PORT, PortIds.Http.portId],
  ["KV_PORT", literals.KV_PORT, PortIds.Kv.portId],
  ["SECURE_STORE_PORT", literals.SECURE_STORE_PORT, PortIds.SecureStore.portId],
  ["FS_PORT", literals.FS_PORT, PortIds.Fs.portId],
  ["CONNECTIVITY_PORT", literals.CONNECTIVITY_PORT, PortIds.Connectivity.portId],
  ["CONNECTIVITY_CHANGED", literals.CONNECTIVITY_CHANGED, PortIds.Connectivity.changed],
  ["LIFECYCLE_PORT", literals.LIFECYCLE_PORT, PortIds.Lifecycle.portId],
  ["LIFECYCLE_CHANGED", literals.LIFECYCLE_CHANGED, PortIds.Lifecycle.changed],
  ["TIMER_PORT", literals.TIMER_PORT, PortIds.Timer.portId],
];

describe("port-literals", () => {
  it.each(pinned)("%s is the id PortIds computes from the name", (_name, literal, derived) => {
    expect(literal).toBe(derived);
  });

  it("derives them from the names by the rule of SPEC 1.1 (port.<Trait>, <Trait>.<method>)", () => {
    expect(literals.HTTP_PORT).toBe(fnv1a32("port.Http"));
    expect(literals.KV_PORT).toBe(fnv1a32("port.Kv"));
    expect(literals.SECURE_STORE_PORT).toBe(fnv1a32("port.SecureStore"));
    expect(literals.FS_PORT).toBe(fnv1a32("port.Fs"));
    expect(literals.CONNECTIVITY_PORT).toBe(fnv1a32("port.Connectivity"));
    expect(literals.CONNECTIVITY_CHANGED).toBe(fnv1a32("Connectivity.changed"));
    expect(literals.LIFECYCLE_PORT).toBe(fnv1a32("port.Lifecycle"));
    expect(literals.LIFECYCLE_CHANGED).toBe(fnv1a32("Lifecycle.changed"));
    expect(literals.TIMER_PORT).toBe(fnv1a32("port.Timer"));
  });

  it("is every export of the module (a new literal is pinned here too)", () => {
    expect(Object.keys(literals).sort()).toEqual(pinned.map(([name]) => name).sort());
  });
});
