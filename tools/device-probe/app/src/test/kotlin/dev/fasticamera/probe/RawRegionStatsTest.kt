package dev.fasticamera.probe

import dev.fasticamera.probe.camera.RawRegionStats
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder

class RawRegionStatsTest {

    /**
     * Синтетический RAW16: значение = база канала CFA × коэффициент области.
     * Центр яркий (×1.0), левый верхний угол затемнён (×0.5) — как виньетирование без коррекции.
     * Stride больше ширины: паддинг заполнен мусором 0xFFFF, который не должен попасть в статистику.
     */
    private fun syntheticFrame(width: Int, height: Int, rowStrideBytes: Int): ByteBuffer {
        val buffer = ByteBuffer.allocate(rowStrideBytes * height).order(ByteOrder.LITTLE_ENDIAN)
        val channelBase = intArrayOf(400, 800, 600, 200)
        for (y in 0 until height) {
            for (x in 0 until rowStrideBytes / 2) {
                val value = if (x >= width) {
                    0xFFFF
                } else {
                    val darkCorner = x < width / 4 && y < height / 4
                    val base = channelBase[(y and 1) * 2 + (x and 1)]
                    if (darkCorner) base / 2 else base
                }
                buffer.putShort(y * rowStrideBytes + x * 2, value.toShort())
            }
        }
        return buffer
    }

    @Test
    fun `каналы CFA разделены, паддинг строки не учитывается, угол темнее центра`() {
        val width = 64
        val height = 48
        val stride = width * 2 + 32
        val stats = RawRegionStats(blockSize = 8).compute(syntheticFrame(width, height, stride), width, height, stride)

        val center = stats.getJSONArray("center")
        assertEquals(400.0, center.getJSONObject(0).getDouble("mean"), 1e-9)
        assertEquals(800.0, center.getJSONObject(1).getDouble("mean"), 1e-9)
        assertEquals(600.0, center.getJSONObject(2).getDouble("mean"), 1e-9)
        assertEquals(200.0, center.getJSONObject(3).getDouble("mean"), 1e-9)
        // Плоская область — нулевой шум.
        assertEquals(0.0, center.getJSONObject(0).getDouble("std"), 1e-9)

        val topLeft = stats.getJSONArray("top_left")
        assertEquals(200.0, topLeft.getJSONObject(0).getDouble("mean"), 1e-9)
        assertEquals(400.0, topLeft.getJSONObject(1).getDouble("mean"), 1e-9)

        // Правый нижний угол не затемнён и не задет паддингом справа (там 0xFFFF).
        val bottomRight = stats.getJSONArray("bottom_right")
        assertEquals(800.0, bottomRight.getJSONObject(1).getDouble("mean"), 1e-9)
    }

    @Test
    fun `нечётный блок отвергается, иначе сбилась бы фаза CFA`() {
        assertThrows(IllegalArgumentException::class.java) { RawRegionStats(blockSize = 7) }
    }
}
