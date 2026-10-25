package dev.fasticamera.probe.camera

import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CaptureFailure
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.media.ImageReader
import android.view.Surface
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * Собирает факты о потоке: метки сенсора из результатов, метки пришедших RAW-картинок,
 * отказы, потерянные буферы и значения вендорских ключей результата.
 *
 * Метки из результатов и из картинок считаются раздельно: HAL может отдавать
 * метаданные на 60 к/с, а буферы — реже. Для вердикта «4K60 RAW» важны именно картинки.
 */
class FrameCollector(
    private val frameTarget: Int,
    private val resultKeys: List<VendorResultKey>,
    /** Проверка содержимого кадров (C2-валидация, C3); null — только темп. */
    private val contentProbe: RawContentProbe?,
) : CameraCaptureSession.CaptureCallback() {

    private val lock = Any()
    private val resultTimestamps = ArrayList<Long>(frameTarget)
    private val imageTimestamps = ArrayList<Long>(frameTarget)
    private val frameDurations = ArrayList<Long>(frameTarget)
    private val exposureTimes = ArrayList<Long>(frameTarget)
    private val failureReasons = HashMap<String, Int>()
    private val lostBuffers = AtomicInteger(0)
    private val imageAcquireErrors = AtomicInteger(0)
    private val imageAnalysisErrors = AtomicInteger(0)
    private var binningFactorUsed: Boolean? = null
    private var firstResultSnapshot: JSONObject? = null

    /** Для каждого вендорского ключа — множество наблюдённых значений (по строковому виду). */
    private val vendorValues = LinkedHashMap<String, MutableSet<String>>()

    /** Срабатывает, когда набралось [frameTarget] картинок — сценарий перестаёт ждать. */
    private val enoughImages = CountDownLatch(1)

    override fun onCaptureCompleted(session: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
        val sensorTimestamp = result.get(CaptureResult.SENSOR_TIMESTAMP)
        synchronized(lock) {
            if (resultTimestamps.size >= frameTarget) return
            sensorTimestamp?.let(resultTimestamps::add)
            result.get(CaptureResult.SENSOR_FRAME_DURATION)?.let(frameDurations::add)
            result.get(CaptureResult.SENSOR_EXPOSURE_TIME)?.let(exposureTimes::add)
            result.get(CaptureResult.SENSOR_RAW_BINNING_FACTOR_USED)?.let { binningFactorUsed = it }
            for (key in resultKeys) {
                val observed = when (val outcome = key.read(result)) {
                    is VendorResultKey.ReadOutcome.Value -> outcome.value.toString()
                    VendorResultKey.ReadOutcome.Absent -> "<absent>"
                    is VendorResultKey.ReadOutcome.Failed -> "<failed: ${outcome.reason}>"
                }
                vendorValues.getOrPut(key.name) { LinkedHashSet() }.add(observed)
            }
            if (firstResultSnapshot == null) {
                firstResultSnapshot = snapshot(result)
            }
        }
    }

    override fun onCaptureFailed(session: CameraCaptureSession, request: CaptureRequest, failure: CaptureFailure) {
        val reason = when (failure.reason) {
            CaptureFailure.REASON_ERROR -> "REASON_ERROR"
            CaptureFailure.REASON_FLUSHED -> "REASON_FLUSHED"
            else -> "REASON_${failure.reason}"
        }
        synchronized(lock) { failureReasons.merge(reason, 1, Int::plus) }
    }

    override fun onCaptureBufferLost(session: CameraCaptureSession, request: CaptureRequest, target: Surface, frameNumber: Long) {
        lostBuffers.incrementAndGet()
    }

    /**
     * Слушатель RAW-картинок: забираем метку и сразу закрываем картинку,
     * иначе очередь ImageReader переполнится и HAL начнёт терять буферы (это исказит замер).
     */
    val imageListener = ImageReader.OnImageAvailableListener { reader ->
        val image = try {
            reader.acquireNextImage()
        } catch (error: IllegalStateException) {
            // maxImages исчерпан — считаем, а не глотаем: это сигнал, что слушатель не успевает.
            imageAcquireErrors.incrementAndGet()
            null
        } ?: return@OnImageAvailableListener
        val timestamp = image.timestamp
        try {
            contentProbe?.maybeAnalyze(image)
        } catch (error: RuntimeException) {
            // Сбой анализа не должен ломать замер темпа: считаем и продолжаем.
            imageAnalysisErrors.incrementAndGet()
        } finally {
            image.close()
        }
        val reachedTarget = synchronized(lock) {
            if (imageTimestamps.size < frameTarget) imageTimestamps.add(timestamp)
            imageTimestamps.size >= frameTarget
        }
        if (reachedTarget) enoughImages.countDown()
    }

    /** Ждёт нужное число картинок; false — вышел таймаут (это тоже результат, а не ошибка зонда). */
    fun awaitImages(timeoutMs: Long): Boolean = enoughImages.await(timeoutMs, TimeUnit.MILLISECONDS)

    fun toJson(): JSONObject = synchronized(lock) {
        JSONObject()
            .put("result_timing", FrameTimingStats.compute(resultTimestamps).toJson())
            .put("image_timing", FrameTimingStats.compute(imageTimestamps).toJson())
            .put("images_received", imageTimestamps.size)
            .put("results_received", resultTimestamps.size)
            .put("frame_duration_ns", distinctSummary(frameDurations))
            .put("exposure_time_ns", distinctSummary(exposureTimes))
            .put("raw_binning_factor_used", binningFactorUsed ?: JSONObject.NULL)
            .put("capture_failures", JSONObject(failureReasons as Map<*, *>))
            .put("buffers_lost", lostBuffers.get())
            .put("image_acquire_errors", imageAcquireErrors.get())
            .put("image_analysis_errors", imageAnalysisErrors.get())
            .put("content", contentProbe?.toJson() ?: JSONObject.NULL)
            .put("vendor_result_values", JSONObject(vendorValues.mapValues { JSONArray(it.value.toList()) }))
            .put("first_result", firstResultSnapshot ?: JSONObject.NULL)
    }

    /** Мин/макс и до 8 различных значений: видно, держит ли HAL заданную длительность кадра. */
    private fun distinctSummary(values: List<Long>): JSONObject {
        if (values.isEmpty()) return JSONObject().put("count", 0)
        return JSONObject()
            .put("count", values.size)
            .put("min", values.min())
            .put("max", values.max())
            .put("distinct_sample", JSONArray(values.distinct().take(MAX_DISTINCT_SAMPLE)))
    }

    /** Ключевые поля первого результата: что HAL реально применил из нашего запроса. */
    private fun snapshot(result: CaptureResult): JSONObject = JSONObject()
        .put("ae_mode", result.get(CaptureResult.CONTROL_AE_MODE) ?: JSONObject.NULL)
        .put("ae_target_fps_range", result.get(CaptureResult.CONTROL_AE_TARGET_FPS_RANGE)?.toString() ?: JSONObject.NULL)
        .put("frame_duration_ns", result.get(CaptureResult.SENSOR_FRAME_DURATION) ?: JSONObject.NULL)
        .put("exposure_ns", result.get(CaptureResult.SENSOR_EXPOSURE_TIME) ?: JSONObject.NULL)
        .put("iso", result.get(CaptureResult.SENSOR_SENSITIVITY) ?: JSONObject.NULL)
        .put("shading_mode", result.get(CaptureResult.SHADING_MODE) ?: JSONObject.NULL)
        .put("noise_reduction_mode", result.get(CaptureResult.NOISE_REDUCTION_MODE) ?: JSONObject.NULL)
        .put("rolling_shutter_skew_ns", result.get(CaptureResult.SENSOR_ROLLING_SHUTTER_SKEW) ?: JSONObject.NULL)
        .put("ois_mode", result.get(CaptureResult.LENS_OPTICAL_STABILIZATION_MODE) ?: JSONObject.NULL)
        .put("dynamic_black_level", result.get(CaptureResult.SENSOR_DYNAMIC_BLACK_LEVEL)?.joinToString() ?: JSONObject.NULL)
        .put("dynamic_white_level", result.get(CaptureResult.SENSOR_DYNAMIC_WHITE_LEVEL) ?: JSONObject.NULL)
        .put("lens_shading_map_mode", result.get(CaptureResult.STATISTICS_LENS_SHADING_MAP_MODE) ?: JSONObject.NULL)

    private companion object {
        const val MAX_DISTINCT_SAMPLE = 8
    }
}
