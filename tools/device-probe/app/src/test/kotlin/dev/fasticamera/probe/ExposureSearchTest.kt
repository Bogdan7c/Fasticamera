package dev.fasticamera.probe

import dev.fasticamera.probe.camera.shading.ExposureSearch
import dev.fasticamera.probe.camera.shading.ExposureSetting
import dev.fasticamera.probe.camera.shading.FrameLevels
import dev.fasticamera.probe.camera.shading.SensorLimits
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ExposureSearchTest {
    private val limits = SensorLimits(minExposureNs = 100_000, maxExposureNs = 1_000_000_000, minIso = 50, maxIso = 6400)
    private val capNs = 15_666_667L

    @Test
    fun `яркость масштабируется отношением цели к измеренному центру при фиксированном ISO`() {
        val next = ExposureSearch.nextSetting(ExposureSetting(5_000_000, 100), FrameLevels(0.2, 0.3), 0.4, limits, capNs)
        // Произведение x2: растёт выдержка (до потолка), ISO сохраняется.
        assertEquals(10_000_000L, next.exposureNs)
        assertEquals(100, next.iso)
    }

    @Test
    fun `при упоре выдержки в потолок кадра недостающее уходит в ISO`() {
        val next = ExposureSearch.nextSetting(ExposureSetting(10_000_000, 100), FrameLevels(0.1, 0.2), 0.4, limits, capNs)
        // Произведение 10 мс x 100 x4 = 4e9: выдержка = потолок, ISO = 4e9 / 15 666 667 ≈ 255.
        assertEquals(capNs, next.exposureNs)
        assertEquals(255, next.iso)
    }

    @Test
    fun `яркий блок ограничивает рост яркости, чтобы не было клиппинга`() {
        val factor = ExposureSearch.brightnessFactor(FrameLevels(centreGreen = 0.2, brightestBlock = 0.8), 0.4)
        assertEquals(0.85 / 0.8, factor, 1e-9)
    }

    @Test
    fun `слишком яркий кадр уменьшает яркость`() {
        val next = ExposureSearch.nextSetting(ExposureSetting(8_000_000, 200), FrameLevels(0.8, 0.95), 0.4, limits, capNs)
        // 0.4/0.8 = 0.5 по центру, 0.85/0.95 ≈ 0.895 по клиппингу: берём меньший, 0.5; ISO сохраняется.
        assertEquals(4_000_000L, next.exposureNs)
        assertEquals(200, next.iso)
    }

    @Test
    fun `ISO упирается в максимум сенсора`() {
        val next = ExposureSearch.settingForProduct(1e13, preferredIso = 100, limits, capNs)
        assertEquals(capNs, next.exposureNs)
        assertEquals(6400, next.iso)
    }

    @Test
    fun `сходимость требует центр в допуске и яркий блок ниже клиппинга`() {
        assertTrue(ExposureSearch.isConverged(FrameLevels(0.42, 0.7), 0.4))
        assertFalse(ExposureSearch.isConverged(FrameLevels(0.5, 0.7), 0.4))
        assertFalse(ExposureSearch.isConverged(FrameLevels(0.4, 0.92), 0.4))
    }

    @Test
    fun `почти чёрный кадр поднимает яркость максимальным шагом, а не делит на ноль`() {
        assertEquals(16.0, ExposureSearch.brightnessFactor(FrameLevels(0.0, 0.0), 0.4), 0.0)
    }

    @Test
    fun `стартовая экспозиция ограничивается потолком режима`() {
        val constrained = ExposureSearch.constrain(ExposureSetting(33_000_000, 100), limits, capNs)
        assertEquals(capNs, constrained.exposureNs)
        assertEquals(211, constrained.iso)
    }
}
