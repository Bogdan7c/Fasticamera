package dev.fasticamera.probe

import dev.fasticamera.probe.camera.VendorKeyValue
import dev.fasticamera.probe.camera.VendorResultKey
import dev.fasticamera.probe.camera.VendorValueType
import dev.fasticamera.probe.scenario.ScenarioArgs
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class VendorKeySpecTest {

    @Test
    fun `список рычагов 60 кадров разбирается в точные типы HAL`() {
        val keys = VendorKeyValue.parseList(
            "org.quic.camera.manualExposure.disableFPSLimits:byte=1;" +
                "org.quic.camera.manualExposure.overrideSensorFrameDuration:long=16666667;" +
                "org.codeaurora.qcamera3.sessionParameters.dynamicFPSConfig:float=60",
        )
        assertEquals(3, keys.size)
        assertEquals(VendorValueType.BYTE, keys[0].type)
        assertEquals(1.toByte(), keys[0].value)
        assertEquals(16_666_667L, keys[1].value)
        assertEquals(60.0f, keys[2].value)
        assertEquals("org.quic.camera.manualExposure.disableFPSLimits", keys[0].name)
    }

    @Test
    fun `массив int задаётся через вертикальную черту и hex`() {
        val key = VendorKeyValue.parseOne("com.oplus.ultra.high.resolution.enable:int[]=1|0x10")
        assertArrayEquals(intArrayOf(1, 16), key.value as IntArray)
        assertEquals("com.oplus.ultra.high.resolution.enable:int[]=1|16", key.describe())
    }

    @Test
    fun `неверный тип или значение — понятная ошибка, а не молчаливый ноль`() {
        assertThrows(IllegalArgumentException::class.java) { VendorKeyValue.parseOne("a.b:short=1") }
        assertThrows(IllegalArgumentException::class.java) { VendorKeyValue.parseOne("a.b:int=sixty") }
        assertThrows(IllegalArgumentException::class.java) { VendorKeyValue.parseOne("a.b=1") }
        assertThrows(IllegalArgumentException::class.java) { VendorKeyValue.parseOne("a.b:byte=999") }
    }

    @Test
    fun `пустая строка — пустой список`() {
        assertTrue(VendorKeyValue.parseList(null).isEmpty())
        assertTrue(VendorKeyValue.parseList("  ").isEmpty())
    }

    @Test
    fun `ключи результата разбираются с типом`() {
        val keys = VendorResultKey.parseList("com.oplus.actualCurFps:int;com.qti.stats_control.CurFPS:float")
        assertEquals(VendorResultKey("com.oplus.actualCurFps", VendorValueType.INT32), keys[0])
        assertEquals(VendorValueType.FLOAT, keys[1].type)
    }

    @Test
    fun `режимы сессии QTI в hex сохраняют все биты`() {
        assertEquals(0x8021, ScenarioArgs.parseIntAnyRadix("0x8021"))
        assertEquals(0x4000830B, ScenarioArgs.parseIntAnyRadix("0x4000830B"))
        assertEquals(0xF0000001.toInt(), ScenarioArgs.parseIntAnyRadix("0xF0000001"))
        assertEquals(32801, ScenarioArgs.parseIntAnyRadix("32801"))
    }

    @Test
    fun `аргументы сценария сообщают имя сломанного параметра`() {
        val args = ScenarioArgs(mapOf("frames" to "много"))
        val error = assertThrows(IllegalArgumentException::class.java) { args.int("frames", 600) }
        assertTrue(error.message!!.contains("frames"))
        assertEquals(600, ScenarioArgs(emptyMap()).int("frames", 600))
        assertEquals(listOf(32, 37), ScenarioArgs(mapOf("formats" to "0x20, 37")).intList("formats", emptyList()))
    }
}
