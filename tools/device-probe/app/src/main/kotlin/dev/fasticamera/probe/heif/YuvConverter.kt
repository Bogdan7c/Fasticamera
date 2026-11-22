package dev.fasticamera.probe.heif

/** Матрица RGB→YCbCr: коэффициенты яркости Kr и Kb (Kg = 1 − Kr − Kb). */
enum class YuvMatrix(val kr: Double, val kb: Double) {
    BT601(0.299, 0.114),
    BT709(0.2126, 0.0722),
}

/** Диапазон кодирования: полный 0..255 или «телевизионный» (Y 16..235, C 16..240). */
enum class YuvRange { FULL, LIMITED }

/**
 * RGB→YUV420 на CPU. Наш собственный код: так мы точно знаем матрицу и диапазон,
 * которые отдали кодеру, и можем сравнить с тем, что он/декодер «поняли».
 * Цветность субдискретизируется усреднением блока 2×2 (чётные размеры мира обязательны).
 */
object YuvConverter {

    /** Планарный I420: Y (w*h), затем U (w/2*h/2), затем V. Так HeifWriter.addYuvBuffer ждёт YUV_420_888. */
    fun toI420(argb: IntArray, width: Int, height: Int, matrix: YuvMatrix, range: YuvRange): ByteArray {
        val planes = convertPlanes(argb, width, height, matrix, range)
        val out = ByteArray(planes.luma.size + planes.cb.size + planes.cr.size)
        System.arraycopy(planes.luma, 0, out, 0, planes.luma.size)
        System.arraycopy(planes.cb, 0, out, planes.luma.size, planes.cb.size)
        System.arraycopy(planes.cr, 0, out, planes.luma.size + planes.cb.size, planes.cr.size)
        return out
    }

    /** NV21: Y, затем чередующиеся V,U. Формат YuvImage. */
    fun toNv21(argb: IntArray, width: Int, height: Int, matrix: YuvMatrix, range: YuvRange): ByteArray {
        val planes = convertPlanes(argb, width, height, matrix, range)
        val out = ByteArray(planes.luma.size + planes.cb.size * 2)
        System.arraycopy(planes.luma, 0, out, 0, planes.luma.size)
        var offset = planes.luma.size
        for (index in planes.cb.indices) {
            out[offset++] = planes.cr[index]
            out[offset++] = planes.cb[index]
        }
        return out
    }

    private class Planes(val luma: ByteArray, val cb: ByteArray, val cr: ByteArray)

    private fun convertPlanes(argb: IntArray, width: Int, height: Int, matrix: YuvMatrix, range: YuvRange): Planes {
        require(width % 2 == 0 && height % 2 == 0) { "нужны чётные размеры, а не ${width}x$height" }
        val kr = matrix.kr
        val kb = matrix.kb
        val kg = 1.0 - kr - kb
        // Масштаб и смещение диапазона: Y' = yOffset + yScale*y, C' = 128 + cScale*c.
        val yScale = if (range == YuvRange.FULL) 255.0 else 219.0
        val yOffset = if (range == YuvRange.FULL) 0.0 else 16.0
        val cScale = if (range == YuvRange.FULL) 255.0 else 224.0
        val luma = ByteArray(width * height)
        val chromaWidth = width / 2
        val cb = ByteArray(chromaWidth * (height / 2))
        val cr = ByteArray(cb.size)
        for (blockRow in 0 until height / 2) {
            for (blockColumn in 0 until chromaWidth) {
                var sumCb = 0.0
                var sumCr = 0.0
                for (dy in 0..1) {
                    for (dx in 0..1) {
                        val x = blockColumn * 2 + dx
                        val y = blockRow * 2 + dy
                        val pixel = argb[y * width + x]
                        val r = ((pixel shr 16) and 0xFF) / 255.0
                        val g = ((pixel shr 8) and 0xFF) / 255.0
                        val b = (pixel and 0xFF) / 255.0
                        val luminance = kr * r + kg * g + kb * b
                        luma[y * width + x] = clampToByte(yOffset + yScale * luminance)
                        sumCb += (b - luminance) / (2.0 * (1.0 - kb))
                        sumCr += (r - luminance) / (2.0 * (1.0 - kr))
                    }
                }
                cb[blockRow * chromaWidth + blockColumn] = clampToByte(128.0 + cScale * sumCb / 4.0)
                cr[blockRow * chromaWidth + blockColumn] = clampToByte(128.0 + cScale * sumCr / 4.0)
            }
        }
        return Planes(luma, cb, cr)
    }

    private fun clampToByte(value: Double): Byte = Math.round(value).coerceIn(0L, 255L).toInt().toByte()
}
