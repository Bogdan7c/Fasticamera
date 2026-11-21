package dev.fasticamera.probe.camera.dual

import dev.fasticamera.probe.camera.Raw10Pixels
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Раскладка зелёных в ячейке 2x2: индексы чтения (строка*2 + столбец) двух зелёных каналов. */
class GreenLayout(val firstReadoutIndex: Int, val secondReadoutIndex: Int)

/**
 * Зелёная плоскость для сопоставления кадров двух камер: среднее двух зелёных пикселей каждой CFA-ячейки 2x2,
 * вычет чёрного, затем усреднение блоками [downsample] x [downsample] СЫРЫХ пикселей (downsample чётный:
 * блок = целое число ячеек, фаза CFA не плывёт). Пиксели выходного изображения идут строка за строкой.
 */
class GreenPlane(val width: Int, val height: Int, val pixels: FloatArray, val clippedFraction: Double, val meanLevel: Double) {

    /** Little-endian float32 без заголовка: размеры и геометрия лежат в JSON-файле рядом. */
    fun toLittleEndianBytes(): ByteArray {
        val buffer = ByteBuffer.allocate(pixels.size * Float.SIZE_BYTES).order(ByteOrder.LITTLE_ENDIAN)
        pixels.forEach { buffer.putFloat(it) }
        return buffer.array()
    }

    companion object {
        /**
         * [region] — область буфера (чётные левый верхний угол и размеры); [blackLevels] — чёрный по четырём
         * позициям чтения; [whiteLevel] — порог клиппинга. Не-делящийся остаток блока отбрасывается.
         */
        fun extract(
            buffer: ByteBuffer,
            rowStrideBytes: Int,
            region: PixelRegion,
            greens: GreenLayout,
            blackLevels: DoubleArray,
            whiteLevel: Int,
            downsample: Int,
        ): GreenPlane {
            require(downsample >= 2 && downsample % 2 == 0) { "downsample должен быть чётным >= 2, получено $downsample" }
            val outWidth = region.width / downsample
            val outHeight = region.height / downsample
            require(outWidth > 0 && outHeight > 0) { "область $region слишком мала для downsample=$downsample" }
            val out = FloatArray(outWidth * outHeight)
            var clipped = 0L
            var total = 0L
            val cellsPerBlockSide = downsample / 2
            val cellsPerBlock = (cellsPerBlockSide * cellsPerBlockSide).toDouble()
            for (blockY in 0 until outHeight) {
                for (blockX in 0 until outWidth) {
                    var sum = 0.0
                    for (cellY in 0 until cellsPerBlockSide) {
                        val rawY = region.top + blockY * downsample + cellY * 2
                        for (cellX in 0 until cellsPerBlockSide) {
                            val rawX = region.left + blockX * downsample + cellX * 2
                            sum += greenOfCell(buffer, rowStrideBytes, rawX, rawY, greens, blackLevels) { raw ->
                                total++
                                if (raw >= whiteLevel) clipped++
                            }
                        }
                    }
                    out[blockY * outWidth + blockX] = (sum / cellsPerBlock).toFloat()
                }
            }
            return GreenPlane(outWidth, outHeight, out, clipped.toDouble() / total, out.average())
        }

        /** Чёрно-вычтенное среднее двух зелёных одной ячейки; [onRaw] получает каждое сырое зелёное значение. */
        private inline fun greenOfCell(
            buffer: ByteBuffer,
            rowStrideBytes: Int,
            x: Int,
            y: Int,
            greens: GreenLayout,
            blackLevels: DoubleArray,
            onRaw: (Int) -> Unit,
        ): Double {
            val first = Raw10Pixels.read(buffer, rowStrideBytes, x + greens.firstReadoutIndex % 2, y + greens.firstReadoutIndex / 2)
            val second = Raw10Pixels.read(buffer, rowStrideBytes, x + greens.secondReadoutIndex % 2, y + greens.secondReadoutIndex / 2)
            onRaw(first)
            onRaw(second)
            return ((first - blackLevels[greens.firstReadoutIndex]) + (second - blackLevels[greens.secondReadoutIndex])) / 2.0
        }
    }
}
