package dev.keel.playground.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

/**
 * The top of a screen: a small [overline] naming it and a large [headline], which carries the number the
 * screen is about ("2 left", "10,000 rows") and the test tag [headlineTag].
 */
@Composable
fun ScreenHeader(overline: String, headline: String, headlineTag: String, modifier: Modifier = Modifier) {
    Column(modifier.padding(top = 20.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            overline.uppercase(),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            fontWeight = FontWeight.SemiBold,
        )
        Text(headline, style = MaterialTheme.typography.headlineMedium, modifier = Modifier.testTag(headlineTag))
    }
}
