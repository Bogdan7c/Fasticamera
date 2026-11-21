package dev.fasticamera.probe

import dev.fasticamera.probe.camera.dual.GreenLayout
import dev.fasticamera.probe.camera.dual.GreenPlane
import dev.fasticamera.probe.camera.dual.PixelRegion
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Test

class GreenPlaneTest {
    /** Пакует RAW10: значение пикселя (x, y) = valueAt(x, y). */
    private fun packed(width: Int, height: Int, valueAt: (Int, Int) -> Int): ByteBuffer {
        val stride = width * 5 / 4
        val buffer = ByteBuffer.allocate(stride * height)
        for (y in 0 until height) for (group in 0 until width / 4) {
            var low = 0
            for (i in 0 until 4) {
                val value = valueAt(group * 4 + i, y)
                buffer.put(y * stride + group * 5 + i, (value shr 2).toByte())
                low = low or ((value and 3) shl (i * 2))
            }
            buffer.put(y * stride + group * 5 + 4, low.toByte())
        }
        return buffer
    }

    @Test
    fun `зелёная плоскость усредняет два зелёных, вычитает чёрный и уменьшает блоками`() {
        // RGGB: зелёные на позициях чтения 1 и 2. Значение зелёного = 100 + x (по столбцу), красный/синий мусор.
        val buffer = packed(16, 16) { x, y -> if ((x + y) % 2 == 1) 100 + x else 900 }
        val plane = GreenPlane.extract(
            buffer, 20, PixelRegion(left = 0, top = 0, width = 16, height = 16),
            GreenLayout(1, 2), doubleArrayOf(64.0, 64.0, 64.0, 64.0), whiteLevel = 1023, downsample = 4,
        )
        assertEquals(4L, plane.width.toLong())
        assertEquals(4L, plane.height.toLong())
        // Блок 0: ячейки x=0..3: Gr (x+1) даёт 101, 103; Gb (x) даёт 100, 102 -> среднее (101+100 и 103+102)/2 - 64.
        assertEquals(37.5, plane.pixels[0].toDouble(), 1e-3)
        assertEquals(0.0, plane.clippedFraction, 0.0)
    }

    @Test
    fun `клиппинг считается по сырым зелёным`() {
        val buffer = packed(16, 16) { _, _ -> 1023 }
        val plane = GreenPlane.extract(
            buffer, 20, PixelRegion(0, 0, 16, 16), GreenLayout(1, 2),
            doubleArrayOf(64.0, 64.0, 64.0, 64.0), whiteLevel = 1023, downsample = 4,
        )
        assertEquals(1.0, plane.clippedFraction, 0.0)
        assertEquals(1023.0 - 64.0, plane.pixels[0].toDouble(), 1e-3)
    }
}
