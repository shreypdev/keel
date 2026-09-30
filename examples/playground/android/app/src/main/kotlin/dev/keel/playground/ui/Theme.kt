package dev.keel.playground.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val Light = lightColorScheme(
    primary = Color(0xFF0B6E75),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFBDEFF2),
    onPrimaryContainer = Color(0xFF00363A),
    secondaryContainer = Color(0xFFD3E8EA),
    onSecondaryContainer = Color(0xFF0C1F21),
    surface = Color(0xFFF8FAFA),
    background = Color(0xFFF8FAFA),
)

private val Dark = darkColorScheme(
    primary = Color(0xFF6FD6DD),
    onPrimary = Color(0xFF00363A),
    primaryContainer = Color(0xFF00505A),
    onPrimaryContainer = Color(0xFFBDEFF2),
    secondaryContainer = Color(0xFF2A3F42),
    onSecondaryContainer = Color(0xFFD3E8EA),
    surface = Color(0xFF101415),
    background = Color(0xFF101415),
)

/** Material 3 with one teal accent, in the system's light or dark mode. */
@Composable
fun PlaygroundTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) Dark else Light, content = content)
}
