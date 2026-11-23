package dev.fasticamera.probe.camera.shading

import android.hardware.camera2.TotalCaptureResult
import android.media.Image
import dev.fasticamera.probe.camera.Raw10Pixels
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.BlockingQueue
import java.util.concurrent.TimeUnit

/** Потребитель плотного кадра (после распаковки), например запись RAW16 на диск. */
fun interface DenseFrameConsumer {
    /** [frameIndex] — номер собранного кадра в фазе (с нуля); [validRows] — строки с данными. */
    fun onFrame(frameIndex: Int, pixels: ShortArray, width: Int, validRows: Int)
}

/** Что и как собирать в одной фазе. */
class CollectionRequest(
    /** Поколение запроса: кадры других поколений отбрасываются как устаревшие. */
    val generation: Int,
    /** Сколько первых кадров поколения пропустить (HAL применяет значения с задержкой). */
    val settleFrames: Int,
    val wantedFrames: Int,
    val timeoutMs: Long,
    val denseConsumer: DenseFrameConsumer? = null,
)

/** Счётчики пропущенных кадров: каждая причина отдельно, чтобы потери были видны. */
class CollectionCounters {
    var staleGeneration = 0
    var settling = 0
    var noResult = 0
    var inconsistent = 0
    val inconsistencyMessages = ArrayList<String>()

    fun toJson(): JSONObject = JSONObject()
        .put("stale_generation", staleGeneration)
        .put("settling", settling)
        .put("no_result", noResult)
        .put("inconsistent", inconsistent)
        .put("inconsistency_messages", JSONArray(inconsistencyMessages))
}

/** Итог сбора одной фазы; [means] null, если не собран ни один кадр. */
class PhaseCollection(
    val grid: BlockGrid?,
    val validRows: Int,
    val means: FloatArray?,
    val frameRecords: List<JSONObject>,
    val counters: CollectionCounters,
    val wantedFrames: Int,
) {
    val collectedFrames: Int get() = frameRecords.size
}

/** Геометрия буфера по первому кадру (в отчёт). */
data class BufferGeometry(val width: Int, val height: Int, val rowStrideBytes: Int)

/**
 * Берёт кадры RAW10 из очереди по одному, парой к результату по метке сенсора, распаковывает в плотный u16
 * и копит средние по блокам. Работает в потоке сценария (не в потоке колбэков камеры): тяжёлая распаковка
 * 12,6 Мпикс не блокирует камеру, а лишние кадры камере возвращает слушатель ридера.
 */
class ShadingFrameCollector(
    private val images: BlockingQueue<Image>,
    private val results: FrameResultStore<TotalCaptureResult>,
) {
    /** Геометрия буфера по первому увиденному кадру. */
    var geometry: BufferGeometry? = null
        private set

    private var dense: ShortArray? = null
    private var packedRow: ByteArray? = null

    fun collect(request: CollectionRequest): PhaseCollection {
        val counters = CollectionCounters()
        val records = ArrayList<JSONObject>()
        var accumulator: BlockMeanAccumulator? = null
        var validRowsForGrid = 0
        val deadlineNs = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(request.timeoutMs)
        while (records.size < request.wantedFrames && System.nanoTime() < deadlineNs) {
            val image = images.poll(POLL_INTERVAL_MS, TimeUnit.MILLISECONDS) ?: continue
            try {
                val result = acceptableResult(image, request, counters) ?: continue
                val pixels = unpack(image)
                val validRows = ValidRows.detect(pixels, image.width, image.height)
                val frameAccumulator = accumulator ?: BlockMeanAccumulator(image.width, validRows).also {
                    accumulator = it
                    validRowsForGrid = validRows
                }
                try {
                    frameAccumulator.addFrame(pixels, image.width, validRows)
                } catch (error: IllegalArgumentException) {
                    // Кадр не подходит к сетке (например, режим сенсора сменился посреди фазы): считаем и пропускаем.
                    counters.inconsistent++
                    counters.inconsistencyMessages += error.message.orEmpty()
                    continue
                }
                request.denseConsumer?.onFrame(records.size, pixels, image.width, validRows)
                records += ShadingJson.frameRecord(result, request.generation, validRows)
            } finally {
                image.close()
            }
        }
        val grid = accumulator?.grid
        return PhaseCollection(grid, validRowsForGrid, accumulator?.takeIf { it.framesAccumulated > 0 }?.means(), records, counters, request.wantedFrames)
    }

    /** Результат кадра, если кадр годен для сбора (то же поколение, успокоился, результат нашёлся), иначе null со счётчиком. */
    private fun acceptableResult(image: Image, request: CollectionRequest, counters: CollectionCounters): TotalCaptureResult? {
        val result = results.awaitResult(image.timestamp, RESULT_WAIT_MS)
        if (result == null) {
            counters.noResult++
            return null
        }
        val generation = result.request.tag as? Int
        if (generation != request.generation) {
            counters.staleGeneration++
            return null
        }
        val firstFrame = results.firstFrameNumber(request.generation) ?: return null
        if (result.frameNumber - firstFrame < request.settleFrames) {
            counters.settling++
            return null
        }
        return result
    }

    /** RAW10 → плотный массив u16 width x height (переиспользуется между кадрами). */
    private fun unpack(image: Image): ShortArray {
        val plane = image.planes[0]
        val width = image.width
        val height = image.height
        if (geometry == null) geometry = BufferGeometry(width, height, plane.rowStride)
        val pixels = dense?.takeIf { it.size == width * height } ?: ShortArray(width * height).also { dense = it }
        val rowBytes = packedRow?.takeIf { it.size == Raw10Pixels.packedRowBytes(width) }
            ?: ByteArray(Raw10Pixels.packedRowBytes(width)).also { packedRow = it }
        val buffer = plane.buffer.duplicate()
        for (y in 0 until height) {
            buffer.position(y * plane.rowStride)
            buffer.get(rowBytes)
            Raw10Pixels.unpackRow(rowBytes, width, pixels, y * width)
        }
        return pixels
    }

    private companion object {
        const val POLL_INTERVAL_MS = 100L

        /** Результат приходит за несколько мс до/после картинки; секунда — с большим запасом. */
        const val RESULT_WAIT_MS = 1_000L
    }
}
