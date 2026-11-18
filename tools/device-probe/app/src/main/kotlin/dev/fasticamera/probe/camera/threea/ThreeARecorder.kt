package dev.fasticamera.probe.camera.threea

import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CaptureFailure
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.TotalCaptureResult
import android.media.ImageReader
import android.view.Surface
import dev.fasticamera.probe.report.ProbeLog
import org.json.JSONObject
import java.util.Collections
import java.util.concurrent.atomic.AtomicInteger

/**
 * Собирает результаты кадров (с метками запросов), отказы и метки пришедших RAW-картинок.
 * Каждый результат сразу пишется в JSONL событием `frame`; копия в памяти нужна для сводок фаз.
 */
class ThreeARecorder(private val log: ProbeLog, private val rawContentProbe: ThreeARaw10Probe? = null) : CameraCaptureSession.CaptureCallback() {

    private val rows: MutableList<ThreeAFrameRow> = Collections.synchronizedList(ArrayList())
    private val imageTimestamps: MutableSet<Long> = Collections.synchronizedSet(HashSet())
    private val failuresByPhase: MutableMap<String, Int> = Collections.synchronizedMap(HashMap())
    private val imageAcquireErrors = AtomicInteger(0)
    private val untaggedResults = AtomicInteger(0)

    @Volatile
    var lastFrameNumber: Long = -1
        private set

    val imageAcquireErrorCount: Int get() = imageAcquireErrors.get()
    val untaggedResultCount: Int get() = untaggedResults.get()

    override fun onCaptureCompleted(session: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
        val tag = request.tag as? ThreeATag
        if (tag == null) {
            untaggedResults.incrementAndGet()
            return
        }
        val row = ThreeAFrameRow.from(tag, result)
        rows.add(row)
        lastFrameNumber = maxOf(lastFrameNumber, row.frameNumber)
        log.event("frame", row.json)
    }

    override fun onCaptureFailed(session: CameraCaptureSession, request: CaptureRequest, failure: CaptureFailure) {
        val tag = request.tag as? ThreeATag
        failuresByPhase.merge(tag?.phase ?: "<untagged>", 1, Int::plus)
        log.event(
            "capture_failed",
            JSONObject().put("tag", tag?.toString() ?: "<untagged>").put("frame_number", failure.frameNumber)
                .put("reason", failure.reason).put("was_image_captured", failure.wasImageCaptured()),
        )
    }

    override fun onCaptureBufferLost(session: CameraCaptureSession, request: CaptureRequest, target: Surface, frameNumber: Long) {
        val tag = request.tag as? ThreeATag
        failuresByPhase.merge("buffer_lost:" + (tag?.phase ?: "<untagged>"), 1, Int::plus)
        log.event("buffer_lost", JSONObject().put("tag", tag?.toString() ?: "<untagged>").put("frame_number", frameNumber))
    }

    /** RAW-картинки закрываются сразу: иначе очередь ридера переполнится и HAL начнёт терять буферы. */
    val imageListener = ImageReader.OnImageAvailableListener { reader ->
        val image = try {
            reader.acquireNextImage()
        } catch (error: IllegalStateException) {
            imageAcquireErrors.incrementAndGet()
            log.error("image_acquire", error)
            null
        } ?: return@OnImageAvailableListener
        try {
            imageTimestamps.add(image.timestamp)
            rawContentProbe?.maybeAnalyze(image)
        } finally {
            image.close()
        }
    }

    private val companionTimestamps: MutableList<Long> = Collections.synchronizedList(ArrayList())

    /** Вспомогательный поток: фиксируем метку (для реального к/с) и сразу закрываем картинку. */
    val companionImageListener = ImageReader.OnImageAvailableListener { reader ->
        val image = try {
            reader.acquireNextImage()
        } catch (error: IllegalStateException) {
            imageAcquireErrors.incrementAndGet()
            null
        } ?: return@OnImageAvailableListener
        try {
            companionTimestamps.add(image.timestamp)
        } finally {
            image.close()
        }
    }

    fun companionTimestampsSnapshot(): List<Long> = synchronized(companionTimestamps) { companionTimestamps.toList() }

    fun rowsOfPhase(phase: String): List<ThreeAFrameRow> = synchronized(rows) { rows.filter { it.tag.phase == phase } }

    fun allRows(): List<ThreeAFrameRow> = synchronized(rows) { rows.toList() }

    fun hasImageWithTimestamp(timestampNs: Long): Boolean = imageTimestamps.contains(timestampNs)

    fun rawImageTimestampsSorted(): List<Long> = synchronized(imageTimestamps) { imageTimestamps.sorted() }

    val imageCount: Int get() = imageTimestamps.size

    fun failureCounts(): Map<String, Int> = synchronized(failuresByPhase) { failuresByPhase.toMap() }
}
