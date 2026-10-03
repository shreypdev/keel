import { UndraError } from "./errors.js";
import { msg } from "./messages.js";

/*
 * The two error classes only a mode or an operation throws: the dev server forgot this core's objects (a `remote` core), a snapshot
 * was refused (`restore`, and crash recovery). They are modules of their own so that a page whose core is in this thread does not
 * carry them (ADR-057); the package root still exports them, and the first chunk tells them apart by `kind` (`"sessionLost"`,
 * `"restore"`), not by `instanceof`.
 */

/**
 * The dev server no longer holds the objects of this core (ADR-051): it was restarted (`undra dev`
 * rebuilt the core) or the session's grace period passed while the client was away. The handles of
 * every store and object of this core are dead; load a new core and create them again. A core
 * reports this as `closed` with reason `"sessionLost"`; the page of a web app reloads.
 */
export class UndraSessionLostError extends UndraError {
  override readonly name: string = "UndraSessionLostError";

  /** @param message The server's reason, when it gave one. */
  constructor(message = msg(84)) {
    super("sessionLost", message);
  }
}

/**
 * `UndraCore.restore` was refused: the core rejected the snapshot and is unchanged (SPEC 5.9, the
 * `undra_restore` code of SPEC 7). A refused restore changes nothing, so the core and every handle
 * keep working. The Swift runtime has the same error (`UndraRestoreError`).
 */
export class UndraRestoreError extends UndraError {
  override readonly name: string = "UndraRestoreError";
  /** `undra_restore` code 2: a store's restore function panicked (contained). */
  static readonly PANICKED = 2;
  /** `undra_restore` code 5: the snapshot is malformed (also one in a layout before ADR-037), names an unknown store type, has a null or duplicate handle, or a store rejected its values. */
  static readonly BAD_SNAPSHOT = 5;
  /** `undra_restore` code 6: no running core, it is shut down, or the restore was made from inside a core callback. */
  static readonly UNAVAILABLE = 6;
  /**
   * `undra_restore` code 7 (ADR-037): a store's persisted values cannot become this build's types; they neither
   * migrate structurally nor through a `#[undra::migrate]` hook. The reason is in the ERROR record the core logged.
   */
  static readonly INCOMPATIBLE = 7;
  /**
   * The non-zero code `undra_restore` returned: {@link UndraRestoreError.PANICKED} (2) a store's restore panicked,
   * {@link UndraRestoreError.BAD_SNAPSHOT} (5) the snapshot is malformed or names something the core does not have,
   * {@link UndraRestoreError.UNAVAILABLE} (6) the core is shut down or was called from inside a callback,
   * {@link UndraRestoreError.INCOMPATIBLE} (7) a store's persisted values cannot become this build's types (ADR-037).
   */
  readonly code: number;

  /** @param code The non-zero code `undra_restore` returned. */
  constructor(code: number) {
    super(
      "restore",
      msg(85, String(code)),
    );
    this.code = code;
  }
}
