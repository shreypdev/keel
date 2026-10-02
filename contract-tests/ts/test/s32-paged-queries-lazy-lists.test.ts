import { test } from "vitest";
import { boot } from "../src/harness.js";
import { pagingSteps } from "../src/paging-steps.js";

// S32 paged queries and lazy lists (ADR-043): the Library's two lazy lists (`books`, 10,000 rows the host never receives whole, and
// `evens`, a view of the even rows of a small source), the infinite feed and its keyed patches. Every step goes through the generated
// classes; the page calls are counted at the core's transport (src/page-calls.ts). The steps are written once (src/paging-steps.ts) so
// that `paging-worker.test.ts` can run them with the core in a worker.

test("S32 paged queries and lazy lists", async () => {
  const { core } = await boot();
  await pagingSteps(core, { sync: true });
});
