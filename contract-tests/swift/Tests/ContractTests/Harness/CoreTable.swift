import UndraFFI
import PlaygroundCoreFFI

/// The playground core's C ABI table (ADR-044: the core's one export, `playground_core_undra_api`),
/// for the checks that reach below `UndraCore`: `init` as another embedder would call it (S16), the
/// exported schema (S16) and `stats_json` while no core is running (S17).
func playgroundTable() -> UndraApi {
    let api = UnsafeRawPointer(playground_core_undra_api())
    return api!.assumingMemoryBound(to: UndraApi.self).pointee
}
