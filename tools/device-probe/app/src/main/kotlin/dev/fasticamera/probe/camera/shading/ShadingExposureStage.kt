package dev.fasticamera.probe.camera.shading

import org.json.JSONArray
import org.json.JSONObject

/** Итог подбора: финальная экспозиция и журнал итераций (в отчёт). */
class ExposureSearchOutcome(val setting: ExposureSetting, val log: JSONArray, val converged: Boolean)

/**
 * Прогрев вендорским AE и подбор ручной экспозиции под целевой уровень центра.
 * Не более [MAX_ADJUSTMENTS] корректировок; каждый замер — один кадр после кадров успокоения.
 */
class ShadingExposureStage(
    private val mode: ShadingMode,
    private val facts: ShadingSensorFacts,
    private val session: ShadingCaptureSession,
    private val targetCentreLevel: Double,
) {
    fun findBaseExposure(): ExposureSearchOutcome {
        var current = ExposureSearch.constrain(warmUpWithVendorAe(), facts.limits, mode.exposureCapNs)
        val log = JSONArray()
        var converged = false
        for (iteration in 0..MAX_ADJUSTMENTS) {
            val controls = PhaseControls(ExposureControl.Manual(current), lensShadingMapRequested = true, focus = FocusControl.CONTINUOUS_PICTURE)
            val levels = measure(session.capture(controls, frames = 1))
            converged = ExposureSearch.isConverged(levels, targetCentreLevel)
            log.put(iterationRecord(iteration, current, levels, converged))
            if (converged || iteration == MAX_ADJUSTMENTS) break
            current = ExposureSearch.nextSetting(current, levels, targetCentreLevel, facts.limits, mode.exposureCapNs)
        }
        return ExposureSearchOutcome(current, log, converged)
    }

    /** ~1,5 с вендорского AE, затем берём выдержку и ISO последнего кадра как стартовую точку. */
    private fun warmUpWithVendorAe(): ExposureSetting {
        val generation = session.apply(ShadingRig.WARMUP_CONTROLS)
        Thread.sleep(WARMUP_MS)
        val result = checkNotNull(session.latestResultOf(generation)) { "прогрев: HAL не вернул ни одного результата" }
        val exposureNs = checkNotNull(result.get(android.hardware.camera2.CaptureResult.SENSOR_EXPOSURE_TIME)) { "прогрев: нет SENSOR_EXPOSURE_TIME" }
        val iso = checkNotNull(result.get(android.hardware.camera2.CaptureResult.SENSOR_SENSITIVITY)) { "прогрев: нет SENSOR_SENSITIVITY" }
        return ExposureSetting(exposureNs, iso)
    }

    private fun measure(collection: PhaseCollection): FrameLevels {
        val means = checkNotNull(collection.means) { "подбор экспозиции: не собран ни один кадр (${collection.counters.toJson()})" }
        val grid = checkNotNull(collection.grid)
        return BlockStatistics.levels(means, grid, facts.greenPlanes, facts.blackByPlane, facts.whiteLevel)
    }

    private fun iterationRecord(iteration: Int, setting: ExposureSetting, levels: FrameLevels, converged: Boolean): JSONObject = JSONObject()
        .put("iteration", iteration)
        .put("exposure_ns", setting.exposureNs)
        .put("iso", setting.iso)
        .put("centre_green_level", levels.centreGreen)
        .put("brightest_block_level", levels.brightestBlock)
        .put("target_level", targetCentreLevel)
        .put("converged", converged)

    private companion object {
        const val MAX_ADJUSTMENTS = 3
        const val WARMUP_MS = 1_500L
    }
}
