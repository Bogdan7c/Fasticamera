package dev.fasticamera.probe

import dev.fasticamera.probe.camera.shading.LensShadingGains
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class LensShadingGainsTest {
    @Test
    fun `плоский массив идёт по строкам, внутри точки каналы R Geven Godd B`() {
        // 3 столбца x 2 строки; значение кодирует (канал, столбец, строка) однозначно.
        val gains = LensShadingGains.flatten(columns = 3, rows = 2) { channel, column, row ->
            (channel * 100 + row * 10 + column).toFloat()
        }
        assertEquals(24, gains.size)
        // Первая точка (столбец 0, строка 0): 0, 100, 200, 300; вторая — столбец 1 той же строки.
        assertArrayEquals(floatArrayOf(0f, 100f, 200f, 300f, 1f, 101f, 201f, 301f), gains.copyOfRange(0, 8), 0f)
        // Первая точка второй строки начинается после 3 точек первой: индекс (1*3+0)*4 = 12.
        assertArrayEquals(floatArrayOf(10f, 110f, 210f, 310f), gains.copyOfRange(12, 16), 0f)
        // Последняя точка: столбец 2, строка 1, канал B.
        assertEquals(312f, gains[23], 0f)
    }
}
