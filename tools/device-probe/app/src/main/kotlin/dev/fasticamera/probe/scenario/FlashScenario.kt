package dev.fasticamera.probe.scenario

import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraMetadata
import android.util.Size
import dev.fasticamera.probe.camera.StreamExperimentConfig
import dev.fasticamera.probe.camera.VendorKeyValue
import dev.fasticamera.probe.camera.dual.GreenLayout
import dev.fasticamera.probe.camera.flash.FlashFrameStore
import dev.fasticamera.probe.camera.flash.FlashPhases
import dev.fasticamera.probe.camera.flash.FlashSettings
import dev.fasticamera.probe.camera.flash.RawBandSettings
import dev.fasticamera.probe.camera.threea.ThreeARecorder
import dev.fasticamera.probe.camera.threea.ThreeASession
import dev.fasticamera.probe.camera.threea.ThreeASessionConfig
import org.json.JSONArray
import org.json.JSONObject

/**
 * Сценарий `flash`: освещает ли вспышка RAW-кадр, если сверху на повторяющийся запрос (AE=OFF, ручная выдержка/ISO)
 * положить одиночный `capture()` с FLASH_MODE=SINGLE. Сессия — продуктовая, как в `threea`.
 * Фазы: baseline, single, single_intent, torch (см. [FlashPhases]).
 * Камера 2: вендорские ключи 4:3@60; другие камеры: AE target fps 60 (ключи CamX на них ломают HAL).
 */
class FlashScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val args = context.args
        val cameraId = args.string("camera") ?: DEFAULT_CAMERA
        val characteristics = context.cameraManager.getCameraCharacteristics(cameraId)
        val greens = greenLayoutOf(characteristics)
        context.log.event("flash_characteristics", JSONObject()
            .put("flash_info_available", characteristics.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) ?: JSONObject.NULL)
            .put("torch_strength_default", characteristics.get(CameraCharacteristics.FLASH_TORCH_STRENGTH_DEFAULT_LEVEL) ?: JSONObject.NULL)
            .put("torch_strength_max", characteristics.get(CameraCharacteristics.FLASH_TORCH_STRENGTH_MAX_LEVEL) ?: JSONObject.NULL)
            .put("single_strength_default", characteristics.get(CameraCharacteristics.FLASH_SINGLE_STRENGTH_DEFAULT_LEVEL) ?: JSONObject.NULL)
            .put("single_strength_max", characteristics.get(CameraCharacteristics.FLASH_SINGLE_STRENGTH_MAX_LEVEL) ?: JSONObject.NULL)
            .put("cfa", characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT) ?: JSONObject.NULL)
            .put("exposure_range", characteristics.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)?.toString() ?: JSONObject.NULL))

        val isMainCamera = cameraId == DEFAULT_CAMERA
        val sessionConfig = ThreeASessionConfig(
            cameraId = cameraId,
            size = if (isMainCamera) Size(args.int("width", DEFAULT_WIDTH), args.int("height", DEFAULT_HEIGHT))
            else Size(args.int("width", WIDE_WIDTH), args.int("height", WIDE_HEIGHT)),
            format = args.intAnyRadix("format", FORMAT_RAW10),
            readerUsage = args.longAnyRadix("usage", USAGE_GPU_SAMPLED_IMAGE),
            maxImages = args.int("max_images", DEFAULT_MAX_IMAGES),
            perRequestVendorKeys = VendorKeyValue.parseList(
                args.string("req_keys")?.takeIf { it != "-" } ?: DEFAULT_60FPS_KEYS.takeIf { isMainCamera && args.string("req_keys") == null },
            ),
            aeTargetFps = args.intOrNull("ae_fps") ?: DEFAULT_AE_FPS.takeIf { !isMainCamera },
            sessionVendorKeys = VendorKeyValue.parseList(args.string("sess_keys")),
            openTimeoutMs = args.long("open_timeout_ms", DEFAULT_OPEN_TIMEOUT_MS),
            // Компаньон YUV объявлен в сессии, но ни один запрос его не целит: так держит 3A у продукта.
            companionYuvSize = StreamExperimentConfig.parseSize(args.string("yuv") ?: DEFAULT_COMPANION_YUV),
            companionTargeted = args.string("companion_targeted") == "1",
        )
        context.log.event("session_config", JSONObject().put("camera", cameraId).put("size", sessionConfig.size.toString())
            .put("vendor_keys", JSONArray(sessionConfig.perRequestVendorKeys.map { it.describe() }))
            .put("ae_target_fps", sessionConfig.aeTargetFps ?: JSONObject.NULL))

        val bandSettings = RawBandSettings(
            greens = greens,
            blackLevel = args.int("black", DEFAULT_BLACK_LEVEL),
            bandCount = args.int("bands", DEFAULT_BAND_COUNT),
            stepX = args.int("step_x", DEFAULT_STEP_X),
            stepY = args.int("step_y", DEFAULT_STEP_Y),
            clipLevel = args.int("clip_level", DEFAULT_CLIP_LEVEL),
        )
        val store = FlashFrameStore(context.log, bandSettings)
        val recorder = ThreeARecorder(context.log, rawContentProbe = null, frameObserver = store)
        val opened = ThreeASession.open(context.newSessionRunner(), sessionConfig, recorder, context.imageHandler)
        if (opened !is ThreeASession.OpenResult.Ready) {
            opened as ThreeASession.OpenResult.Failed
            context.log.error("open_${opened.stage}", opened.details)
            return
        }
        val session = opened.session
        val phases = FlashPhases(session, store, context.log, settingsFrom(args))
        try {
            phases.runAll(only = args.stringList("only", emptyList()).takeIf { it.isNotEmpty() }?.toSet())
        } finally {
            // Вспышка и torch обязаны быть выключены на любом пути выхода.
            try {
                phases.forceFlashOff()
            } catch (error: Exception) {
                context.log.error("flash_off_on_exit", error)
            }
            context.log.result("all_timestamps", JSONObject().put("sensor_timestamps_ns", JSONArray(store.allFrames().map { it.result.sensorTimestampNs }))
                .put("frame_numbers", JSONArray(store.allFrames().map { it.result.frameNumber })))
            session.close()?.let { context.log.error("close", it) }
        }
    }

    private fun settingsFrom(args: ScenarioArgs): FlashSettings {
        val singleExposures = args.stringList("exposures", DEFAULT_SINGLE_EXPOSURES).map { it.toLong() }
        return FlashSettings(
            iso = args.int("iso", DEFAULT_ISO),
            baselineExposureNs = args.long("exposure_ns", DEFAULT_EXPOSURE_NS),
            singleExposuresNs = singleExposures,
            torchExposuresNs = args.stringList("torch_exposures", DEFAULT_TORCH_EXPOSURES).map { it.toLong() },
            frameDurationNs = args.long("frame_duration_ns", DEFAULT_FRAME_DURATION_NS),
            baselineFrames = args.int("baseline_frames", DEFAULT_BASELINE_FRAMES),
            settleFrames = args.int("settle_frames", DEFAULT_SETTLE_FRAMES),
            shotsPerExposure = args.int("shots", DEFAULT_SHOTS),
            intentShots = args.int("intent_shots", DEFAULT_INTENT_SHOTS),
            shotGapMs = args.long("gap_ms", DEFAULT_SHOT_GAP_MS),
            framesBeforeShot = args.int("pre", DEFAULT_FRAMES_BEFORE),
            framesAfterShot = args.int("post", DEFAULT_FRAMES_AFTER),
            torchOnFrames = args.int("torch_on_frames", DEFAULT_TORCH_ON_FRAMES),
            torchOffFrames = args.int("torch_off_frames", DEFAULT_TORCH_OFF_FRAMES),
            intentExposureNs = args.long("intent_exposure_ns", DEFAULT_EXPOSURE_NS),
            preflashExposureNs = args.long("preflash_ns", 4_000_000L),
            preflashLitFrames = args.int("torch_lit_frames", 2),
            offGapsMs = args.stringList("off_gaps", listOf("50", "100", "200")).map { it.toLong() },
        )
    }

    /** Позиции двух зелёных в ячейке 2x2 по CFA датчика (индексы чтения: строка*2 + столбец). */
    private fun greenLayoutOf(characteristics: CameraCharacteristics): GreenLayout =
        when (val arrangement = characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT)) {
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_RGGB,
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_BGGR -> GreenLayout(1, 2)
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GRBG,
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GBRG -> GreenLayout(0, 3)
            else -> throw IllegalStateException("неподдерживаемая раскладка CFA: $arrangement")
        }

    private companion object {
        const val DEFAULT_CAMERA = "2"
        const val DEFAULT_WIDTH = 4096
        const val DEFAULT_HEIGHT = 3072

        /** RAW ширика (камера 3): 4000x3000. */
        const val WIDE_WIDTH = 4000
        const val WIDE_HEIGHT = 3000
        const val FORMAT_RAW10 = 0x25
        const val USAGE_GPU_SAMPLED_IMAGE = 0x100L
        const val DEFAULT_MAX_IMAGES = 6
        const val DEFAULT_OPEN_TIMEOUT_MS = 5_000L
        const val DEFAULT_AE_FPS = 60
        const val DEFAULT_COMPANION_YUV = "640x480"
        const val DEFAULT_BLACK_LEVEL = 64
        const val DEFAULT_BAND_COUNT = 16
        const val DEFAULT_STEP_X = 32
        const val DEFAULT_STEP_Y = 16
        const val DEFAULT_CLIP_LEVEL = 1020
        const val DEFAULT_ISO = 800
        const val DEFAULT_EXPOSURE_NS = 16_000_000L
        const val DEFAULT_FRAME_DURATION_NS = 16_666_667L
        const val DEFAULT_BASELINE_FRAMES = 60
        const val DEFAULT_SETTLE_FRAMES = 40
        const val DEFAULT_SHOTS = 3
        const val DEFAULT_INTENT_SHOTS = 2
        const val DEFAULT_SHOT_GAP_MS = 2_500L
        const val DEFAULT_FRAMES_BEFORE = 10
        const val DEFAULT_FRAMES_AFTER = 15
        const val DEFAULT_TORCH_ON_FRAMES = 45
        const val DEFAULT_TORCH_OFF_FRAMES = 30
        val DEFAULT_SINGLE_EXPOSURES = listOf("16000000", "8000000", "4000000", "1000000")
        val DEFAULT_TORCH_EXPOSURES = listOf("16000000", "4000000")

        /** Режим 4:3@60 из сессии 00: два покадровых вендорских ключа CamX. */
        const val DEFAULT_60FPS_KEYS =
            "org.quic.camera.manualExposure.disableFPSLimits:byte=1;" +
                "org.quic.camera.manualExposure.overrideSensorFrameDuration:long=16666667"
    }
}
