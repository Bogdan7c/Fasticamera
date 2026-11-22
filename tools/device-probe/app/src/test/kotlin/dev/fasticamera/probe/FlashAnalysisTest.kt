package dev.fasticamera.probe

import dev.fasticamera.probe.camera.dual.GreenLayout
import dev.fasticamera.probe.camera.flash.Ambient
import dev.fasticamera.probe.camera.flash.FlashAnalysis
import dev.fasticamera.probe.camera.flash.RawBandAnalyzer
import dev.fasticamera.probe.camera.flash.RawBandProfile
import dev.fasticamera.probe.camera.flash.RawBandSettings
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FlashAnalysisTest {
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

    private val settings = RawBandSettings(GreenLayout(1, 2), blackLevel = 64, bandCount = 4, stepX = 2, stepY = 2, clipLevel = 1020)

    @Test
    fun `полосы различают частично освещённый кадр и клип считается`() {
        // 16x16: верхняя половина (полосы 0-1) зелёные 164, нижняя (2-3) зелёные 1023; красный/синий мусор не должен влиять.
        val buffer = packed(16, 16) { x, y -> if ((x + y) % 2 == 1) (if (y < 8) 164 else 1023) else 500 }
        val profile = RawBandAnalyzer.analyze(buffer, 20, 16, 16, settings)
        assertEquals(100.0, profile.bandMeans[0], 1e-9)
        assertEquals(100.0, profile.bandMeans[1], 1e-9)
        assertEquals(959.0, profile.bandMeans[2], 1e-9)
        assertEquals(959.0, profile.bandMeans[3], 1e-9)
        assertEquals((100.0 + 959.0) / 2, profile.overallMean, 1e-9)
        assertEquals(0.5, profile.clipFraction, 1e-9)
    }

    @Test
    fun `освещённость определяется порогом над фоном`() {
        val ambient = Ambient(100.0, DoubleArray(4) { 100.0 })
        assertFalse(FlashAnalysis.isLit(RawBandProfile(120.0, DoubleArray(4), 0.0), ambient)) // +20% < порога 25%
        assertTrue(FlashAnalysis.isLit(RawBandProfile(130.0, DoubleArray(4), 0.0), ambient))
        val darkAmbient = Ambient(0.0, DoubleArray(4))
        assertFalse(FlashAnalysis.isLit(RawBandProfile(4.0, DoubleArray(4), 0.0), darkAmbient)) // < 5 единиц
        assertTrue(FlashAnalysis.isLit(RawBandProfile(6.0, DoubleArray(4), 0.0), darkAmbient))
    }

    @Test
    fun `свёртка в серии считает подряд идущие значения`() {
        val runs = FlashAnalysis.runLengths(listOf(2, 2, 3, null, null, 2))
        assertEquals("[[2,2],[3,1],[null,2],[2,1]]", runs.toString())
    }
}
