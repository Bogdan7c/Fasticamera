package dev.fasticamera.probe.camera.dual

import android.media.ImageReader
import android.os.Handler
import android.os.SystemClock
import android.view.Surface
import dev.fasticamera.probe.camera.FrameTimingStats
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * Приёмник кадров одной ноги: ImageReader, который сразу возвращает картинки и копит их метки
 * сенсора (`Image.timestamp`) и момент прихода первого кадра по часам телефона.
 */
class LegRecorder(val spec: LegSpec, imageHandler: Handler, private val discardFrames: Boolean = false) {
    private val lock = Any()
    private val sensorTimestampsNs = ArrayList<Long>()
    private val acquireErrors = AtomicInteger(0)
    private val firstFrame = CountDownLatch(1)

    /** Момент первого кадра (elapsedRealtimeNanos) или null, пока кадров нет. */
    @Volatile
    var firstFrameAtNs: Long? = null
        private set

    private val reader: ImageReader = ImageReader.newInstance(
        spec.width, spec.height, spec.format.imageFormat, MAX_IMAGES,
    ).also { created ->
        created.setOnImageAvailableListener({ source -> drain(source) }, imageHandler)
    }

    val surface: Surface get() = reader.surface

    val framesReceived: Int get() = synchronized(lock) { sensorTimestampsNs.size }

    private fun drain(source: ImageReader) {
        val image = try {
            source.acquireNextImage()
        } catch (_: IllegalStateException) {
            // maxImages исчерпан: считаем, а не глотаем, — это признак, что слушатель не успевает.
            acquireErrors.incrementAndGet()
            null
        } ?: return
        val arrivedAtNs = SystemClock.elapsedRealtimeNanos()
        val timestampNs = image.timestamp
        image.close()
        // «В никуда»: буфер возвращён сразу, кадры не считаем — проверяем, что поток от этого не зависит.
        if (discardFrames) return
        synchronized(lock) { sensorTimestampsNs += timestampNs }
        if (firstFrameAtNs == null) {
            firstFrameAtNs = arrivedAtNs
            firstFrame.countDown()
        }
    }

    /** Ждёт первый кадр; false — не дождались за [timeoutMs]. */
    fun awaitFirstFrame(timeoutMs: Long): Boolean = firstFrame.await(timeoutMs, TimeUnit.MILLISECONDS)

    fun close() = reader.close()

    fun toJson(): JSONObject = synchronized(lock) {
        JSONObject()
            .put("leg", spec.label)
            .put("frames", sensorTimestampsNs.size)
            .put("timing", FrameTimingStats.compute(sensorTimestampsNs).toJson())
            .put("acquire_errors", acquireErrors.get())
    }

    private companion object {
        const val MAX_IMAGES = 4
    }
}
