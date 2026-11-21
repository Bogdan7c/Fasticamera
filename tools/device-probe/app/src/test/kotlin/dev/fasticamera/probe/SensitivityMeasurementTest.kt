package dev.fasticamera.probe

import dev.fasticamera.probe.camera.dual.BringUpOrder
import dev.fasticamera.probe.camera.dual.ExposureStep
import dev.fasticamera.probe.camera.dual.OpticalCenter
import dev.fasticamera.probe.camera.dual.OpticalCenterSource
import dev.fasticamera.probe.camera.dual.PixelRegion
import dev.fasticamera.probe.camera.dual.RawRegionSampler
import dev.fasticamera.probe.camera.dual.SensitivityGeometry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.nio.ByteBuffer

class SensitivityMeasurementTest {

    /** Упаковывает синтетический RAW10: значение зависит от CFA-позиции 2x2; в пикселях x>=clipFromX — белый уровень. */
    private fun packedFrame(width: Int, height: Int, rowStride: Int, clipFromX: Int = Int.MAX_VALUE): ByteBuffer {
        val buffer = ByteBuffer.allocate(rowStride * height)
        val channelValue = intArrayOf(100, 201, 302, 403)
        for (y in 0 until height) for (group in 0 until width / 4) {
            var lowBits = 0
            for (i in 0 until 4) {
                val x = group * 4 + i
                val value = if (x >= clipFromX) 1023 else channelValue[(y % 2) * 2 + (x % 2)]
                buffer.put(y * rowStride + group * 5 + i, (value shr 2).toByte())
                lowBits = lowBits or ((value and 3) shl (i * 2))
            }
            buffer.put(y * rowStride + group * 5 + 4, lowBits.toByte())
        }
        return buffer
    }

    @Test
    fun `сэмплер читает упакованный RAW10 по каналам и считает клиппинг`() {
        val sampler = RawRegionSampler(PixelRegion(left = 16, top = 16, width = 64, height = 64), whiteLevel = 1023)
        val sample = sampler.measure(packedFrame(128, 128, rowStride = 160, clipFromX = 48), 160, timestampNs = 7)
        assertEquals(7L, sample.timestampNs)
        // Блоки выборки по x: 16, 24, 32, 40 (нормальные), 48, 56, 64, 72 (в белом): пол блоков зажат, значит средние смешанные.
        assertEquals(listOf(32, 32, 32, 32), sample.clippedPerChannel.toList())
        assertEquals(64, sample.pixelsPerChannel)
        assertEquals((100.0 * 32 + 1023.0 * 32) / 64, sample.channelMeans[0], 1e-9)
    }

    @Test
    fun `центр берётся из калибровки, а нулевая калибровка падает на центр массива`() {
        val calibrated = SensitivityGeometry.opticalCenter(floatArrayOf(1f, 1f, 1000f, 500f, 0f), 4000, 3000)
        assertEquals(OpticalCenterSource.LENS_INTRINSIC_CALIBRATION, calibrated.source)
        assertEquals(0.25, calibrated.fractionX, 1e-9)
        val fallback = SensitivityGeometry.opticalCenter(floatArrayOf(1f, 1f, 0f, 0f, 0f), 4000, 3000)
        assertEquals(OpticalCenterSource.ARRAY_CENTER, fallback.source)
        assertEquals(0.5, fallback.fractionY, 1e-9)
    }

    @Test
    fun `область чётная, сдвигается внутрь буфера и имеет нужную долю`() {
        val region = SensitivityGeometry.centeredRegion(4096, 3072, 0.4, OpticalCenter(0.5, 0.5, OpticalCenterSource.ARRAY_CENTER))
        assertEquals(PixelRegion(left = 1229 and 1.inv(), top = 922 and 1.inv(), width = 1638, height = 1228), region)
        val edge = SensitivityGeometry.centeredRegion(4000, 3000, 0.4, OpticalCenter(0.01, 0.99, OpticalCenterSource.ARRAY_CENTER))
        assertEquals(0, edge.left)
        assertEquals(3000 - edge.height, edge.top)
    }

    @Test
    fun `EV диафрагмы и эквивалентное фокусное считаются по формулам`() {
        assertEquals(1.0, SensitivityGeometry.expectedEvFromAperture(apertureMain = 2.0, apertureWide = 2.0 * Math.sqrt(2.0)), 1e-9)
        assertEquals(1.0, SensitivityGeometry.evMainMinusWide(200.0, 100.0), 1e-9)
        assertEquals(35.0, SensitivityGeometry.equivalentFocalLengthMm(35.0 * 5.0 / 43.27 * 1.0, 3.0, 4.0), 0.01)
    }

    @Test
    fun `ступени и порядок разбираются строго`() {
        assertEquals(listOf(ExposureStep(2_000_000, 100), ExposureStep(1_000_000, 400)), ExposureStep.parseList(listOf("2000000:100", "1000000:400")))
        assertThrows(IllegalArgumentException::class.java) { ExposureStep.parseList(listOf("2000000")) }
        assertEquals(BringUpOrder.WIDE_FIRST, BringUpOrder.fromWire("wide_first"))
        assertThrows(IllegalArgumentException::class.java) { BringUpOrder.fromWire("both") }
    }
}
