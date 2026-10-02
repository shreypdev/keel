import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/*
 * A runtime-internal member, by the name the build under test gives it. The package's production build renames every private property
 * (`_giveBack` -> `_c`, ADR-057, `scripts/mangle.mjs`), and `UNDRA_TEST_DIST=1 npm run test:dist` runs this suite against that build:
 * the few tests that reach for an internal ask for its name here, and get the same name when the sources are what runs.
 */
const cache: Readonly<Record<string, string>> =
  process.env.UNDRA_TEST_DIST === undefined
    ? {}
    : (JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../../dist/mangle-cache.json"), "utf8")) as Record<string, string>);

/** `name` as the build under test spells it. */
export function internal(name: string): string {
  return cache[name] ?? name;
}

/** The internal `name` of `target`, as a function bound to it. */
export function method<A extends unknown[], R>(target: object, name: string): (...args: A) => R {
  const fn = (target as Record<string, unknown>)[internal(name)];
  if (typeof fn !== "function") throw new Error(`no internal method ${name}`);
  return (...args: A) => (fn as (...a: A) => R).apply(target, args);
}

/** The internal property `name` of `target`. */
export function internalValue<T>(target: object, name: string): T {
  return (target as Record<string, unknown>)[internal(name)] as T;
}
