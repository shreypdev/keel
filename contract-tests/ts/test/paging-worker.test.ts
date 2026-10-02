import { test } from "vitest";
import { bootWorker } from "../src/harness.js";
import { pagingSteps } from "../src/paging-steps.js";

// Not a scenario: S32 runs in `wasm-main`; this runs its steps with the core in a worker (ADR-049), where a page call is asynchronous
// (`UndraModeError` from `callSync`, then `core.call`), replies arrive in later messages, and the change-sets of a call and of the
// next can reach the mirror in one drain. The steps that need the core to answer in the caller's turn do not run (src/paging-steps.ts).

test("paged queries and lazy lists in wasm-worker mode", async () => {
  const { core } = await bootWorker();
  await pagingSteps(core, { sync: false });
});
