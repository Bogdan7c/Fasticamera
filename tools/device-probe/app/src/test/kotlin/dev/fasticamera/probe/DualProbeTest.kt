package dev.fasticamera.probe

import dev.fasticamera.probe.camera.dual.LegFormat
import dev.fasticamera.probe.camera.dual.LegSpec
import dev.fasticamera.probe.camera.dual.SampleSummary
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class DualProbeTest {

    @Test
    fun `нога физической камеры разбирается целиком`() {
        val leg = LegSpec.parse("0:yuv:640x480:3")
        assertEquals("0", leg.cameraId)
        assertEquals("3", leg.physicalCameraId)
        assertEquals(LegFormat.YUV, leg.format)
        assertEquals("cam0/phys3:yuv:640x480", leg.label)
    }

    @Test
    fun `нога без физической камеры и с raw10`() {
        val leg = LegSpec.parse("2:raw10:4096x3072")
        assertNull(leg.physicalCameraId)
        assertEquals(LegFormat.RAW10, leg.format)
        assertEquals(4096, leg.width)
    }

    @Test
    fun `кривые ноги отвергаются с исключением`() {
        assertThrows(IllegalArgumentException::class.java) { LegSpec.parse("2:yuv") }
        assertThrows(IllegalArgumentException::class.java) { LegSpec.parse("2:bmp:640x480") }
        assertThrows(IllegalArgumentException::class.java) { LegSpec.parse("2:yuv:640") }
    }

    @Test
    fun `медиана нечётного и чётного ряда`() {
        assertEquals(5.0, SampleSummary.of(listOf(9.0, 1.0, 5.0))!!.median, 0.0)
        val even = SampleSummary.of(listOf(4.0, 1.0, 3.0, 2.0))!!
        assertEquals(2.5, even.median, 0.0)
        assertEquals(1.0, even.min, 0.0)
        assertEquals(4.0, even.max, 0.0)
    }

    @Test
    fun `пустой ряд не превращается в нули`() {
        assertNull(SampleSummary.of(emptyList()))
    }
}
