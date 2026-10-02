/*
 * The message kinds, apart from the envelope codec that frames them: the in-process transport needs the
 * numbers, only the framed transports (`remote`, the worker's) need the framing (ADR-052).
 */

/**
 * Message kind carried in an envelope header (docs/SPEC.md section 3.2). The
 * numeric values are the wire values.
 */
export enum Kind {
  /** host to core: invoke a function, method, constructor or lazy-list page. */
  Call = 1,
  /** core to host: outcome of a call. */
  Reply = 2,
  /** core to host: signal updates of one transaction. */
  ChangeSet = 3,
  /** core to host: invoke a platform port. */
  PortCall = 4,
  /** host to core: outcome of a port call. */
  PortReply = 5,
  /** host to core: cancel an in-flight call or stream. */
  Cancel = 6,
  /** host to core: grant a stream more credit. */
  StreamCredit = 7,
  /** core to host: one stream item, the end, or an error. */
  StreamItem = 8,
  /** host to core: start or stop observing a signal. */
  Observe = 9,
  /** host to core: release an object handle. */
  Release = 10,
  /** host to core: deliver a port event. */
  Event = 11,
  /** both ways: version and schema handshake. */
  Hello = 12,
  /** core to host: a log record. */
  Log = 13,
  /** host to core: a timer set through the Timer port is due. */
  TimerFired = 14,
  /** core to host: a snapshot of every store. */
  Snapshot = 15,
  /** host to core: restore from a snapshot. */
  Restore = 16,
}
