/// The spelling generated stores use for "every signal of the store":
/// `core.observe(handle, signal: Observe.allSignals, on: true)`.
///
/// The `Observe` payload itself lives in the wire namespace (`Wire.Observe`); this enum only
/// carries the constant so that generated code keeps compiling.
public enum Observe {
    /// The `signal_id` that means "all signals of the store" (docs/SPEC.md section 1.1).
    public static let allSignals: UInt32 = Wire.Observe.allSignals
}
