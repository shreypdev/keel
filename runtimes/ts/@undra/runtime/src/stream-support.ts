import { type CallTargetArg, encodeTarget } from "./call-head.js";
import type { UndraCore } from "./core.js";
import { UndraReplyError, UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";
import { StreamCall } from "./stream.js";
import {
  Kind,
  ReplyStatus,
  StreamFlag,
  type StreamFailure,
  decodeStreamFailure,
  encodeCancel,
  encodeStreamCredit,
  streamFailureReplyBody,
} from "./wire/index.js";

/*
 * Everything a core needs to run streams (`UndraCore.stream`, SPEC 3.7), as a feature (ADR-057): a hello page has no stream,
 * so the code is not in its first chunk. The generated entry of a schema that has a stream method or function passes
 * `features: [streams]` to `UndraCore.load` and `attach`, which has the support in the page's first chunk (the entry imports
 * it); a core loaded without the feature (by hand, or with bindings older than ADR-057) loads this module at its first stream,
 * so nothing that worked stops working.
 */

/**
 * A part of the runtime that a core loads only when its schema needs it: give the ones the bindings name to
 * `AttachOptions.features`. Generated entries do it themselves; `streams` is the one that exists today.
 */
export interface UndraFeature {
  /** Called once by the core that is given the feature, before it starts. @internal */
  _install(core: UndraCore): void;
}

/** What `UndraCore.stream` and the transport's stream items are handed to once the feature is installed. @internal */
export interface StreamSupport {
  /** Opens a stream: the `Call` goes out now, the items arrive through {@link StreamSupport.item}. */
  open(core: UndraCore, target: CallTargetArg, methodId: number, args: Uint8Array): StreamCall;
  /** Hands the core's `StreamItem` message to the stream it belongs to. */
  item(core: UndraCore, payload: Uint8Array): void;
}

/** The pending-map entry of an open stream: the core hands it the reply to its call and any failure of the channel. @internal */
export interface PendingStream {
  readonly kind: "stream";
  readonly stream: StreamCall;
  /** The `Reply` to the stream's call: `StreamOpened`, or why it did not open. */
  reply(status: number, body: Uint8Array): void;
  /** The channel failed (the core closed, the connection dropped): the stream fails with `failure`. */
  reject(failure: unknown): void;
  readonly cleanup?: undefined;
}

const support: StreamSupport = {
  open(core, target, methodId, args) {
    const pending = core._pending;
    const callId = core.closed ? 0 : core._allocCallId();
    const stream = new StreamCall(callId, {
      sendCredit: (id, credit) => {
        core._assertOpen();
        core._transport.send(Kind.StreamCredit, encodeStreamCredit({ callId: id, credit }));
      },
      cancel: (id) => {
        pending.delete(id);
        if (core.closed) return;
        core._transport.send(Kind.Cancel, encodeCancel({ callId: id }));
      },
    });
    if (core.closed) {
      stream.fail(new UndraTransportError("closed", core._closedMessage));
      return stream;
    }
    const entry: PendingStream = {
      kind: "stream",
      stream,
      reject: (failure) => stream.fail(failure),
      reply(status, body) {
        if (status === ReplyStatus.StreamOpened) {
          stream.opened();
          return;
        }
        pending.delete(callId);
        stream.fail(
          status > ReplyStatus.BadRequest
            ? new UndraTransportError("protocol", `the core sent reply status ${status}`)
            : status === ReplyStatus.Ok
              ? new UndraTransportError("protocol", "the core answered a stream call with a plain result")
              : new UndraReplyError(status as ReplyStatus, body),
        );
      },
    };
    pending.set(callId, entry);
    try {
      core._transport.send(Kind.Call, encodeTarget(target, methodId, callId, args));
    } catch (error) {
      pending.delete(callId);
      stream.fail(error);
    }
    return stream;
  },

  item(core, payload) {
    const pending = core._pending;
    if (payload.length < 5) {
      core.report(new UndraTransportError("protocol", "the core sent a truncated stream item"), "stream");
      return;
    }
    const callId = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0, true);
    const flag = payload[4] as number;
    const entry = pending.get(callId);
    if (entry?.kind !== "stream") return;
    const body = payload.subarray(5);
    switch (flag) {
      case StreamFlag.Item:
        entry.stream.push(body);
        return;
      case StreamFlag.End:
        pending.delete(callId);
        entry.stream.end();
        return;
      case StreamFlag.Error:
        // The stream's own `E`; generated code decodes it.
        pending.delete(callId);
        entry.stream.fail(new UndraReplyError(ReplyStatus.Error, body));
        return;
      case StreamFlag.Failed: {
        // Panicked, cancelled by the core or refused: exactly the failed reply with that status (ADR-036).
        pending.delete(callId);
        let failure: StreamFailure;
        try {
          failure = decodeStreamFailure(body);
        } catch (error) {
          entry.stream.fail(
            new UndraTransportError("protocol", `the core sent a malformed stream failure: ${errorMessage(error)}`, { cause: error }),
          );
          return;
        }
        entry.stream.fail(new UndraReplyError(failure.status, streamFailureReplyBody(failure)));
        return;
      }
      default:
        pending.delete(callId);
        entry.stream.fail(new UndraTransportError("protocol", `the core sent stream flag ${flag}`));
    }
  },
};

/**
 * The stream support of the runtime, as the feature the generated entry of a schema with a stream passes
 * (`features: [streams]`).
 */
export const streams: UndraFeature = {
  _install(core) {
    core._streams = support;
  },
};
