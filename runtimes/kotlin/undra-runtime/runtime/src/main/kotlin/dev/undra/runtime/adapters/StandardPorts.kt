package dev.undra.runtime.adapters

/**
 * Port and method ids of the standard ports (`undra-ports`, SPEC section 8), so adapters can register
 * themselves without generated code: `port_id = fnv1a32("port.<Trait>")` and
 * `method_id = fnv1a32("<Trait>.<method>")` (SPEC 1.1). A test recomputes every constant with
 * `Fnv.fnv1a32`, so a typo cannot survive.
 */
public object StandardPorts {
    /** `Clock`: `now_ms() -> i64`, `monotonic_ns() -> u64` (sync). */
    public object Clock {
        /** `fnv1a32("port.Clock")`. */
        public const val PORT_ID: UInt = 0xcd99c48eu

        /** `fnv1a32("Clock.now_ms")`. */
        public const val NOW_MS: UInt = 0xccc94d90u

        /** `fnv1a32("Clock.monotonic_ns")`. */
        public const val MONOTONIC_NS: UInt = 0x2cb2b4bfu
    }

    /** `Rng`: `fill(len: u32) -> Bytes` (sync). */
    public object Rng {
        /** `fnv1a32("port.Rng")`. */
        public const val PORT_ID: UInt = 0x25135bf5u

        /** `fnv1a32("Rng.fill")`. */
        public const val FILL: UInt = 0x2832b8edu
    }

    /** `Log`: `log(level: u8, target: String, message: String)` (sync). */
    public object Log {
        /** `fnv1a32("port.Log")`. */
        public const val PORT_ID: UInt = 0x575ff24au

        /** `fnv1a32("Log.log")`. */
        public const val LOG: UInt = 0xd49d5649u
    }

    /** `Http`: `request(req: HttpRequest) -> Result<HttpResponse, HttpError>` (async). */
    public object Http {
        /** `fnv1a32("port.Http")`. */
        public const val PORT_ID: UInt = 0x1ebeb908u

        /** `fnv1a32("Http.request")`. */
        public const val REQUEST: UInt = 0x6b14df26u
    }

    /**
     * `Kv` (async; ADR-049): `get(key) -> Result<Option<Bytes>, StorageError>`, `set(key, value) -> Result<(), StorageError>`,
     * `delete(key) -> Result<(), StorageError>`, `list(prefix) -> Result<Vec<String>, StorageError>`. A failure is
     * answered with status 1 and the encoded [StorageError] ([StoragePort] does it for a [KeyValueBackend]).
     */
    public object Kv {
        /** `fnv1a32("port.Kv")`. */
        public const val PORT_ID: UInt = 0x5389110du

        /** `fnv1a32("Kv.get")`. */
        public const val GET: UInt = 0xf050bb1au

        /** `fnv1a32("Kv.set")`. */
        public const val SET: UInt = 0x62427856u

        /** `fnv1a32("Kv.delete")`. */
        public const val DELETE: UInt = 0x60a386b9u

        /** `fnv1a32("Kv.list")`. */
        public const val LIST: UInt = 0x32f1d03au
    }

    /** `SecureStore`: the same four methods as [Kv], with the same [StorageError] channel, under its own ids (async). */
    public object SecureStore {
        /** `fnv1a32("port.SecureStore")`. */
        public const val PORT_ID: UInt = 0xc01f5beau

        /** `fnv1a32("SecureStore.get")`. */
        public const val GET: UInt = 0x57036f6fu

        /** `fnv1a32("SecureStore.set")`. */
        public const val SET: UInt = 0xe91e017bu

        /** `fnv1a32("SecureStore.delete")`. */
        public const val DELETE: UInt = 0xd57db4e2u

        /** `fnv1a32("SecureStore.list")`. */
        public const val LIST: UInt = 0xf5bab8c9u
    }

    /** `Fs`: `read`, `write`, `delete`, `list` (async), each failing with an [FsError]. */
    public object Fs {
        /** `fnv1a32("port.Fs")`. */
        public const val PORT_ID: UInt = 0x4ea34cabu

        /** `fnv1a32("Fs.read")`. */
        public const val READ: UInt = 0x01fdbe44u

        /** `fnv1a32("Fs.write")`. */
        public const val WRITE: UInt = 0x6b70d47fu

        /** `fnv1a32("Fs.delete")`. */
        public const val DELETE: UInt = 0xa90a826bu

        /** `fnv1a32("Fs.list")`. */
        public const val LIST: UInt = 0x4fba8678u
    }

    /** `Timer`: `set(timer_id: u32, delay_ms: u64)`, fire-and-forget; completion comes back as `TimerFired`. */
    public object Timer {
        /** `fnv1a32("port.Timer")`. */
        public const val PORT_ID: UInt = 0x00c2cdd9u

        /** `fnv1a32("Timer.set")`. */
        public const val SET: UInt = 0x923a766cu
    }

    /** `Connectivity` (event port): `changed(online: bool, kind: NetKind)`. */
    public object Connectivity {
        /** `fnv1a32("port.Connectivity")`. */
        public const val PORT_ID: UInt = 0x1feff6ffu

        /** `fnv1a32("Connectivity.changed")`. */
        public const val CHANGED: UInt = 0xb4f2a010u
    }

    /** `Lifecycle` (event port): `changed(state: AppState)`. */
    public object Lifecycle {
        /** `fnv1a32("port.Lifecycle")`. */
        public const val PORT_ID: UInt = 0x81c0afd4u

        /** `fnv1a32("Lifecycle.changed")`. */
        public const val CHANGED: UInt = 0x0bc82569u
    }

    /**
     * `Diagnostics` (sync, ADR-046): `panicked(report: PanicReport)`, called once per contained panic of the core, fire and forget
     * (port call id 0, like a `Log` record), after the core's FATAL log record. The runtime registers its own adapter for it, which
     * hands the report to `LoadOptions.onPanic`.
     */
    public object Diagnostics {
        /** `fnv1a32("port.Diagnostics")`. */
        public const val PORT_ID: UInt = 0xab68cd7cu

        /** `fnv1a32("Diagnostics.panicked")`. */
        public const val PANICKED: UInt = 0xbd147e2eu
    }
}

/**
 * Function ids of the standard functions (ADR-046): free functions that are in every schema, are never generated and are
 * implemented by the runtime's own API (`function_id = fnv1a32("fn.<name>")`).
 */
public object StandardFunctions {
    /**
     * `fnv1a32("fn.run_background")`: `run_background(deadline_ms: u64) -> BackgroundReport` (async), behind
     * `UndraCore.runInBackground`.
     */
    public const val RUN_BACKGROUND: UInt = 0x0e5b14ffu
}
