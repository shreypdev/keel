/** PROTOTYPE (ADR-057 lever b): what a production build says instead of a sentence: the code, the values, where to read it. */
export function __m(code: number, ...args: unknown[]): string {
  return `Undra #${code}${args.length > 0 ? ` [${args.join(", ")}]` : ""}; see https://undra.dev/e/${code}`;
}
