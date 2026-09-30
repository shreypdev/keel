// Stand-in for JUnit 5's @Test so the test sources compile unchanged under scripts/test-local.sh,
// where no JUnit jars are available. It is NOT part of the Gradle build (Gradle uses the real
// org.junit.jupiter.api.Test); it exists only on the local compile path.
package org.junit.jupiter.api

@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Test
