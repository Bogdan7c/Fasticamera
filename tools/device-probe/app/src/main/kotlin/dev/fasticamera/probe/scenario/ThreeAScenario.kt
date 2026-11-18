package dev.fasticamera.probe.scenario

import android.hardware.camera2.CameraCharacteristics
import android.util.Size
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.StreamExperimentConfig
import dev.fasticamera.probe.camera.VendorKeyValue
import dev.fasticamera.probe.camera.threea.ThreeAPhaseSettings
import dev.fasticamera.probe.camera.threea.ThreeAPhases
import dev.fasticamera.probe.camera.threea.ThreeARaw10Probe
import dev.fasticamera.probe.camera.threea.ThreeARecorder
import dev.fasticamera.probe.camera.threea.ThreeASession
import dev.fasticamera.probe.camera.threea.ThreeASessionConfig
import org.json.JSONArray
import org.json.JSONObject

/**
 * Сценарий `threea`: разведка 3A (AF/AE/AWB, ручная экспозиция, вендорский приоритет ISO, пачки)
 * в ТОЙ сессии, которая будет у продукта: Java Camera2, камера 2, RAW10 4096x3072, 4:3@60 через
 * вендорские покадровые ключи, без AE target fps, RAW-only.
 */
class ThreeAScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val args = context.args
        val cameraId = args.string("camera") ?: DEFAULT_CAMERA
        val characteristics = context.cameraManager.getCameraCharacteristics(cameraId)
        context.log.event("small_yuv_sizes", JSONObject().put("sizes", JSONArray(
            context.cameraManager.getCameraCharacteristics(cameraId).get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
                ?.getOutputSizes(android.graphics.ImageFormat.YUV_420_888)?.filter { it.width <= 1920 }?.map { it.toString() } ?: emptyList<String>())))
        context.log.event("characteristics", describeCharacteristics(characteristics))
        val activeArray = requireNotNull(characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)) {
            "нет SENSOR_INFO_ACTIVE_ARRAY_SIZE"
        }
        val sessionConfig = ThreeASessionConfig(
            cameraId = cameraId,
            size = Size(args.int("width", DEFAULT_WIDTH), args.int("height", DEFAULT_HEIGHT)),
            format = args.intAnyRadix("format", FORMAT_RAW10),
            readerUsage = args.longAnyRadix("usage", USAGE_GPU_SAMPLED_IMAGE),
            maxImages = args.int("max_images", DEFAULT_MAX_IMAGES),
            perRequestVendorKeys = VendorKeyValue.parseList(
                // "-" = явно без вендорских ключей; без ae_fps и без req_keys — режим 4:3@60 основной камеры.
                args.string("req_keys")?.takeIf { it != "-" } ?: DEFAULT_60FPS_KEYS.takeIf { args.string("req_keys") == null && args.intOrNull("ae_fps") == null },
            ) + VendorKeyValue.parseList(args.string("extra_req_keys")),
            aeTargetFps = args.intOrNull("ae_fps"),
            sessionVendorKeys = VendorKeyValue.parseList(args.string("sess_keys")),
            openTimeoutMs = args.long("open_timeout_ms", DEFAULT_OPEN_TIMEOUT_MS),
            companionYuvSize = args.string("yuv")?.let(StreamExperimentConfig::parseSize),
            companionFormat = if (args.string("companion_format") == "private") android.graphics.ImageFormat.PRIVATE else android.graphics.ImageFormat.YUV_420_888,
            companionTargeted = args.string("companion_targeted") != "0",
        )
        context.log.event("session_config", JSONObject().put("camera", cameraId).put("size", sessionConfig.size.toString())
            .put("format", sessionConfig.format).put("usage", sessionConfig.readerUsage)
            .put("companion_yuv", sessionConfig.companionYuvSize?.toString() ?: JSONObject.NULL).put("vendor_keys", JSONArray(sessionConfig.perRequestVendorKeys.map { it.describe() })))

        val contentEvery = args.int("content_every", 0)
        val rawProbe = contentEvery.takeIf { it > 0 }?.let { ThreeARaw10Probe(it, args.int("content_skip", 60), args.int("content_max", 20)) }
        val recorder = ThreeARecorder(context.log, rawProbe)
        val opened = ThreeASession.open(context.newSessionRunner(), sessionConfig, recorder, context.imageHandler)
        if (opened !is ThreeASession.OpenResult.Ready) {
            opened as ThreeASession.OpenResult.Failed
            context.log.error("open_${opened.stage}", opened.details)
            return
        }
        val session = opened.session
        try {
            val settings = ThreeAPhaseSettings(
                holdMs = args.long("hold_ms", DEFAULT_HOLD_MS),
                firstFrameGraceMs = args.long("grace_ms", DEFAULT_GRACE_MS),
                activeArray = activeArray,
                ageTestExposureNs = args.long("age_exposure_ns", 8_000_000L),
                frameDurationNs = args.long("frame_duration_ns", DEFAULT_FRAME_DURATION_NS),
            )
            val power = if (args.string("power") == "1") dev.fasticamera.probe.camera.threea.ThreeAPowerSampler(context.appContext, context.log, 1000L).also { it.start() } else null
            val phases = ThreeAPhases(session, recorder, context.log, settings, rawProbe)
            try {
                phases.runAll(only = args.stringList("only", emptyList()).takeIf { it.isNotEmpty() }?.toSet())
            } finally {
                power?.stop()
            }
            reportOverall(context, recorder, phases)
            context.log.result("age_stats", JSONObject()
                .put("image_age_ms", ageStats(recorder.imageAgesSnapshot()))
                .put("result_age_ms", ageStats(recorder.resultAgesSnapshot())))
            rawProbe?.let { context.log.result("raw10_content", it.toJson()) }
        } finally {
            session.close()?.let { context.log.error("close", it) }
        }
    }

    /** Итоги по всему прогону: связь результатов и RAW-буферов, отказы, отношения exposure×ISO к baseline. */
    private fun reportOverall(context: ScenarioContext, recorder: ThreeARecorder, phases: ThreeAPhases) {
        val rows = recorder.allRows()
        val withImage = rows.count { row -> row.sensorTimestampNs?.let(recorder::hasImageWithTimestamp) == true }
        val baseline = phases.summaries.firstOrNull { it.phase == "baseline" }?.medianExposureTimesIso
        val ratios = JSONObject()
        for (summary in phases.summaries) {
            val value = summary.medianExposureTimesIso
            ratios.put(summary.phase, if (value != null && baseline != null && baseline > 0) value / baseline else JSONObject.NULL)
        }
        val yuvStamps = recorder.companionTimestampsSnapshot()
        context.log.result("companion_stream", JSONObject().put("images", yuvStamps.size)
            .put("timing", dev.fasticamera.probe.camera.FrameTimingStats.compute(yuvStamps.sorted()).toJson()))
        context.log.result(
            "overall",
            JSONObject().put("raw_image_timing", dev.fasticamera.probe.camera.FrameTimingStats.compute(recorder.rawImageTimestampsSorted()).toJson())
                .put("results_total", rows.size).put("results_with_raw_image", withImage)
                .put("raw_images_total", recorder.imageCount).put("image_acquire_errors", recorder.imageAcquireErrorCount)
                .put("untagged_results", recorder.untaggedResultCount)
                .put("failures", JSONObject(recorder.failureCounts() as Map<*, *>))
                .put("exposure_x_iso_ratio_to_baseline", ratios),
        )
    }

    /** Статистика возраста в мс, первые 30 кадров пропущены (разгон потока). */
    private fun ageStats(agesNs: List<Long>): JSONObject {
        val settled = agesNs.drop(AGE_SKIP_FRAMES).map { it / NS_PER_MS }.sorted()
        if (settled.isEmpty()) return JSONObject().put("count", 0)
        return JSONObject().put("count", settled.size).put("median", settled[settled.size / 2])
            .put("min", settled.first()).put("max", settled.last()).put("p95", settled[settled.size * 95 / 100])
    }

    private fun describeCharacteristics(characteristics: CameraCharacteristics): JSONObject {
        val isoModes = characteristics.get(CameraCharacteristics.Key("$ISO_SECTION.iso_available_modes", IntArray::class.java))
        val exposureRange = characteristics.get(CameraCharacteristics.Key("$ISO_SECTION.exposure_time_range", LongArray::class.java))
        return JSONObject()
            .put("iso_available_modes", isoModes?.let { JSONArray(it.toList()) } ?: "<absent>")
            .put("exposure_time_range", exposureRange?.let { JSONArray(it.toList()) } ?: "<absent>")
            .put("active_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)?.toString() ?: "<absent>")
            .put("ae_compensation_step", characteristics.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_STEP)?.toString() ?: "<absent>")
            .put("ae_compensation_range", characteristics.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_RANGE)?.toString() ?: "<absent>")
            .put("af_modes", characteristics.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)?.let { JSONArray(it.toList()) } ?: "<absent>")
            .put("ae_antibanding_modes", characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_ANTIBANDING_MODES)?.let { JSONArray(it.toList()) } ?: "<absent>")
            .put("iso_range", characteristics.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)?.toString() ?: "<absent>")
            .put("exposure_range", characteristics.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)?.toString() ?: "<absent>")
            .put("min_focus_distance", characteristics.get(CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE) ?: "<absent>")
    }

    private companion object {
        const val AGE_SKIP_FRAMES = 30
        const val NS_PER_MS = 1_000_000.0
        const val DEFAULT_CAMERA = "2"
        const val DEFAULT_WIDTH = 4096
        const val DEFAULT_HEIGHT = 3072
        const val FORMAT_RAW10 = 0x25
        const val USAGE_GPU_SAMPLED_IMAGE = 0x100L
        const val DEFAULT_MAX_IMAGES = 6
        const val DEFAULT_OPEN_TIMEOUT_MS = 5_000L
        const val DEFAULT_HOLD_MS = 2_500L
        const val DEFAULT_GRACE_MS = 3_000L
        const val DEFAULT_FRAME_DURATION_NS = 16_666_667L
        const val ISO_SECTION = "org.codeaurora.qcamera3.iso_exp_priority"

        /** Режим 4:3@60 из сессии 00: два покадровых вендорских ключа CamX. */
        const val DEFAULT_60FPS_KEYS =
            "org.quic.camera.manualExposure.disableFPSLimits:byte=1;" +
                "org.quic.camera.manualExposure.overrideSensorFrameDuration:long=16666667"
    }
}
