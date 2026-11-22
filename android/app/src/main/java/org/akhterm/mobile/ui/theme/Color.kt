package org.akhterm.mobile.ui.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

// Dark Zinc palette matching desktop AkhTerm
val ZincDarkBg = Color(0xFF09090B)
val ZincDarkSurface = Color(0xFF18181B)
val ZincDarkSurfaceElevated = Color(0xFF1F1F23)
val ZincDarkTerminalBg = Color(0xFF0C0C0F)
val ZincDarkBorder = Color(0xFF27272A)
val ZincDarkTextPrimary = Color(0xFFF4F4F5)
val ZincDarkTextSecondary = Color(0xFFA1A1AA)
val ZincDarkTextMuted = Color(0xFF71717A)

// Light Zinc palette matching desktop AkhTerm
val ZincLightBg = Color(0xFFFAFAFA)
val ZincLightSurface = Color(0xFFFFFFFF)
val ZincLightSurfaceElevated = Color(0xFFF4F4F5)
val ZincLightTerminalBg = Color(0xFF111114)
val ZincLightBorder = Color(0xFFE4E4E7)
val ZincLightTextPrimary = Color(0xFF09090B)
val ZincLightTextSecondary = Color(0xFF52525B)
val ZincLightTextMuted = Color(0xFF71717A)

// Shared semantic accents
val AmberWaiting = Color(0xFFF59E0B)
val BlueWorking = Color(0xFF3B82F6)
val GreenDone = Color(0xFF22C55E)
val RedError = Color(0xFFEF4444)
val VioletAccent = Color(0xFF8B5CF6)

@Immutable
data class AkhTermColors(
    val background: Color,
    val surface: Color,
    val surfaceElevated: Color,
    val terminalBackground: Color,
    val terminalText: Color,
    val border: Color,
    val textPrimary: Color,
    val textSecondary: Color,
    val textMuted: Color,
    val waiting: Color,
    val working: Color,
    val done: Color,
    val error: Color,
    val accent: Color,
    val isDark: Boolean,
)

val DarkAkhTermColors = AkhTermColors(
    background = ZincDarkBg,
    surface = ZincDarkSurface,
    surfaceElevated = ZincDarkSurfaceElevated,
    terminalBackground = ZincDarkTerminalBg,
    terminalText = Color(0xFFE4E4E7),
    border = ZincDarkBorder,
    textPrimary = ZincDarkTextPrimary,
    textSecondary = ZincDarkTextSecondary,
    textMuted = ZincDarkTextMuted,
    waiting = AmberWaiting,
    working = BlueWorking,
    done = GreenDone,
    error = RedError,
    accent = VioletAccent,
    isDark = true,
)

val LightAkhTermColors = AkhTermColors(
    background = ZincLightBg,
    surface = ZincLightSurface,
    surfaceElevated = ZincLightSurfaceElevated,
    terminalBackground = ZincLightTerminalBg,
    terminalText = Color(0xFFF4F4F5),
    border = ZincLightBorder,
    textPrimary = ZincLightTextPrimary,
    textSecondary = ZincLightTextSecondary,
    textMuted = ZincLightTextMuted,
    waiting = Color(0xFFD97706),
    working = Color(0xFF2563EB),
    done = Color(0xFF16A34A),
    error = Color(0xFFDC2626),
    accent = Color(0xFF7C3AED),
    isDark = false,
)
