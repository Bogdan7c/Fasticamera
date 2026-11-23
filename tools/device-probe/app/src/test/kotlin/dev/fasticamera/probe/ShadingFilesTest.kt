package dev.fasticamera.probe

import dev.fasticamera.probe.camera.Raw10Pixels
import dev.fasticamera.probe.camera.shading.ShadingFiles
import dev.fasticamera.probe.camera.shading.ShadingMode
import dev.fasticamera.probe.camera.shading.ShadingOptions
import dev.fasticamera.probe.scenario.ScenarioArgs
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ShadingFilesTest {
    @Test
    fun `float32 пишется little-endian подряд`() {
        val file = File.createTempFile("blocks", ".f32").apply { deleteOnExit() }
        ShadingFiles.writeFloat32LittleEndian(file, floatArrayOf(1.5f, -2f, 1000.25f))
        val read = ByteBuffer.wrap(file.readBytes()).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(12, file.length().toInt())
        assertEquals(1.5f, read.getFloat(0), 0f)
        assertEquals(-2f, read.getFloat(4), 0f)
        assertEquals(1000.25f, read.getFloat(8), 0f)
    }

    @Test
    fun `RAW16 пишет только валидные строки плотно u16 little-endian`() {
        val width = 3
        val pixels = ShortArray(width * 4) { (it + 1000).toShort() }
        val file = File.createTempFile("frame", ".raw16").apply { deleteOnExit() }
        ShadingFiles.writeDenseRaw16(file, pixels, width, rows = 3)
        val read = ByteBuffer.wrap(file.readBytes()).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(width * 3 * 2, file.length().toInt())
        assertEquals(1000, read.getShort(0).toInt())
        assertEquals(1008, read.getShort(16).toInt())
    }

    @Test
    fun `распаковка строки RAW10 совпадает с поштучным чтением`() {
        val width = 16
        val values = IntArray(width) { (it * 61 + 7) % 1024 }
        val packed = ByteArray(Raw10Pixels.packedRowBytes(width))
        for (group in 0 until width / 4) {
            var low = 0
            for (i in 0 until 4) {
                packed[group * 5 + i] = (values[group * 4 + i] shr 2).toByte()
                low = low or ((values[group * 4 + i] and 3) shl (i * 2))
            }
            packed[group * 5 + 4] = low.toByte()
        }
        val unpacked = ShortArray(width + 2)
        Raw10Pixels.unpackRow(packed, width, unpacked, outputOffset = 2)
        for (x in 0 until width) {
            assertEquals(values[x], unpacked[2 + x].toInt())
            assertEquals(values[x], Raw10Pixels.read(ByteBuffer.wrap(packed), packed.size, x, 0))
        }
    }

    @Test
    fun `разбор параметров shading - значения по умолчанию и ошибки`() {
        val options = ShadingOptions.fromArgs(ScenarioArgs(mapOf("mode" to "front30")))
        assertEquals(ShadingMode.FRONT_30, options.mode)
        assertEquals(16, options.framesPerPhase)
        assertEquals(15, options.settleFrames)
        assertEquals(0.40, options.targetCentreLevel, 0.0)
        assertEquals(6, options.phases.size)
        val chosen = ShadingOptions.fromArgs(ScenarioArgs(mapOf("mode" to "main169", "phases" to "base,ae_on", "frames" to "4")))
        assertEquals(listOf("base", "ae_on"), chosen.phases.map { it.wireName })
        assertEquals(4, chosen.framesPerPhase)
        assertThrows(IllegalArgumentException::class.java) { ShadingOptions.fromArgs(ScenarioArgs(mapOf("mode" to "nope"))) }
        assertThrows(IllegalArgumentException::class.java) { ShadingOptions.fromArgs(ScenarioArgs(mapOf("mode" to "main43", "phases" to "base,xyz"))) }
    }

    @Test
    fun `режимы повторяют продуктовую конфигурацию`() {
        assertEquals("2", ShadingMode.MAIN_4_3.cameraId)
        assertEquals(null, ShadingMode.MAIN_4_3.aeTargetFps)
        assertEquals(2, ShadingMode.MAIN_4_3.vendorKeys().size)
        assertEquals(60, ShadingMode.MAIN_16_9.aeTargetFps)
        assertEquals(0, ShadingMode.WIDE_4_3.vendorKeys().size)
        assertEquals(null, ShadingMode.FRONT_30.aeTargetFps)
        assertEquals(0, ShadingMode.FRONT_30.vendorKeys().size)
        assertEquals("2304x1728", "${ShadingMode.FRONT_30.bufferWidth}x${ShadingMode.FRONT_30.bufferHeight}")
    }
}
