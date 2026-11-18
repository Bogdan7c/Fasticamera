package dev.fasticamera.probe.camera.threea

import android.graphics.ImageFormat
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.ImageReader
import android.os.Handler
import android.util.Size
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.VendorKeyValue

/** Параметры сессии 3A-разведки: ровно тот тип сессии, который будет у продукта. */
data class ThreeASessionConfig(
    val cameraId: String,
    val size: Size,
    val format: Int,
    val readerUsage: Long,
    val maxImages: Int,
    /** Вендорские ключи 4:3@60: кладутся в КАЖДЫЙ запрос (они покадровые). */
    val perRequestVendorKeys: List<VendorKeyValue>,
    val openTimeoutMs: Long,
    /** Диагностика: добавить второй выход YUV_420_888 этого размера (проверка, нужен ли 3A «обычный» поток). null = RAW-only. */
    val companionYuvSize: Size? = null,
    /** Формат вспомогательного потока (YUV_420_888 или PRIVATE). */
    val companionFormat: Int = ImageFormat.YUV_420_888,
    /** false = поток сконфигурирован в сессии, но ни один запрос его не использует как цель. */
    val companionTargeted: Boolean = true,
    /** Публичный AE target fps [N,N] (сессионный параметр и в каждом запросе); null — не задавать. */
    val aeTargetFps: Int? = null,
)

/**
 * Жизненный цикл одной RAW-only сессии и фабрика запросов.
 * Владеет устройством, сессией и ридером; [close] обязателен (вызывать в finally).
 */
class ThreeASession private constructor(
    private val config: ThreeASessionConfig,
    private val device: CameraDevice,
    private val reader: ImageReader,
    private val companionReader: ImageReader?,
    private val session: CameraCaptureSession,
    private val recorder: ThreeARecorder,
    private val callbackHandler: Handler,
) {

    /** Любое исключение Camera2 пробрасывается: сценарий пишет его в JSONL как ошибку фазы. */
    fun newRequest(tag: ThreeATag, configure: (CaptureRequest.Builder) -> Unit = {}): CaptureRequest {
        val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
        builder.addTarget(reader.surface)
        if (config.companionTargeted) companionReader?.let { builder.addTarget(it.surface) }
        // Автоматика по умолчанию = baseline; фазы переопределяют нужное в configure.
        builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON)
        builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
        builder.set(CaptureRequest.CONTROL_AWB_MODE, CaptureRequest.CONTROL_AWB_MODE_AUTO)
        builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CaptureRequest.CONTROL_AF_TRIGGER_IDLE)
        config.aeTargetFps?.let { builder.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, android.util.Range(it, it)) }
        config.perRequestVendorKeys.forEach { it.applyTo(builder) }
        configure(builder)
        builder.setTag(tag)
        return builder.build()
    }

    fun setRepeating(request: CaptureRequest) {
        session.setRepeatingRequest(request, recorder, callbackHandler)
    }

    fun captureOnce(request: CaptureRequest) {
        session.capture(request, recorder, callbackHandler)
    }

    fun captureBurst(requests: List<CaptureRequest>) {
        session.captureBurst(requests, recorder, callbackHandler)
    }

    /** Остановка и закрытие всего; ошибки остановки возвращаются текстом, а не глотаются. */
    fun close(): String? {
        var stopError: String? = null
        try {
            session.stopRepeating()
            session.abortCaptures()
        } catch (error: Exception) {
            stopError = "${error.javaClass.name}: ${error.message}"
        } finally {
            session.close()
            device.close()
            reader.close()
            companionReader?.close()
        }
        return stopError
    }

    sealed interface OpenResult {
        data class Ready(val session: ThreeASession, val device: CameraDevice) : OpenResult
        data class Failed(val stage: String, val details: String) : OpenResult
    }

    companion object {
        private const val COMPANION_MAX_IMAGES = 4

        /** Сессионные параметры: AE fps range — сессионный ключ, по нему HAL выбирает режим сенсора. */
        private fun sessionParameters(device: CameraDevice, config: ThreeASessionConfig): CaptureRequest? {
            val fps = config.aeTargetFps ?: return null
            val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
            builder.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, android.util.Range(fps, fps))
            return builder.build()
        }

        fun open(
            runner: CameraSessionRunner,
            config: ThreeASessionConfig,
            recorder: ThreeARecorder,
            callbackHandler: Handler,
        ): OpenResult {
            val opened = runner.open(config.cameraId, config.openTimeoutMs)
            if (opened !is CameraSessionRunner.OpenOutcome.Opened) return OpenResult.Failed("open", opened.toJson().toString())
            val device = opened.device
            val reader = ImageReader.newInstance(config.size.width, config.size.height, config.format, config.maxImages, config.readerUsage)
            reader.setOnImageAvailableListener(recorder.imageListener, callbackHandler)
            val companion = config.companionYuvSize?.let { size ->
                val companionReader = if (config.companionFormat == ImageFormat.PRIVATE) {
                    ImageReader.newInstance(size.width, size.height, config.companionFormat, COMPANION_MAX_IMAGES, config.readerUsage)
                } else {
                    ImageReader.newInstance(size.width, size.height, config.companionFormat, COMPANION_MAX_IMAGES)
                }
                // Картинки вспомогательного потока нужны только HAL: считаем метки и закрываем сразу.
                companionReader.setOnImageAvailableListener(recorder.companionImageListener, callbackHandler)
                companionReader
            }
            val outputs = listOfNotNull(reader.surface, companion?.surface).map(::OutputConfiguration)
            val outcome = runner.createSession(device, outputs, SessionConfiguration.SESSION_REGULAR, sessionParameters(device, config), config.openTimeoutMs)
            if (outcome !is CameraSessionRunner.SessionOutcome.Configured) {
                device.close()
                reader.close()
                companion?.close()
                return OpenResult.Failed("session", outcome.toJson().toString())
            }
            val session = ThreeASession(config, device, reader, companion, outcome.session, recorder, callbackHandler)
            return OpenResult.Ready(session, device)
        }
    }
}
