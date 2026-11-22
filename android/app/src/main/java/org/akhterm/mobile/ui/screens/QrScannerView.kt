package org.akhterm.mobile.ui.screens

import android.util.Size
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size as ComposeSize
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong

@Composable
fun QrScannerPreview(
    enabled: Boolean,
    onQrDetected: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val latestCallback = rememberUpdatedState(onQrDetected)
    val latestEnabled = rememberUpdatedState(enabled)

    val analysisExecutor = remember { Executors.newSingleThreadExecutor() }
    val previewView = remember {
        PreviewView(context).apply {
            scaleType = PreviewView.ScaleType.FILL_CENTER
        }
    }

    DisposableEffect(lifecycleOwner, previewView) {
        val providerFuture = ProcessCameraProvider.getInstance(context)
        val analyzer = QrFrameAnalyzer(
            isEnabled = { latestEnabled.value },
            onCode = { raw ->
                ContextCompat.getMainExecutor(context).execute {
                    latestCallback.value(raw)
                }
            },
        )
        var boundProvider: ProcessCameraProvider? = null

        providerFuture.addListener(
            {
                runCatching {
                    val provider = providerFuture.get()
                    boundProvider = provider
                    val preview = Preview.Builder().build().also {
                        it.surfaceProvider = previewView.surfaceProvider
                    }
                    @Suppress("DEPRECATION")
                    val imageAnalysis = ImageAnalysis.Builder()
                        .setTargetResolution(Size(1280, 720))
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build()
                        .also { it.setAnalyzer(analysisExecutor, analyzer) }

                    provider.unbindAll()
                    provider.bindToLifecycle(
                        lifecycleOwner,
                        CameraSelector.DEFAULT_BACK_CAMERA,
                        preview,
                        imageAnalysis,
                    )
                }
            },
            ContextCompat.getMainExecutor(context),
        )

        onDispose {
            runCatching { boundProvider?.unbindAll() }
            analysisExecutor.shutdown()
        }
    }

    Box(modifier = modifier) {
        AndroidView(
            factory = { previewView },
            modifier = Modifier.fillMaxSize(),
        )
        Canvas(modifier = Modifier.fillMaxSize()) {
            val side = minOf(size.width, size.height) * 0.62f
            val topLeft = Offset((size.width - side) / 2f, (size.height - side) / 2f)
            drawRoundRect(
                color = Color(0xFFF59E0B),
                topLeft = topLeft,
                size = ComposeSize(side, side),
                cornerRadius = CornerRadius(24f, 24f),
                style = Stroke(width = 6f),
            )
        }
    }
}

private class QrFrameAnalyzer(
    private val isEnabled: () -> Boolean,
    private val onCode: (String) -> Unit,
) : ImageAnalysis.Analyzer {
    private val reader = QRCodeReader()
    private val decodeHints = mapOf(
        DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
        DecodeHintType.CHARACTER_SET to "UTF-8",
    )
    private val lastEmitMillis = AtomicLong(0L)

    override fun analyze(image: ImageProxy) {
        try {
            if (!isEnabled()) return
            val yBytes = extractLumaPlane(image) ?: return
            val source = PlanarYUVLuminanceSource(
                yBytes,
                image.width,
                image.height,
                0,
                0,
                image.width,
                image.height,
                false,
            )
            val bitmap = BinaryBitmap(HybridBinarizer(source))
            val decoded = runCatching { reader.decode(bitmap, decodeHints) }.getOrNull()
            reader.reset()

            val text = decoded?.text?.trim().orEmpty()
            if (text.isNotEmpty()) {
                val now = System.currentTimeMillis()
                val prev = lastEmitMillis.get()
                if (now - prev >= 1500L && lastEmitMillis.compareAndSet(prev, now)) {
                    onCode(text)
                }
            }
        } finally {
            image.close()
        }
    }

    private fun extractLumaPlane(image: ImageProxy): ByteArray? {
        val plane = image.planes.firstOrNull() ?: return null
        val buffer = plane.buffer
        val width = image.width
        val height = image.height
        val rowStride = plane.rowStride
        val pixelStride = plane.pixelStride

        if (pixelStride == 1 && rowStride == width) {
            val out = ByteArray(buffer.remaining())
            buffer.get(out)
            return out
        }

        val out = ByteArray(width * height)
        val rowBuffer = ByteArray(rowStride)
        for (row in 0 until height) {
            val remaining = buffer.remaining()
            if (remaining <= 0) break
            val toRead = minOf(rowStride, remaining)
            buffer.get(rowBuffer, 0, toRead)
            val dstOffset = row * width
            if (pixelStride == 1) {
                System.arraycopy(rowBuffer, 0, out, dstOffset, minOf(width, toRead))
            } else {
                for (col in 0 until width) {
                    val srcIdx = col * pixelStride
                    if (srcIdx < toRead) {
                        out[dstOffset + col] = rowBuffer[srcIdx]
                    }
                }
            }
        }
        return out
    }
}
