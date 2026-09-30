/**
 * Typed errors of the wire layer (docs/SPEC.md section 3.9).
 *
 * Decoding never throws anything but {@link WireError}: malformed, truncated
 * or hostile input is reported as a typed value with the offset at which it
 * was detected. Encoders throw `RangeError` for arguments that cannot be
 * represented (for example a `u8` of 300) because that is a programming
 * error in the caller, not a property of the wire data; the only encoder-side
 * `WireError`s are `length_too_large`, `negative_duration` and `duplicate_key`.
 */

/**
 * Structured description of a wire failure. The variants mirror the Rust
 * `WireError` enum of `undra-wire`; `duplicate_key`, `negative_duration` and
 * `unsafe_integer` are additions that the TypeScript layer needs (see each
 * variant). Offsets (`at`) are byte positions relative to the start of the
 * buffer handed to the reader.
 */
export type WireErrorDetail =
  /** The input ended before a value was complete. */
  | { readonly code: "unexpected_eof"; readonly needed: number; readonly at: number }
  /** A string payload is not well-formed UTF-8. */
  | { readonly code: "invalid_utf8"; readonly at: number }
  /** A tag (bool, option, result, enum variant, kind, status, op...) has no meaning. `ty` names the type being decoded. */
  | {
      readonly code: "invalid_tag";
      readonly tag: number;
      readonly at: number;
      readonly ty: string;
    }
  /** A length or count exceeds what the remaining input (or the u32 range) can hold. */
  | { readonly code: "length_too_large"; readonly len: number; readonly at: number }
  /** A message was fully decoded but bytes are left over. */
  | { readonly code: "trailing_bytes"; readonly count: number }
  /** An envelope does not start with `UNDRA`. */
  | { readonly code: "bad_magic" }
  /** An envelope carries a wire version this runtime does not speak. */
  | { readonly code: "unsupported_version"; readonly version: number }
  /** The peer was built from a different schema than this runtime expects. */
  | { readonly code: "schema_mismatch"; readonly expected: bigint; readonly got: bigint }
  /** A map contains the same key twice. `at` is the offset of the second occurrence. */
  | { readonly code: "duplicate_key"; readonly at: number }
  /** A `Duration` is negative; durations are unsigned in every language. */
  | { readonly code: "negative_duration"; readonly nanos: bigint }
  /**
   * A 64-bit integer that the caller asked to read as a JS `number` (a
   * `#[undra(js_number)]` field or a `Timestamp`) is outside the safe integer range.
   */
  | { readonly code: "unsafe_integer"; readonly value: bigint; readonly at: number };

/** Discriminant of {@link WireErrorDetail}. */
export type WireErrorCode = WireErrorDetail["code"];

function describe(d: WireErrorDetail): string {
  switch (d.code) {
    case "unexpected_eof":
      return `unexpected end of input at offset ${d.at}: needed ${d.needed} more byte${d.needed === 1 ? "" : "s"}`;
    case "invalid_utf8":
      return `invalid UTF-8 in string at offset ${d.at}`;
    case "invalid_tag":
      return `invalid ${d.ty} tag ${d.tag} at offset ${d.at}`;
    case "length_too_large":
      return `length ${d.len} at offset ${d.at} exceeds the available input`;
    case "trailing_bytes":
      return `${d.count} trailing byte${d.count === 1 ? "" : "s"} after the end of the message`;
    case "bad_magic":
      return "bad magic: envelope does not start with \"UNDRA\"";
    case "unsupported_version":
      return `unsupported wire version ${d.version}`;
    case "schema_mismatch":
      return `schema mismatch: expected 0x${d.expected.toString(16).padStart(16, "0")}, got 0x${d.got.toString(16).padStart(16, "0")}`;
    case "duplicate_key":
      return `duplicate map key at offset ${d.at}`;
    case "negative_duration":
      return `negative duration (${d.nanos} ns)`;
    case "unsafe_integer":
      return `integer ${d.value} at offset ${d.at} is outside the JS safe integer range`;
  }
}

/**
 * Error thrown by every wire decoder for malformed input.
 *
 * Narrow on `detail.code` to reach the structured fields:
 *
 * ```ts
 * try { decodeEnvelope(bytes); } catch (e) {
 *   if (e instanceof WireError && e.detail.code === "unsupported_version") {
 *     console.error(e.detail.version);
 *   }
 * }
 * ```
 */
export class WireError extends Error {
  /** Always `"WireError"`. */
  override readonly name = "WireError";
  /** Discriminant, identical to `detail.code`. */
  readonly code: WireErrorCode;
  /** Structured fields of this failure. */
  readonly detail: WireErrorDetail;

  /** @param detail What went wrong; the message is derived from it. */
  constructor(detail: WireErrorDetail) {
    super(`wire: ${describe(detail)}`);
    this.code = detail.code;
    this.detail = detail;
  }
}

/**
 * Raised by `applyPatch` when a keyed-patch operation does not fit the list
 * it is applied to. This means the host mirror and the core disagree about the
 * list state; the runtime must resynchronise (re-observe the signal) rather
 * than continue with a corrupted mirror.
 */
export class PatchError extends Error {
  /** Always `"PatchError"`. */
  override readonly name = "PatchError";
  /** Zero-based position of the failing operation within the patch. */
  readonly opIndex: number;
  /** Name of the failing operation (`insert`, `remove`, `update` or `move`). */
  readonly op: string;
  /** The offending index (`from` or `to` for a move). */
  readonly index: number;
  /** Length of the list when the operation was attempted. */
  readonly length: number;

  /** @param opIndex Position of the failing operation. @param op Its name. @param index The offending index. @param length The list length at that point. */
  constructor(opIndex: number, op: string, index: number, length: number) {
    super(
      `keyed patch operation #${opIndex} (${op}) index ${index} is out of bounds for a list of length ${length}`,
    );
    this.opIndex = opIndex;
    this.op = op;
    this.index = index;
    this.length = length;
  }
}
