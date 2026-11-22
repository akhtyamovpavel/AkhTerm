package org.akhterm.mobile.data.storage

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.akhterm.mobile.data.model.ThemeMode

class AppPreferences(
    context: Context,
) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    private val _themeMode = MutableStateFlow(readThemeMode())
    val themeMode: StateFlow<ThemeMode> = _themeMode.asStateFlow()

    private val _backgroundMonitorEnabled = MutableStateFlow(
        prefs.getBoolean(KEY_BG_MONITOR, true),
    )
    val backgroundMonitorEnabled: StateFlow<Boolean> = _backgroundMonitorEnabled.asStateFlow()

    fun setThemeMode(mode: ThemeMode) {
        prefs.edit().putString(KEY_THEME_MODE, mode.name).apply()
        _themeMode.value = mode
    }

    fun setBackgroundMonitorEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(KEY_BG_MONITOR, enabled).apply()
        _backgroundMonitorEnabled.value = enabled
    }

    private fun readThemeMode(): ThemeMode {
        val raw = prefs.getString(KEY_THEME_MODE, ThemeMode.SYSTEM.name)
        return ThemeMode.entries.firstOrNull { it.name == raw } ?: ThemeMode.SYSTEM
    }

    companion object {
        private const val PREFS_NAME = "akhterm_app_prefs"
        private const val KEY_THEME_MODE = "theme_mode"
        private const val KEY_BG_MONITOR = "background_monitor_enabled"
    }
}
