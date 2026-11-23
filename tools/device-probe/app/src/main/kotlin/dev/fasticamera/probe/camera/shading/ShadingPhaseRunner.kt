package dev.fasticamera.probe.camera.shading

import dev.fasticamera.probe.report.ProbeLog
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Снимает фазы замера и пишет по каждой: `<mode>_<phase>.blocks.f32`, для base — два полных кадра RAW16,
 * строку-итог в журнал прогона и запись в отчёт.
 */
class ShadingPhaseRunner(
    private val mode: ShadingMode,
    private val framesPerPhase: Int,
    private val facts: ShadingSensorFacts,
    private val session: ShadingCaptureSession,
    private val outputDir: File,
    private val log: ProbeLog,
) {
    /** Снимает все [phases]; фаза, недоступная на этой камере, пропускается с записанной причиной. */
    fun runAll(phases: List<ShadingPhaseId>, base: ExposureSetting): JSONArray {
        val report = JSONArray()
        for (phase in phases) {
            val skipReason = PhasePlanner.skipReason(phase, facts.focus)
            if (skipReason != null) {
                log.event("phase_skipped", JSONObject().put("phase", phase.wireName).put("reason", skipReason))
                report.put(JSONObject().put("phase", phase.wireName).put("skipped_reason", skipReason))
                continue
            }
            report.put(runPhase(phase, PhasePlanner.controlsFor(phase, base, facts.limits)))
        }
        return report
    }

    private fun runPhase(phase: ShadingPhaseId, controls: PhaseControls): JSONObject {
        log.event("phase_start", JSONObject().put("phase", phase.wireName))
        val rawFiles = ArrayList<String>()
        val consumer = if (phase == ShadingPhaseId.BASE) rawFrameWriter(rawFiles) else null
        val collection = session.capture(controls, framesPerPhase, consumer)
        val phaseReport = JSONObject()
            .put("phase", phase.wireName)
            .put("controls", describe(controls))
            .put("requested_frames", collection.wantedFrames)
            .put("collected_frames", collection.collectedFrames)
            .put("counters", collection.counters.toJson())
            .put("frames", JSONArray(collection.frameRecords))
        val means = collection.means
        val grid = collection.grid
        if (means == null || grid == null) {
            log.error("phase_${phase.wireName}", "не собран ни один кадр: ${collection.counters.toJson()}")
            return phaseReport.put("error", "не собран ни один кадр")
        }
        val blocksFile = "${mode.wireName}_${phase.wireName}.blocks.f32"
        ShadingFiles.writeFloat32LittleEndian(File(outputDir, blocksFile), means)
        phaseReport.put("grid", JSONObject().put("blocks_x", grid.blocksX).put("blocks_y", grid.blocksY).put("block_size", BLOCK_SIZE_PIXELS))
            .put("valid_rows", collection.validRows)
            .put("files", JSONArray(listOf(blocksFile) + rawFiles))
        val summary = summarize(phase, means, grid, collection)
        phaseReport.put("summary", summary)
        log.result("shading_phase", summary)
        return phaseReport
    }

    /** Строка-итог: отношение угол/центр по зелёному и по каждой плоскости (грубая оценка виньетирования до разбора на ПК). */
    private fun summarize(phase: ShadingPhaseId, means: FloatArray, grid: BlockGrid, collection: PhaseCollection): JSONObject {
        val byPlane = JSONArray()
        for (plane in 0 until CFA_PLANE_COUNT) {
            byPlane.put(BlockStatistics.cornerCentreRatio(means, grid, intArrayOf(plane), facts.blackByPlane) ?: JSONObject.NULL)
        }
        return JSONObject()
            .put("mode", mode.wireName)
            .put("phase", phase.wireName)
            .put("collected_frames", collection.collectedFrames)
            .put("valid_rows", collection.validRows)
            .put("corner_centre_ratio_green", BlockStatistics.cornerCentreRatio(means, grid, facts.greenPlanes, facts.blackByPlane) ?: JSONObject.NULL)
            .put("corner_centre_ratio_by_plane", byPlane)
    }

    /** Пишет два первых кадра фазы base полностью: на них на ПК считается шум. */
    private fun rawFrameWriter(writtenNames: MutableList<String>) = DenseFrameConsumer { frameIndex, pixels, width, validRows ->
        if (frameIndex < RAW_FRAMES_KEPT) {
            val name = "${mode.wireName}_base.frame$frameIndex.raw16"
            ShadingFiles.writeDenseRaw16(File(outputDir, name), pixels, width, validRows)
            writtenNames += name
        }
    }

    private fun describe(controls: PhaseControls): JSONObject {
        val exposure = JSONObject()
        when (val control = controls.exposure) {
            ExposureControl.VendorAuto -> exposure.put("mode", "vendor_ae")
            is ExposureControl.Manual -> exposure.put("mode", "manual")
                .put("exposure_ns", control.setting.exposureNs).put("iso", control.setting.iso)
        }
        return JSONObject().put("exposure", exposure)
            .put("lens_shading_map_requested", controls.lensShadingMapRequested)
            .put("focus", controls.focus.name)
    }

    private companion object {
        const val RAW_FRAMES_KEPT = 2
    }
}
