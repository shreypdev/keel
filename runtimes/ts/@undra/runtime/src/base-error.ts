/*
 * The root of the error hierarchy, in a file of its own so that the wire layer (`WireError`) can extend
 * it without an import cycle (`errors.ts` reads reply bodies with the wire layer's reader). Import it from
 * `errors.ts` or the package root.
 */

/**
 * Base class of every Undra error. `kind` is a short, stable, camelCase
 * discriminant; the runtime's own errors use `"reply"`, `"mode"`,
 * `"schemaMismatch"`, `"port"`, `"transport"`, `"restore"`, `"observe"` and `"state"`.
 */
export class UndraError extends Error {
  override readonly name: string = "UndraError";
  /** Stable discriminant of this error. */
  readonly kind: string;

  /**
   * @param kind Stable discriminant.
   * @param message Human-readable description.
   * @param options Standard `Error` options (`cause`).
   */
  constructor(kind: string, message?: string, options?: ErrorOptions) {
    super(message, options);
    this.kind = kind;
  }
}
