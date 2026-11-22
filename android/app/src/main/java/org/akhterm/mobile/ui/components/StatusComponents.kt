package org.akhterm.mobile.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.akhterm.mobile.R
import org.akhterm.mobile.data.model.ConnectionState
import org.akhterm.mobile.data.model.PaneState
import org.akhterm.mobile.ui.theme.LocalAkhTermColors

@Composable
fun ConnectionStatusPill(
    state: ConnectionState,
    hostName: String?,
    modifier: Modifier = Modifier,
) {
    val colors = LocalAkhTermColors.current
    val (dotColor, labelRes) = when (state) {
        ConnectionState.CONNECTED -> colors.done to R.string.status_connected
        ConnectionState.CONNECTING -> colors.working to R.string.status_connecting
        ConnectionState.RECONNECTING -> colors.waiting to R.string.status_reconnecting
        ConnectionState.UNAUTHORIZED -> colors.error to R.string.status_unauthorized
        ConnectionState.ERROR -> colors.error to R.string.status_error
        ConnectionState.DISCONNECTED -> colors.textMuted to R.string.status_disconnected
    }

    val label = if (state == ConnectionState.CONNECTED && !hostName.isNullOrBlank()) {
        hostName
    } else {
        stringResource(labelRes)
    }

    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(999.dp),
        color = colors.surfaceElevated,
        border = BorderStroke(1.dp, colors.border),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(dotColor),
            )
            Text(
                text = label,
                style = MaterialTheme.typography.labelMedium,
                color = colors.textPrimary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
fun AgentBadge(
    agent: String,
    modifier: Modifier = Modifier,
) {
    val colors = LocalAkhTermColors.current
    val tint = when (agent.lowercase()) {
        "claude code" -> Color(0xFFD97706)
        "codex" -> Color(0xFF10B981)
        "opencode" -> Color(0xFF3B82F6)
        "agy" -> Color(0xFF8B5CF6)
        "deepseek" -> Color(0xFF06B6D4)
        "kimi" -> Color(0xFFEC4899)
        else -> colors.textSecondary
    }
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(6.dp),
        color = tint.copy(alpha = 0.14f),
        border = BorderStroke(1.dp, tint.copy(alpha = 0.35f)),
    ) {
        Text(
            text = agent,
            modifier = Modifier.padding(horizontal = 7.dp, vertical = 2.dp),
            style = MaterialTheme.typography.labelSmall,
            fontWeight = FontWeight.SemiBold,
            color = tint,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
fun PaneStateBadge(
    state: PaneState,
    modifier: Modifier = Modifier,
) {
    val colors = LocalAkhTermColors.current
    val (color, labelRes) = when (state) {
        PaneState.WAITING -> colors.waiting to R.string.state_waiting
        PaneState.WORKING -> colors.working to R.string.state_working
        PaneState.DONE -> colors.done to R.string.state_done
        PaneState.ERROR -> colors.error to R.string.state_error
        PaneState.IDLE -> colors.textMuted to R.string.state_idle
    }
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(6.dp),
        color = color.copy(alpha = 0.14f),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 7.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(5.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(6.dp)
                    .clip(CircleShape)
                    .background(color),
            )
            Text(
                text = stringResource(labelRes),
                style = MaterialTheme.typography.labelSmall,
                fontWeight = FontWeight.SemiBold,
                color = color,
            )
        }
    }
}

@Composable
fun MetaPill(
    text: String,
    monospace: Boolean = false,
    accentColor: Color? = null,
    modifier: Modifier = Modifier,
) {
    val colors = LocalAkhTermColors.current
    val fg = accentColor ?: colors.textSecondary
    val bg = accentColor?.copy(alpha = 0.12f) ?: colors.surfaceElevated
    val border = accentColor?.copy(alpha = 0.30f) ?: colors.border
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(6.dp),
        color = bg,
        border = BorderStroke(1.dp, border),
    ) {
        Text(
            text = text,
            modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp),
            style = MaterialTheme.typography.labelSmall.copy(
                fontFamily = if (monospace) FontFamily.Monospace else FontFamily.SansSerif,
            ),
            color = fg,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

fun formatElapsed(seconds: Long?): String? {
    if (seconds == null || seconds < 0) return null
    return when {
        seconds < 60 -> "${seconds}s"
        seconds < 3600 -> {
            val mins = seconds / 60
            val secs = seconds % 60
            "${mins}m ${secs.toString().padStart(2, '0')}s"
        }

        else -> {
            val hours = seconds / 3600
            val mins = (seconds % 3600) / 60
            "${hours}h ${mins.toString().padStart(2, '0')}m"
        }
    }
}
