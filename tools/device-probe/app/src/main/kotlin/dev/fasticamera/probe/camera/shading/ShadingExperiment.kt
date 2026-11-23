package dev.fasticamera.probe.camera.shading

import dev.fasticamera.probe.scenario.ScenarioContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Замер карты затенения объектива в продуктовой конфигурации одного режима камеры:
 * прогрев → подбор экспозиции → фазы → `<mode>.json` + бинарные файлы в `filesDir/captures/<run>/`.
 *
 * Отчёт пишется в `finally`: если замер упал посреди пути, уже собранное и причина сбоя остаются на диске.
 */
class ShadingExperiment(private val context: ScenarioContext, private val options: ShadingOptions) {
    private val mode = options.mode
    private val outputDir = File(context.appContext.filesDir, "captures/${context.runId}").apply { mkdirs() }
    private val report = JSONObject()

    fun run() {
        val characteristics = context.cameraManager.getCameraCharacteristics(mode.cameraId)
        val facts = ShadingSensorFacts.read(characteristics)
        report.put("schema", REPORT_SCHEMA)
            .put("run", context.runId)
            .put("mode", mode.wireName)
            .put("options", describe(options))
            .put("request_config", describeRequestConfig())
            .put("characteristics", ShadingJson.characteristics(characteristics))
        val rig = ShadingRig(mode, facts.focus, context.newSessionRunner(), context.cameraHandler, context.imageHandler, OPEN_TIMEOUT_MS)
        var session: ShadingCaptureSession? = null
        try {
            rig.start(characteristics)
            val collector = ShadingFrameCollector(rig.images, rig.results)
            session = ShadingCaptureSession(rig, collector, options.settleFrames)
            measureAllPhases(facts, session)
        } catch (error: Exception) {
            report.put("failure", "${error.javaClass.name}: ${error.message}")
            throw error
        } finally {
            report.put("buffer_geometry", session?.bufferGeometry?.let { JSONObject().put("width", it.width).put("height", it.height).put("row_stride_bytes", it.rowStrideBytes) })
            report.put("rig_counters", rig.countersJson())
            rig.close()
            File(outputDir, "${mode.wireName}.json").writeText(report.toString(2))
        }
    }

    private fun measureAllPhases(facts: ShadingSensorFacts, session: ShadingCaptureSession) {
        val search = ShadingExposureStage(mode, facts, session, options.targetCentreLevel).findBaseExposure()
        report.put("exposure_search", search.log)
        report.put("base_exposure", JSONObject().put("exposure_ns", search.setting.exposureNs).put("iso", search.setting.iso).put("converged", search.converged))
        context.log.result(
            "shading_exposure",
            JSONObject().put("mode", mode.wireName).put("exposure_ns", search.setting.exposureNs)
                .put("iso", search.setting.iso).put("converged", search.converged),
        )
        val runner = ShadingPhaseRunner(mode, options.framesPerPhase, facts, session, outputDir, context.log)
        report.put("phases", runner.runAll(options.phases, search.setting))
    }

    private fun describe(options: ShadingOptions) = JSONObject()
        .put("frames", options.framesPerPhase).put("settle", options.settleFrames)
        .put("target", options.targetCentreLevel).put("phases", JSONArray(options.phases.map { it.wireName }))

    private fun describeRequestConfig() = JSONObject()
        .put("camera", mode.cameraId)
        .put("buffer", "${mode.bufferWidth}x${mode.bufferHeight}")
        .put("ae_target_fps", mode.aeTargetFps ?: JSONObject.NULL)
        .put("vendor_keys", JSONArray(mode.vendorKeys().map { it.describe() }))
        .put("frame_duration_ns", mode.frameDurationNs)
        .put("exposure_cap_ns", mode.exposureCapNs)
        .put("companion_yuv", mode.companionLeg().label)

    private companion object {
        /** Маркер формата: ПК-анализ по нему находит отчёты shading среди других JSON в captures. */
        const val REPORT_SCHEMA = "shading_probe_v1"
        const val OPEN_TIMEOUT_MS = 5_000L
    }
}
