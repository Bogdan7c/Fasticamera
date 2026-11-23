package dev.fasticamera.probe.camera

import java.nio.ByteBuffer

/**
 * Чтение отдельных пикселей упакованного RAW10 (формат Android `ImageFormat.RAW10`).
 *
 * Упаковка: 4 пикселя = 5 байт. Первые 4 байта — старшие 8 бит пикселей группы,
 * пятый байт — младшие 2 бита каждого из четырёх (пиксель 0 — в битах 0..1, пиксель 1 — в 2..3 и т.д.).
 * Общий код для всех проб, которым нужны значения RAW10 (статистика 3A, замер чувствительности).
 */
object Raw10Pixels {
    /** Пикселей в одной упакованной группе. */
    private const val PIXELS_PER_GROUP = 4

    /** Байт в одной упакованной группе. */
    private const val BYTES_PER_GROUP = 5

    /** Бит младших разрядов на пиксель. */
    private const val LOW_BITS_PER_PIXEL = 2

    private const val LOW_BITS_MASK = 0x3
    private const val BYTE_MASK = 0xFF

    /** Значение пикселя (x, y) в диапазоне 0..1023; [rowStrideBytes] — шаг строки в байтах. */
    fun read(buffer: ByteBuffer, rowStrideBytes: Int, x: Int, y: Int): Int {
        val groupBase = y * rowStrideBytes + (x / PIXELS_PER_GROUP) * BYTES_PER_GROUP
        val indexInGroup = x % PIXELS_PER_GROUP
        val high = buffer.get(groupBase + indexInGroup).toInt() and BYTE_MASK
        val lowByte = buffer.get(groupBase + PIXELS_PER_GROUP).toInt() and BYTE_MASK
        val low = (lowByte shr (indexInGroup * LOW_BITS_PER_PIXEL)) and LOW_BITS_MASK
        return (high shl LOW_BITS_PER_PIXEL) or low
    }

    /**
     * Распаковывает одну строку RAW10 ([packedRow] — байты строки, без паддинга) в [output] начиная с [outputOffset].
     * Массовый вариант [read]: на 12,6 Мпикс поштучное чтение из ByteBuffer было бы слишком медленным.
     */
    fun unpackRow(packedRow: ByteArray, width: Int, output: ShortArray, outputOffset: Int) {
        require(width % PIXELS_PER_GROUP == 0) { "ширина $width не кратна $PIXELS_PER_GROUP" }
        require(packedRow.size >= width / PIXELS_PER_GROUP * BYTES_PER_GROUP) { "строка ${packedRow.size} байт короче ширины $width" }
        var packedIndex = 0
        var outputIndex = outputOffset
        repeat(width / PIXELS_PER_GROUP) {
            val lowByte = packedRow[packedIndex + PIXELS_PER_GROUP].toInt() and BYTE_MASK
            for (pixel in 0 until PIXELS_PER_GROUP) {
                val high = packedRow[packedIndex + pixel].toInt() and BYTE_MASK
                val low = (lowByte shr (pixel * LOW_BITS_PER_PIXEL)) and LOW_BITS_MASK
                output[outputIndex++] = ((high shl LOW_BITS_PER_PIXEL) or low).toShort()
            }
            packedIndex += BYTES_PER_GROUP
        }
    }

    /** Байт упакованной строки RAW10 без паддинга для ширины [width]. */
    fun packedRowBytes(width: Int): Int = width / PIXELS_PER_GROUP * BYTES_PER_GROUP
}
