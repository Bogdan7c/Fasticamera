package dev.fasticamera.probe

import dev.fasticamera.probe.camera.shading.ExposureControl
import dev.fasticamera.probe.camera.shading.ExposureSetting
import dev.fasticamera.probe.camera.shading.FocusCapabilities
import dev.fasticamera.probe.camera.shading.FocusControl
import dev.fasticamera.probe.camera.shading.PhasePlanner
import dev.fasticamera.probe.camera.shading.SensorLimits
import dev.fasticamera.probe.camera.shading.ShadingPhaseId
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ShadingPhasesTest {
    private val limits = SensorLimits(100_000, 1_000_000_000, 50, 1600)
    private val base = ExposureSetting(8_000_000, 200)

    @Test
    fun `iso_high делит выдержку и умножает ISO на 4`() {
        assertEquals(ExposureSetting(2_000_000, 800), PhasePlanner.isoHigh(base, limits))
    }

    @Test
    fun `iso_high при упоре ISO в максимум делит выдержку на фактический множитель`() {
        val capped = PhasePlanner.isoHigh(ExposureSetting(8_000_000, 800), limits)
        // 800 x4 = 3200 > 1600: ISO 1600 (x2), выдержка 8 мс / 2 = 4 мс — яркость сохранена.
        assertEquals(ExposureSetting(4_000_000, 1600), capped)
    }

    @Test
    fun `фазы отличаются ровно тем, чем должны`() {
        fun controls(phase: ShadingPhaseId) = PhasePlanner.controlsFor(phase, base, limits)
        assertTrue(controls(ShadingPhaseId.BASE).lensShadingMapRequested)
        assertFalse(controls(ShadingPhaseId.MAP_OFF).lensShadingMapRequested)
        assertEquals(controls(ShadingPhaseId.BASE).exposure, controls(ShadingPhaseId.MAP_OFF).exposure)
        assertEquals(ExposureControl.VendorAuto, controls(ShadingPhaseId.AE_ON).exposure)
        assertEquals(FocusControl.INFINITY, controls(ShadingPhaseId.FOCUS_INF).focus)
        assertEquals(FocusControl.NEAREST, controls(ShadingPhaseId.FOCUS_NEAR).focus)
        assertEquals(FocusControl.CONTINUOUS_PICTURE, controls(ShadingPhaseId.BASE).focus)
        assertEquals(ExposureControl.Manual(ExposureSetting(2_000_000, 800)), controls(ShadingPhaseId.ISO_HIGH).exposure)
    }

    @Test
    fun `на камере с фиксированным фокусом фазы фокуса пропускаются с причиной, остальные нет`() {
        val fixed = FocusCapabilities(minFocusDistanceDiopters = 0f, afModes = intArrayOf(0))
        assertNotNull(PhasePlanner.skipReason(ShadingPhaseId.FOCUS_INF, fixed))
        assertNotNull(PhasePlanner.skipReason(ShadingPhaseId.FOCUS_NEAR, fixed))
        assertNull(PhasePlanner.skipReason(ShadingPhaseId.BASE, fixed))
        assertNull(PhasePlanner.skipReason(ShadingPhaseId.ISO_HIGH, fixed))
    }

    @Test
    fun `камера с AF и ненулевым минимумом фокуса фазы фокуса не пропускает`() {
        val autofocus = FocusCapabilities(minFocusDistanceDiopters = 10f, afModes = intArrayOf(0, 1, 3))
        assertNull(PhasePlanner.skipReason(ShadingPhaseId.FOCUS_NEAR, autofocus))
        assertNotNull(PhasePlanner.skipReason(ShadingPhaseId.FOCUS_NEAR, FocusCapabilities(10f, intArrayOf(0))))
        assertNotNull(PhasePlanner.skipReason(ShadingPhaseId.FOCUS_NEAR, FocusCapabilities(null, intArrayOf(0, 1))))
    }
}
