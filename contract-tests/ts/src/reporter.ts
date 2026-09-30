import type { Reporter, TestCase, TestModule } from "vitest/node";

/** Notes a scenario attaches to its result (`task.meta.notes`), printed after the scenario lines. */
declare module "vitest" {
  interface TaskMeta {
    /** Measurements and remarks a scenario wants in the run's output (S03 prints its nanoseconds per call here). */
    notes?: string[];
  }
}

/** `S07 stream with backpressure` -> id `S07`, title `stream with backpressure`. */
const SCENARIO_NAME = /^(S\d{2}) (.+)$/;

/** The first line of a failure, where the assertion says what was wrong. */
function firstLine(message: string | undefined): string {
  return (message ?? "no message").split("\n")[0]?.trim() ?? "no message";
}

/**
 * Prints the result of every scenario as one line, the format `contract-tests/check.sh` grades:
 *
 * ```text
 * SCENARIO S07 PASS stream with backpressure
 * SCENARIO S12 FAIL query: fetch, stale, refetch: [step 3.] expected 2 to be 3
 * ```
 *
 * A test is a scenario when its name starts with its id (`S07 ...`). Other tests (the harness's
 * own) only count in vitest's totals. The lines come at the end of the run, in scenario order,
 * each at the start of a line.
 */
export default class ScenarioReporter implements Reporter {
  onTestRunEnd(testModules: ReadonlyArray<TestModule>): void {
    const scenarios: { id: string; line: string; notes: string[] }[] = [];
    for (const module of testModules) {
      for (const test of module.children.allTests()) {
        const named = SCENARIO_NAME.exec(test.name);
        if (named === null) continue;
        const [, id, title] = named as unknown as [string, string, string];
        scenarios.push({ id, line: `SCENARIO ${id} ${outcome(test, title)}`, notes: test.meta().notes ?? [] });
      }
    }
    scenarios.sort((a, b) => a.id.localeCompare(b.id));
    const out = process.stdout;
    out.write("\n");
    for (const scenario of scenarios) out.write(`${scenario.line}\n`);
    for (const scenario of scenarios) for (const note of scenario.notes) out.write(`NOTE ${scenario.id} ${note}\n`);
  }
}

function outcome(test: TestCase, title: string): string {
  const result = test.result();
  switch (result.state) {
    case "passed":
      return `PASS ${title}`;
    case "failed":
      return `FAIL ${title}: ${firstLine(result.errors[0]?.message)}`;
    case "skipped":
      return `SKIP ${title}: ${result.note ?? "skipped"}`;
    case "pending":
      return `FAIL ${title}: did not finish`;
  }
}
