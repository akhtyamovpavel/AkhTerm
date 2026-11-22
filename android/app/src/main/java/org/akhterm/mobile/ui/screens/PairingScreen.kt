package org.akhterm.mobile.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CameraAlt
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Terminal
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.launch
import org.akhterm.mobile.R
import org.akhterm.mobile.data.network.ApiException
import org.akhterm.mobile.data.pairing.PairingPayload
import org.akhterm.mobile.data.pairing.QrPairingParser
import org.akhterm.mobile.data.repository.RemoteRepository
import org.akhterm.mobile.ui.theme.LocalAkhTermColors

@Composable
fun PairingScreen(
    repository: RemoteRepository,
    pendingDeepLinkPayload: PairingPayload?,
    onDeepLinkConsumed: () -> Unit,
    onPaired: () -> Unit,
) {
    val context = LocalContext.current
    val colors = LocalAkhTermColors.current
    val scope = rememberCoroutineScope()

    var selectedTab by rememberSaveable { mutableIntStateOf(0) } // 0 = QR, 1 = Manual
    var serverUrl by rememberSaveable { mutableStateOf("") }
    var pairingCode by rememberSaveable { mutableStateOf("") }
    var deviceName by rememberSaveable {
        mutableStateOf(
            listOfNotNull(Build.MANUFACTURER, Build.MODEL)
                .joinToString(" ")
                .trim()
                .ifBlank { "Android" },
        )
    }

    var isTesting by remember { mutableStateOf(false) }
    var isPairing by remember { mutableStateOf(false) }
    var feedbackSuccess by remember { mutableStateOf<String?>(null) }
    var feedbackError by remember { mutableStateOf<String?>(null) }

    var hasCameraPermission by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
    val cameraPermissionLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission(),
    ) { granted ->
        hasCameraPermission = granted
    }

    fun executePair(targetUrl: String, targetCode: String) {
        val normalized = QrPairingParser.normalizeBaseUrl(targetUrl)
        if (normalized == null) {
            feedbackError = context.getString(R.string.pairing_error_invalid_url)
            feedbackSuccess = null
            return
        }
        val cleanCode = targetCode.trim()
        if (cleanCode.isEmpty()) {
            feedbackError = context.getString(R.string.pairing_error_empty_code)
            feedbackSuccess = null
            return
        }

        scope.launch {
            isPairing = true
            feedbackError = null
            feedbackSuccess = null
            val res = repository.pair(
                baseUrl = normalized,
                code = cleanCode,
                deviceName = deviceName.trim().ifBlank { "Android" },
            )
            isPairing = false
            res.onSuccess {
                onPaired()
            }.onFailure { err ->
                feedbackError = when {
                    err is ApiException && err.httpCode == 429 ->
                        context.getString(R.string.pairing_error_rate_limited)

                    err is ApiException && (err.httpCode == 401 || err.httpCode == 400) ->
                        context.getString(R.string.pairing_error_invalid_code)

                    else -> err.message ?: "Pairing failed"
                }
            }
        }
    }

    fun handleQrRaw(raw: String) {
        val parsed = QrPairingParser.parse(raw)
        if (parsed == null) {
            feedbackError = context.getString(R.string.pairing_qr_invalid)
            feedbackSuccess = null
            return
        }
        serverUrl = parsed.baseUrl
        pairingCode = parsed.code
        feedbackError = null
        feedbackSuccess = context.getString(
            R.string.pairing_qr_detected,
            parsed.hostName ?: parsed.baseUrl,
        )
        executePair(parsed.baseUrl, parsed.code)
    }

    LaunchedEffect(pendingDeepLinkPayload) {
        if (pendingDeepLinkPayload != null) {
            serverUrl = pendingDeepLinkPayload.baseUrl
            pairingCode = pendingDeepLinkPayload.code
            selectedTab = 1
            onDeepLinkConsumed()
            executePair(pendingDeepLinkPayload.baseUrl, pendingDeepLinkPayload.code)
        }
    }

    Scaffold(
        containerColor = colors.background,
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Surface(
                    shape = RoundedCornerShape(12.dp),
                    color = colors.surface,
                    border = BorderStroke(1.dp, colors.border),
                ) {
                    Box(
                        modifier = Modifier.padding(10.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.Terminal,
                            contentDescription = null,
                            tint = colors.accent,
                            modifier = Modifier.size(24.dp),
                        )
                    }
                }
                Column {
                    Text(
                        text = stringResource(R.string.pairing_title),
                        style = MaterialTheme.typography.headlineSmall,
                        color = colors.textPrimary,
                    )
                    Text(
                        text = stringResource(R.string.pairing_subtitle),
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.textSecondary,
                    )
                }
            }

            SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
                SegmentedButton(
                    selected = selectedTab == 0,
                    onClick = { selectedTab = 0 },
                    shape = SegmentedButtonDefaults.itemShape(index = 0, count = 2),
                    icon = { Icon(Icons.Rounded.QrCodeScanner, contentDescription = null) },
                ) {
                    Text(stringResource(R.string.pairing_tab_qr))
                }
                SegmentedButton(
                    selected = selectedTab == 1,
                    onClick = { selectedTab = 1 },
                    shape = SegmentedButtonDefaults.itemShape(index = 1, count = 2),
                ) {
                    Text(stringResource(R.string.pairing_tab_manual))
                }
            }

            if (selectedTab == 0) {
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(300.dp),
                    shape = RoundedCornerShape(16.dp),
                    color = colors.surface,
                    border = BorderStroke(1.dp, colors.border),
                ) {
                    if (hasCameraPermission) {
                        Box(modifier = Modifier.fillMaxSize()) {
                            QrScannerPreview(
                                enabled = !isPairing,
                                onQrDetected = ::handleQrRaw,
                                modifier = Modifier
                                    .fillMaxSize()
                                    .clip(RoundedCornerShape(16.dp)),
                            )
                            Surface(
                                modifier = Modifier
                                    .align(Alignment.BottomCenter)
                                    .padding(12.dp),
                                shape = RoundedCornerShape(999.dp),
                                color = colors.background.copy(alpha = 0.85f),
                            ) {
                                Text(
                                    text = stringResource(R.string.pairing_qr_hint),
                                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                                    style = MaterialTheme.typography.labelMedium,
                                    color = colors.textPrimary,
                                )
                            }
                        }
                    } else {
                        Column(
                            modifier = Modifier
                                .fillMaxSize()
                                .padding(20.dp),
                            verticalArrangement = Arrangement.Center,
                            horizontalAlignment = Alignment.CenterHorizontally,
                        ) {
                            Icon(
                                imageVector = Icons.Rounded.CameraAlt,
                                contentDescription = null,
                                tint = colors.textSecondary,
                                modifier = Modifier.size(36.dp),
                            )
                            Spacer(Modifier.height(10.dp))
                            Text(
                                text = stringResource(R.string.pairing_camera_title),
                                style = MaterialTheme.typography.titleMedium,
                                color = colors.textPrimary,
                            )
                            Spacer(Modifier.height(6.dp))
                            Text(
                                text = stringResource(R.string.pairing_camera_rationale),
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.textSecondary,
                            )
                            Spacer(Modifier.height(14.dp))
                            Button(
                                onClick = {
                                    cameraPermissionLauncher.launch(Manifest.permission.CAMERA)
                                },
                            ) {
                                Text(stringResource(R.string.pairing_camera_grant))
                            }
                        }
                    }
                }
            }

            // Manual entry & connection test card (always accessible or primary on tab 1)
            Surface(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(16.dp),
                color = colors.surface,
                border = BorderStroke(1.dp, colors.border),
            ) {
                Column(
                    modifier = Modifier.padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    OutlinedTextField(
                        value = serverUrl,
                        onValueChange = { serverUrl = it },
                        label = { Text(stringResource(R.string.pairing_url_label)) },
                        placeholder = { Text(stringResource(R.string.pairing_url_placeholder)) },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                        textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                        modifier = Modifier.fillMaxWidth(),
                    )

                    OutlinedTextField(
                        value = pairingCode,
                        onValueChange = { pairingCode = it },
                        label = { Text(stringResource(R.string.pairing_code_label)) },
                        placeholder = { Text(stringResource(R.string.pairing_code_placeholder)) },
                        singleLine = true,
                        textStyle = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
                        modifier = Modifier.fillMaxWidth(),
                    )

                    OutlinedTextField(
                        value = deviceName,
                        onValueChange = { deviceName = it },
                        label = { Text(stringResource(R.string.pairing_device_name_label)) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )

                    feedbackSuccess?.let { msg ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(10.dp))
                                .background(colors.done.copy(alpha = 0.12f))
                                .padding(10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Icon(
                                imageVector = Icons.Rounded.CheckCircle,
                                contentDescription = null,
                                tint = colors.done,
                                modifier = Modifier.size(18.dp),
                            )
                            Text(
                                text = msg,
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.done,
                            )
                        }
                    }

                    feedbackError?.let { msg ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(10.dp))
                                .background(colors.error.copy(alpha = 0.12f))
                                .padding(10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Icon(
                                imageVector = Icons.Rounded.ErrorOutline,
                                contentDescription = null,
                                tint = colors.error,
                                modifier = Modifier.size(18.dp),
                            )
                            Text(
                                text = msg,
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.error,
                            )
                        }
                    }

                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        OutlinedButton(
                            onClick = {
                                val normalized = QrPairingParser.normalizeBaseUrl(serverUrl)
                                if (normalized == null) {
                                    feedbackError = context.getString(R.string.pairing_error_invalid_url)
                                    feedbackSuccess = null
                                    return@OutlinedButton
                                }
                                scope.launch {
                                    isTesting = true
                                    feedbackError = null
                                    feedbackSuccess = null
                                    val res = repository.testHealth(normalized)
                                    isTesting = false
                                    res.onSuccess { health ->
                                        feedbackSuccess = context.getString(
                                            R.string.pairing_health_ok,
                                            health.version,
                                            health.apiVersion,
                                        )
                                    }.onFailure { err ->
                                        feedbackError = err.message ?: "Connection failed"
                                    }
                                }
                            },
                            enabled = !isTesting && !isPairing,
                            modifier = Modifier.weight(1f),
                        ) {
                            if (isTesting) {
                                CircularProgressIndicator(
                                    modifier = Modifier.size(16.dp),
                                    strokeWidth = 2.dp,
                                )
                            } else {
                                Text(stringResource(R.string.pairing_test_button))
                            }
                        }

                        Button(
                            onClick = { executePair(serverUrl, pairingCode) },
                            enabled = !isPairing && !isTesting,
                            colors = ButtonDefaults.buttonColors(containerColor = colors.working),
                            modifier = Modifier.weight(1f),
                        ) {
                            if (isPairing) {
                                CircularProgressIndicator(
                                    modifier = Modifier.size(16.dp),
                                    strokeWidth = 2.dp,
                                    color = MaterialTheme.colorScheme.onPrimary,
                                )
                            } else {
                                Text(stringResource(R.string.pairing_pair_button))
                            }
                        }
                    }

                    Text(
                        text = stringResource(R.string.pairing_cleartext_hint),
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.textMuted,
                    )
                }
            }
        }
    }
}
