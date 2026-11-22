package org.akhterm.mobile.ui.theme

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import org.akhterm.mobile.data.model.ThemeMode

val LocalAkhTermColors = staticCompositionLocalOf { DarkAkhTermColors }

private val DarkColorScheme = darkColorScheme(
    primary = BlueWorking,
    onPrimary = Color.White,
    secondary = VioletAccent,
    onSecondary = Color.White,
    tertiary = AmberWaiting,
    background = ZincDarkBg,
    onBackground = ZincDarkTextPrimary,
    surface = ZincDarkSurface,
    onSurface = ZincDarkTextPrimary,
    surfaceVariant = ZincDarkSurfaceElevated,
    onSurfaceVariant = ZincDarkTextSecondary,
    outline = ZincDarkBorder,
    error = RedError,
    onError = Color.White,
)

private val LightColorScheme = lightColorScheme(
    primary = Color(0xFF2563EB),
    onPrimary = Color.White,
    secondary = Color(0xFF7C3AED),
    onSecondary = Color.White,
    tertiary = Color(0xFFD97706),
    background = ZincLightBg,
    onBackground = ZincLightTextPrimary,
    surface = ZincLightSurface,
    onSurface = ZincLightTextPrimary,
    surfaceVariant = ZincLightSurfaceElevated,
    onSurfaceVariant = ZincLightTextSecondary,
    outline = ZincLightBorder,
    error = Color(0xFFDC2626),
    onError = Color.White,
)

@Composable
fun AkhTermTheme(
    themeMode: ThemeMode,
    content: @Composable () -> Unit,
) {
    val darkTheme = when (themeMode) {
        ThemeMode.SYSTEM -> isSystemInDarkTheme()
        ThemeMode.DARK -> true
        ThemeMode.LIGHT -> false
    }
    val customColors = if (darkTheme) DarkAkhTermColors else LightAkhTermColors
    val materialColors = if (darkTheme) DarkColorScheme else LightColorScheme

    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as? Activity)?.window ?: return@SideEffect
            val insetsController = WindowCompat.getInsetsController(window, view)
            insetsController.isAppearanceLightStatusBars = !darkTheme
            insetsController.isAppearanceLightNavigationBars = !darkTheme
        }
    }

    CompositionLocalProvider(LocalAkhTermColors provides customColors) {
        MaterialTheme(
            colorScheme = materialColors,
            typography = AkhTermTypography,
            content = content,
        )
    }
}
