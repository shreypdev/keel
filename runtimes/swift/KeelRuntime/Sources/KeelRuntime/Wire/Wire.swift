/// The namespace of the envelope payload types (docs/SPEC.md sections 3.3 to 3.7 and 5.9):
/// `Wire.Call`, `Wire.Reply`, `Wire.ChangeSet`, `Wire.PortCall`, `Wire.PortReply`, `Wire.Cancel`,
/// `Wire.StreamCredit`, `Wire.StreamItem`, `Wire.Observe`, `Wire.Release`, `Wire.Event`,
/// `Wire.Hello`, `Wire.Log`, `Wire.TimerFired` and `Wire.Snapshot`, plus the small types that
/// only they use (`Wire.ChangeEntry`, `Wire.PortStatus`, `Wire.StreamFlag`, `Wire.SnapshotStore`,
/// `Wire.SnapshotSignal`).
///
/// The payload types live in a namespace because generated code declares top-level types with
/// the names of the standard ports (`Log`, `Http`, `Kv`, ...) and of user records; a payload
/// called `Log` at the top level of `KeelRuntime` would be ambiguous next to a generated
/// `Log` port protocol. The three enums that generated code names directly (`CallTarget`,
/// `ReplyStatus`, `ChangeOp`) and the `KeelPayload` protocol stay at the top level.
public enum Wire {}
