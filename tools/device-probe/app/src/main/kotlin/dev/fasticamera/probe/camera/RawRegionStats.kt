package dev.fasticamera.probe.camera

import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.sqrt

/**
 * C3: статистика RAW16-кадра по областям — центр и четыре угла, отдельно по каждому
 * из четырёх CFA-каналов (позиции 2×2 решётки Байера).
 *
 * Зачем: если вендор применил коррекцию виньетирования, на ровно освещённой поверхности
 * углы будут почти равны центру; без коррекции углы заметно темнее. Стандартное отклонение
 * той же области показывает, не шумодавит ли HAL RAW (NOISE_REDUCTION_MODE_OFF против HIGH_QUALITY).
 *
 * Работает только с плотными 16-битными данными (RAW_SENSOR): RAW10 упакован и здесь не разбирается.
 */
class RawRegionStats(private val blockSize: Int) {

    init {
        require(blockSize > 0 && blockSize % 2 == 0) { "blockSize должен быть чётным и > 0, получено $blockSize" }
    }

    /** Области кадра: имя и левый верхний угол блока (выровнен по 2, чтобы не сбить фазу CFA). */
    private fun regions(width: Int, height: Int): List<Triple<String, Int, Int>> {
        val maxX = alignDown(width - blockSize)
        val maxY = alignDown(height - blockSize)
        return listOf(
            Triple("center", alignDown((width - blockSize) / 2), alignDown((height - blockSize) / 2)),
            Triple("top_left", 0, 0),
            Triple("top_right", maxX, 0),
            Triple("bottom_left", 0, maxY),
            Triple("bottom_right", maxX, maxY),
        )
    }

    /**
     * @param plane буфер плоскости RAW16 (little-endian u16), позиция не меняется.
     * @param rowStrideBytes шаг строки в байтах (может быть больше width*2 из-за выравнивания).
     */
    fun compute(plane: ByteBuffer, width: Int, height: Int, rowStrideBytes: Int): JSONObject {
        require(width >= blockSize && height >= blockSize) { "кадр ${width}x$height меньше блока $blockSize" }
        val view = plane.duplicate().order(ByteOrder.LITTLE_ENDIAN)
        val result = JSONObject()
        for ((name, left, top) in regions(width, height)) {
            result.put(name, blockStats(view, left, top, rowStrideBytes))
        }
        return result.put("block_size", blockSize)
    }

    /** Среднее и СКО по каждому из 4 CFA-каналов внутри блока. */
    private fun blockStats(view: ByteBuffer, left: Int, top: Int, rowStrideBytes: Int): JSONArray {
        val sums = DoubleArray(CFA_CHANNELS)
        val squares = DoubleArray(CFA_CHANNELS)
        val counts = IntArray(CFA_CHANNELS)
        for (y in top until top + blockSize) {
            val rowOffset = y * rowStrideBytes
            for (x in left until left + blockSize) {
                val value = (view.getShort(rowOffset + x * BYTES_PER_SAMPLE).toInt() and U16_MASK).toDouble()
                // Канал = позиция в решётке 2×2: (y чётн., x чётн.) = 0, (чётн., нечётн.) = 1 и т.д.
                val channel = (y and 1) * 2 + (x and 1)
                sums[channel] += value
                squares[channel] += value * value
                counts[channel] += 1
            }
        }
        val channels = JSONArray()
        for (channel in 0 until CFA_CHANNELS) {
            val mean = sums[channel] / counts[channel]
            val variance = (squares[channel] / counts[channel] - mean * mean).coerceAtLeast(0.0)
            channels.put(JSONObject().put("mean", mean).put("std", sqrt(variance)))
        }
        return channels
    }

    private fun alignDown(value: Int): Int = value.coerceAtLeast(0) and 1.inv()

    private companion object {
        const val CFA_CHANNELS = 4
        const val BYTES_PER_SAMPLE = 2
        const val U16_MASK = 0xFFFF
    }
}
