package dev.fasticamera.probe.camera

import android.graphics.ImageFormat
import android.hardware.camera2.CameraConstrainedHighSpeedCaptureSession
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.media.ImageReader
import android.os.Handler
import android.util.Range
import android.view.Surface
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * Куда идёт RAW-поток эксперимента.
 * - [JavaReader] — обычный ImageReader; к/с считаем по пришедшим картинкам.
 * - [ExternalSurface] — чужая поверхность (нативный AImageReader для C4); картинки считает владелец
 *   поверхности, а мы ждём [completion] — сигнал, что нативный тест закончил.
 */
sealed interface RawOutput {
    data object JavaReader : RawOutput
    data class ExternalSurface(val surface: Surface, val completion: Future<*>) : RawOutput
}

/**
 * Один эксперимент с потоком: открыть камеру → сессия → повторяющийся запрос → замер → закрыть всё.
 *
 * Ресурсы (устройство, сессия, ридеры) закрываются в finally при любом исходе:
 * зависшая камера испортила бы следующий запуск и сделала бы замеры несравнимыми.
 */
class StreamExperiment(
    private val runner: CameraSessionRunner,
    private val imageHandler: Handler,
    /** Вехи прогона (открыта, сессия, поток пошёл): по ним видно, где застрял HAL, если `done` не пришёл. */
    private val progress: (stage: String, details: JSONObject) -> Unit,
) {

    fun run(config: StreamExperimentConfig, rawOutput: RawOutput): JSONObject {
        val report = JSONObject().put("config", config.toJson())
        val openOutcome = runner.open(config.cameraId, config.openTimeoutMs)
        report.put("open", openOutcome.toJson())
        progress("open", openOutcome.toJson())
        if (openOutcome !is CameraSessionRunner.OpenOutcome.Opened) {
            return report.put("stage_reached", "open")
        }
        val device = openOutcome.device
        val contentProbe = config.contentEveryNth.takeIf { it > 0 }?.let { everyNth ->
            RawContentProbe(everyNth, config.contentBandCount, RawRegionStats(config.contentBlockSize))
        }
        val collector = FrameCollector(config.frameTarget, config.resultKeys, contentProbe)
        val readers = mutableListOf<ImageReader>()
        var session: CameraCaptureSession? = null
        try {
            val rawSurface = when (rawOutput) {
                RawOutput.JavaReader -> createRawReader(config, collector, report)?.also(readers::add)?.surface
                is RawOutput.ExternalSurface -> rawOutput.surface
            } ?: return report.put("stage_reached", "raw_reader")
            val companionSurface = config.companionYuvSize?.let { size ->
                createDrainingReader(size.width, size.height, ImageFormat.YUV_420_888).also(readers::add).surface
            }
            val targets = listOfNotNull(rawSurface, companionSurface)
            val outputs = targets.map { surface ->
                OutputConfiguration(surface).apply { config.physicalCameraId?.let(::setPhysicalCameraId) }
            }
            val requestErrors = JSONArray()
            report.put("request_key_errors", requestErrors)
            val sessionParameters = buildSessionParameters(device, config, requestErrors)
            val sessionOutcome = runner.createSession(device, outputs, config.sessionType, sessionParameters, config.openTimeoutMs)
            report.put("session", sessionOutcome.toJson())
            progress("session", sessionOutcome.toJson())
            if (sessionOutcome !is CameraSessionRunner.SessionOutcome.Configured) {
                return report.put("stage_reached", "session")
            }
            session = sessionOutcome.session
            val request = buildStreamRequest(device, config, targets, requestErrors)
            startRepeating(session, request, collector)
            report.put("stage_reached", "streaming")
            progress("streaming", JSONObject())
            val completed = awaitCompletion(config, rawOutput, collector)
            report.put("completed_before_timeout", completed)
            progress("collected", JSONObject().put("completed_before_timeout", completed))
        } catch (error: Exception) {
            // Любой сбой посреди потока — часть результата эксперимента, а не падение зонда.
            report.put("stream_exception", "${error.javaClass.name}: ${error.message}")
        } finally {
            stopQuietlyButReport(session, report)
            device.close()
            readers.forEach(ImageReader::close)
            progress("closed", JSONObject())
        }
        return report
            .put("stream", collector.toJson())
            .put("device_errors", JSONArray(runner.deviceErrors.toList()))
    }

    private fun createRawReader(config: StreamExperimentConfig, collector: FrameCollector, report: JSONObject): ImageReader? =
        try {
            val reader = if (config.readerUsage != null) {
                ImageReader.newInstance(config.size.width, config.size.height, config.format, config.maxImages, config.readerUsage)
            } else {
                ImageReader.newInstance(config.size.width, config.size.height, config.format, config.maxImages)
            }
            reader.setOnImageAvailableListener(collector.imageListener, imageHandler)
            reader
        } catch (error: IllegalArgumentException) {
            report.put("raw_reader_error", "${error.javaClass.name}: ${error.message}")
            null
        }

    /** Вспомогательный YUV-поток: картинки сразу закрываются, нужен только самому HAL. */
    private fun createDrainingReader(width: Int, height: Int, format: Int): ImageReader {
        val reader = ImageReader.newInstance(width, height, format, COMPANION_MAX_IMAGES)
        reader.setOnImageAvailableListener({ source -> source.acquireLatestImage()?.close() }, imageHandler)
        return reader
    }

    private fun buildSessionParameters(device: CameraDevice, config: StreamExperimentConfig, errors: JSONArray): CaptureRequest? {
        if (config.sessionKeys.isEmpty() && config.aeTargetFps == null) return null
        val builder = device.createCaptureRequest(config.template.cameraTemplate)
        // aeTargetFpsRange — тоже сессионный ключ (виден в availableSessionKeys), HAL выбирает по нему режим сенсора.
        config.aeTargetFps?.let { fps -> builder.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(fps, fps)) }
        applyVendorKeys(builder, config.sessionKeys, "session", errors)
        return builder.build()
    }

    private fun buildStreamRequest(
        device: CameraDevice,
        config: StreamExperimentConfig,
        targets: List<Surface>,
        errors: JSONArray,
    ): CaptureRequest {
        val builder = device.createCaptureRequest(config.template.cameraTemplate)
        targets.forEach(builder::addTarget)
        config.aeTargetFps?.let { fps -> builder.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(fps, fps)) }
        config.manualExposure?.let { manual ->
            builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_OFF)
            builder.set(CaptureRequest.SENSOR_EXPOSURE_TIME, manual.exposureTimeNs)
            builder.set(CaptureRequest.SENSOR_SENSITIVITY, manual.sensitivityIso)
            builder.set(CaptureRequest.SENSOR_FRAME_DURATION, manual.frameDurationNs)
        }
        // Сессионные ключи дублируем в каждый запрос: часть HAL читает их только из запросов.
        applyVendorKeys(builder, config.sessionKeys, "session_in_request", errors)
        applyVendorKeys(builder, config.requestKeys, "request", errors)
        return builder.build()
    }

    private fun applyVendorKeys(builder: CaptureRequest.Builder, keys: List<VendorKeyValue>, where: String, errors: JSONArray) {
        for (key in keys) {
            try {
                key.applyTo(builder)
            } catch (error: IllegalArgumentException) {
                // Неизвестный тег или чужой тип: фиксируем и продолжаем без этого ключа — это видно в отчёте.
                errors.put(JSONObject().put("where", where).put("key", key.describe()).put("error", error.message))
            }
        }
    }

    private fun startRepeating(session: CameraCaptureSession, request: CaptureRequest, collector: FrameCollector) {
        if (session is CameraConstrainedHighSpeedCaptureSession) {
            // High-speed сессия принимает только пачки, собранные самим фреймворком.
            val burst = session.createHighSpeedRequestList(request)
            session.setRepeatingBurst(burst, collector, imageHandler)
        } else {
            session.setRepeatingRequest(request, collector, imageHandler)
        }
    }

    private fun awaitCompletion(config: StreamExperimentConfig, rawOutput: RawOutput, collector: FrameCollector): Boolean =
        when (rawOutput) {
            RawOutput.JavaReader -> collector.awaitImages(config.timeoutMs)
            is RawOutput.ExternalSurface -> try {
                rawOutput.completion.get(config.timeoutMs, TimeUnit.MILLISECONDS)
                true
            } catch (_: TimeoutException) {
                false
            }
        }

    /** Остановка не должна маскировать результат: её ошибка пишется отдельным полем. */
    private fun stopQuietlyButReport(session: CameraCaptureSession?, report: JSONObject) {
        if (session == null) return
        try {
            session.stopRepeating()
            session.abortCaptures()
        } catch (error: Exception) {
            report.put("stop_exception", "${error.javaClass.name}: ${error.message}")
        } finally {
            session.close()
        }
    }

    private companion object {
        const val COMPANION_MAX_IMAGES = 4
    }
}
