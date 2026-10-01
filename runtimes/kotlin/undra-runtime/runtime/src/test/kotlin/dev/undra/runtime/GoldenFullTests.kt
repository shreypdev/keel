package dev.undra.runtime

import dev.undra.runtime.testing.Suite
import org.junit.jupiter.api.Test

/**
 * The bindgen `full` golden case (`crates/undra-bindgen/tests/golden/full/kotlin`) compiled against this
 * real runtime, and the execution test bindgen ships for it (`tests/fixtures/kotlin-run/full/FullTest.kt`,
 * which drives the generated objects, stores, ports and queries through a fake `UndraCore` subclass, a
 * fake `Mirror` and the real wire layer).
 *
 * Compiling is the point: generated code depends on exactly the runtime surface of SPEC 17.2 plus the
 * additions in bindgen's `UndraBase.kt` fixture, and this is where a runtime change that breaks it shows.
 * `scripts/test-local.sh` and the Gradle build add both source trees to the test compilation.
 */
class GoldenFullTests : Suite() {
    init {
        case("the generated full golden output compiles against the real runtime and passes its execution test") {
            val main = try {
                Class.forName("golden.full.FullTestKt").getMethod("main")
            } catch (e: ClassNotFoundException) {
                skip("golden.full.FullTestKt is not on the classpath (bindgen sources not found)")
            }
            main.invoke(null)
        }
    }

    @Test
    fun allCases() = assertPassed()
}
