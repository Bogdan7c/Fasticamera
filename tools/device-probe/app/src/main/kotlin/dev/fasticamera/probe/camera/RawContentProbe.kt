package dev.fasticamera.probe.camera

import android.media.Image
import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Проверка содержимого RAW16-кадров: настоящие ли это кадры, а не повторы или мусор.
 *
 * Вопросы, на которые отвечает:
 * - есть ли в буфере 4096×3072 «поля» (пустые полосы сверху/снизу), если сенсор работает в 16:9 —
 *   профиль средней яркости по горизонтальным полосам;
 * - не повторяет ли HAL один и тот же кадр, чтобы набрать 60 к/с — выборочный хеш пикселей;
 * - виньетирование и шум по углам — [RawRegionStats].
 *
 * Анализируется каждый [everyNth] кадр, чтобы не искажать замер темпа на остальных.
 */
class RawContentProbe(
    private val everyNth: Int,
    private val bandCount: Int,
    private val regionStats: RawRegionStats,
) {
    private val samples = JSONArray()
    private val sampleHashes = ArrayList<Long>()
    private var analyzed = 0
    private var seen = 0
    private var skippedReason: String? = null

    /** Вызывается слушателем картинок до `image.close()`. */
    @Synchronized
    fun maybeAnalyze(image: Image) {
        seen += 1
        if (everyNth <= 0 || seen % everyNth != 0 || analyzed >= MAX_SAMPLES) return
        val plane = image.planes.firstOrNull() ?: run {
            skippedReason = "нет плоскостей (формат ${image.format})"
            return
        }
        if (plane.pixelStride != BYTES_PER_SAMPLE) {
            skippedReason = "pixelStride=${plane.pixelStride}: не плотный RAW16"
            return
        }
        analyzed += 1
        val buffer = plane.buffer.duplicate().order(ByteOrder.LITTLE_ENDIAN)
        val hash = sparseHash(buffer, image.width, image.height, plane.rowStride)
        sampleHashes += hash
        samples.put(
            JSONObject()
                .put("frame_index", seen)
                .put("timestamp_ns", image.timestamp)
                .put("width", image.width)
                .put("height", image.height)
                .put("row_stride", plane.rowStride)
                .put("sparse_hash", java.lang.Long.toHexString(hash))
                .put("band_means", bandMeans(buffer, image.width, image.height, plane.rowStride))
                .put("regions", regionStats.compute(buffer, image.width, image.height, plane.rowStride)),
        )
    }

    @Synchronized
    fun toJson(): JSONObject = JSONObject()
        .put("every_nth", everyNth)
        .put("analyzed", analyzed)
        .put("skipped_reason", skippedReason ?: JSONObject.NULL)
        // Совпадение хешей двух разных кадров = HAL отдал повтор (или сцена абсолютно статична без шума, что для RAW нереально).
        .put("duplicate_hashes", sampleHashes.size - sampleHashes.toSet().size)
        .put("samples", samples)

    /** Средняя яркость каждой из [bandCount] горизонтальных полос (по каждой 8-й точке строки). */
    private fun bandMeans(buffer: ByteBuffer, width: Int, height: Int, rowStride: Int): JSONArray {
        val means = JSONArray()
        for (band in 0 until bandCount) {
            var sum = 0.0
            var count = 0
            // Начало полосы выравниваем на чётную строку, а шаг ROW_STEP чётный: так каждая полоса
            // читает одни и те же ряды CFA. Без этого при 3000 строк / 24 = 125 соседние полосы
            // попадали в разные ряды Байера и профиль «пилил» (артефакт, найденный в сессии 00).
            val bandStart = (band * height / bandCount) and 1.inv()
            val bandEnd = (band + 1) * height / bandCount
            for (y in bandStart until bandEnd step ROW_STEP) {
                for (x in 0 until width step COLUMN_STEP) {
                    sum += buffer.getShort(y * rowStride + x * BYTES_PER_SAMPLE).toInt() and U16_MASK
                    count += 1
                }
            }
            means.put(if (count > 0) sum / count else JSONObject.NULL)
        }
        return means
    }

    /** FNV-1a по ~16 тысячам точек сетки: дёшево и различает кадры по шуму сенсора. */
    private fun sparseHash(buffer: ByteBuffer, width: Int, height: Int, rowStride: Int): Long {
        var hash = FNV_OFFSET
        for (y in 0 until height step HASH_STEP) {
            for (x in 0 until width step HASH_STEP) {
                hash = (hash xor (buffer.getShort(y * rowStride + x * BYTES_PER_SAMPLE).toLong() and U16_MASK.toLong())) * FNV_PRIME
            }
        }
        return hash
    }

    private companion object {
        const val BYTES_PER_SAMPLE = 2
        const val U16_MASK = 0xFFFF
        const val MAX_SAMPLES = 12
        const val ROW_STEP = 4
        const val COLUMN_STEP = 8
        const val HASH_STEP = 29
        const val FNV_OFFSET = -0x340d631b7bdddcdbL
        const val FNV_PRIME = 0x100000001b3L
    }
}
