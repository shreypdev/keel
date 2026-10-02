import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** The checked-in fixtures every kit reads (testkit/fixtures, testkit/conformance). */
export function fixture(name: string): string {
  return readFileSync(fileURLToPath(new URL(`../../../../../../testkit/${name}`, import.meta.url)), "utf8");
}
