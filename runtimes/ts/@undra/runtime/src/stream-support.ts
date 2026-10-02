// PROTOTYPE (ADR-057 lever d1/e): everything a stream needs of the core, out of the first chunk. The generated entry of a
// core whose schema has a stream passes `features: [streams]`; a core loaded without it loads this module on its first stream.
import type { CallTargetArg, PendingStream, UndraCore } from "./core.js";
import { UndraReplyError, UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";
import { StreamCall } from "./stream.js";
import { Kind, ReplyStatus, StreamFlag, type StreamFailure, decodeStreamFailure, encodeCancel, encodeStreamCredit, streamFailureReplyBody } from "./wire/index.js";

/** What `UndraCore.stream` and the transport's stream items are handed to. */
export interface StreamSupport {
  open(core: UndraCore, target: CallTargetArg, methodId: number, args: Uint8Array): StreamCall;
  item(core: UndraCore, payload: Uint8Array): void;
}

interface Entry extends PendingStream {
  readonly stream: StreamCall;
}

export const streams: StreamSupport = {
  open(core, target, methodId, args) {
    const { pending, transport, callId: alloc, encode, closedMessage } = core._internals;
    const callId = core.closed ? 0 : alloc();
    const stream = new StreamCall(callId, {
      sendCredit: (id, credit) => {
        if (core.closed) throw new UndraTransportError("closed", closedMessage);
        transport.send(Kind.StreamCredit, encodeStreamCredit({ callId: id, credit }));
      },
      cancel: (id) => {
        pending.delete(id);
        if (core.closed) return;
        transport.send(Kind.Cancel, encodeCancel({ callId: id }));
      },
    });
    if (core.closed) {
      stream.fail(new UndraTransportError("closed", closedMessage));
      return stream;
    }
    const entry: Entry = {
      kind: "stream",
      stream,
      reject: (error) => stream.fail(error),
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
      transport.send(Kind.Call, encode(target, methodId, callId, args));
    } catch (error) {
      pending.delete(callId);
      stream.fail(error);
    }
    return stream;
  },

  item(core, payload) {
    const { pending } = core._internals;
    if (payload.length < 5) {
      core.report(new UndraTransportError("protocol", "the core sent a truncated stream item"), "stream");
      return;
    }
    const callId = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0, true);
    const flag = payload[4] as number;
    const entry = pending.get(callId) as Entry | undefined;
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
        pending.delete(callId);
        entry.stream.fail(new UndraReplyError(ReplyStatus.Error, body));
        return;
      case StreamFlag.Failed: {
        pending.delete(callId);
        let failure: StreamFailure;
        try {
          failure = decodeStreamFailure(body);
        } catch (error) {
          entry.stream.fail(new UndraTransportError("protocol", `the core sent a malformed stream failure: ${errorMessage(error)}`, { cause: error }));
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
