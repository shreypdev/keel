export const BUDGETS_PATH: string;
export interface WebBudget {
  /** The p50 per operation, in nanoseconds, above which a harness fails. */
  budgetNs: number;
  /** The p50 measured when the budget was set. */
  measuredNs: number | undefined;
}
export function parseWebBudgets(text: string): Record<string, WebBudget>;
/** `UNDRA_BENCH_SCALE` of `env` (1 when unset); a value that is not a positive number throws. */
export function benchScale(env?: Record<string, string | undefined>): number;
/** The web budgets, each `budgetNs` multiplied by `scale` (default: `UNDRA_BENCH_SCALE`). `measuredNs` is never scaled. */
export function loadWebBudgets(scale?: number): Record<string, WebBudget>;
