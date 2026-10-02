import { describe, expect, it } from "vitest";
import { loadWebBudgets, parseWebBudgets } from "../../../../../scripts/web-budgets.mjs";
import { WEB_BUDGET_IDS } from "./ops";

describe("the web call path's budgets (bench/budgets.toml)", () => {
  it("reads the [web.\"id\"] tables and nothing else", () => {
    const rows = parseWebBudgets(
      [
        '[meta]\nmachine = "x"\n',
        '[bench."wire/u8/roundtrip"]\nbudget_ns = 250\n',
        '[web."sync_call"]  # a comment\nbudget_ns = 3_900\nmeasured_ns = 780\nwhat = "a call"\n',
        '[web."record_1kb"]\nbudget_ns = 14000\n',
        '[size."web/hello-wasm"]\nbudget_gzip_bytes = 120000\n',
      ].join("\n"),
    );
    expect(rows).toEqual({ sync_call: { budgetNs: 3900, measuredNs: 780 }, record_1kb: { budgetNs: 14000, measuredNs: undefined } });
  });

  it("refuses a table without a budget", () => {
    expect(() => parseWebBudgets('[web."a"]\nmeasured_ns = 4\n')).toThrow(/no budget_ns/);
  });

  it("has a budget for every row the device bench holds to one, and each is at least what was measured", () => {
    const rows = loadWebBudgets();
    for (const id of WEB_BUDGET_IDS) {
      const row = rows[id];
      expect(row, `bench/budgets.toml has no [web."${id}"]`).toBeDefined();
      if (row?.measuredNs !== undefined) expect(row.budgetNs).toBeGreaterThanOrEqual(row.measuredNs);
    }
  });
});
