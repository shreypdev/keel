export const BUDGETS_PATH: string;
export interface WebBudget {
  /** The p50 per operation, in nanoseconds, above which a harness fails. */
  budgetNs: number;
  /** The p50 measured when the budget was set. */
  measuredNs: number | undefined;
}
export function parseWebBudgets(text: string): Record<string, WebBudget>;
export function loadWebBudgets(): Record<string, WebBudget>;
