package dev.fasticamera.probe.camera

import android.hardware.camera2.CameraDevice
import android.hardware.camera2.params.SessionConfiguration
import android.util.Size
import dev.fasticamera.probe.scenario.ScenarioArgs
import org.json.JSONArray
import org.json.JSONObject

/**
 * Шаблон запроса Camera2. Отдельный enum вместо голого Int: в команде видно намерение.
 */
enum class RequestTemplate(val wireName: String, val cameraTemplate: Int) {
    PREVIEW("preview", CameraDevice.TEMPLATE_PREVIEW),
    RECORD("record", CameraDevice.TEMPLATE_RECORD),
    STILL("still", CameraDevice.TEMPLATE_STILL_CAPTURE),
    MANUAL("manual", CameraDevice.TEMPLATE_MANUAL),
    ;

    companion object {
        fun fromWire(raw: String): RequestTemplate = entries.firstOrNull { it.wireName == raw }
            ?: throw IllegalArgumentException("неизвестный шаблон '$raw'")
    }
}

/**
 * Ручная экспозиция: выдержка, ISO и длительность кадра задаются вместе,
 * потому что по отдельности они не имеют смысла (AE выключен целиком).
 */
data class ManualExposure(
    val exposureTimeNs: Long,
    val sensitivityIso: Int,
    val frameDurationNs: Long,
)

/**
 * Полное описание одного эксперимента с потоком (C2, C3, C8).
 * Все параметры пришли из команды; значения по умолчанию — здесь, явно.
 */
data class StreamExperimentConfig(
    val cameraId: String,
    val physicalCameraId: String?,
    val format: Int,
    val size: Size,
    val maxImages: Int,
    val readerUsage: Long?,
    val companionYuvSize: Size?,
    val frameTarget: Int,
    val timeoutMs: Long,
    val openTimeoutMs: Long,
    val template: RequestTemplate,
    val sessionType: Int,
    val aeTargetFps: Int?,
    val manualExposure: ManualExposure?,
    val requestKeys: List<VendorKeyValue>,
    val sessionKeys: List<VendorKeyValue>,
    val resultKeys: List<VendorResultKey>,
    /** Анализировать содержимое каждого N-го кадра (0 — выключено, чтобы не мешать замеру темпа). */
    val contentEveryNth: Int,
    val contentBandCount: Int,
    val contentBlockSize: Int,
) {
    val isHighSpeedSession: Boolean get() = sessionType == SessionConfiguration.SESSION_HIGH_SPEED

    fun toJson(): JSONObject = JSONObject()
        .put("camera_id", cameraId)
        .put("physical_camera_id", physicalCameraId ?: JSONObject.NULL)
        .put("format", format)
        .put("width", size.width)
        .put("height", size.height)
        .put("max_images", maxImages)
        .put("reader_usage", readerUsage?.let { "0x" + it.toString(16) } ?: JSONObject.NULL)
        .put("companion_yuv", companionYuvSize?.toString() ?: JSONObject.NULL)
        .put("frame_target", frameTarget)
        .put("timeout_ms", timeoutMs)
        .put("template", template.wireName)
        .put("session_type", "0x" + sessionType.toString(16))
        .put("ae_target_fps", aeTargetFps ?: JSONObject.NULL)
        .put(
            "manual_exposure",
            manualExposure?.let {
                JSONObject().put("exposure_ns", it.exposureTimeNs).put("iso", it.sensitivityIso)
                    .put("frame_duration_ns", it.frameDurationNs)
            } ?: JSONObject.NULL,
        )
        .put("request_keys", JSONArray(requestKeys.map { it.describe() }))
        .put("session_keys", JSONArray(sessionKeys.map { it.describe() }))
        .put("content_every_nth", contentEveryNth)
        .put("result_keys", JSONArray(resultKeys.map { "${it.name}:${it.type.wireName}" }))

    companion object {
        private const val DEFAULT_FRAME_TARGET = 600
        private const val DEFAULT_TIMEOUT_MS = 30_000L
        private const val DEFAULT_OPEN_TIMEOUT_MS = 5_000L
        private const val DEFAULT_MAX_IMAGES = 6
        private const val DEFAULT_ISO = 400

        /** 24 полосы по высоте: для 3072 строк это по 128 строк — поля 16:9 (по 384 строки) видны сразу. */
        private const val DEFAULT_CONTENT_BANDS = 24
        private const val DEFAULT_CONTENT_BLOCK = 128

        /** RAW_SENSOR (0x20) по умолчанию: это формат DNG и главный кандидат для видоискателя. */
        private const val DEFAULT_FORMAT_RAW_SENSOR = 0x20

        /**
         * Ключи результата, которые читаем всегда: текущий режим сенсора и вендорские оценки к/с.
         * Типы взяты из дампа vendor tags (0x800d000a current_mode int32 и т.д.).
         */
        val DEFAULT_RESULT_KEYS: List<VendorResultKey> = listOf(
            VendorResultKey("org.codeaurora.qcamera3.sensor_meta_data.current_mode", VendorValueType.INT32),
            VendorResultKey("org.codeaurora.qcamera3.sensor_meta_data.targetFPS", VendorValueType.INT32),
            VendorResultKey("org.codeaurora.qcamera3.sensor_meta_data.sensorDynamicFPS", VendorValueType.FLOAT),
            VendorResultKey("com.qti.stats_control.CurFPS", VendorValueType.FLOAT),
        )

        fun fromArgs(args: ScenarioArgs): StreamExperimentConfig {
            val exposureNs = args.longOrNull("exposure_ns")
            val frameDurationNs = args.longOrNull("frame_duration_ns")
            val manual = when {
                exposureNs != null -> ManualExposure(
                    exposureTimeNs = exposureNs,
                    sensitivityIso = args.int("iso", DEFAULT_ISO),
                    // Без явной длительности кадра берём выдержку: HAL сам поднимет до минимума режима.
                    frameDurationNs = frameDurationNs ?: exposureNs,
                )
                frameDurationNs != null -> throw IllegalArgumentException(
                    "frame_duration_ns без exposure_ns не имеет смысла: при включённом AE HAL его игнорирует",
                )
                else -> null
            }
            return StreamExperimentConfig(
                cameraId = args.requireString("camera"),
                physicalCameraId = args.string("physical"),
                format = args.intAnyRadix("format", DEFAULT_FORMAT_RAW_SENSOR),
                size = parseSize(args.requireString("size")),
                maxImages = args.int("max_images", DEFAULT_MAX_IMAGES),
                readerUsage = args.string("usage")?.let(ScenarioArgs::parseLongAnyRadix),
                companionYuvSize = args.string("companion_yuv")?.let(::parseSize),
                frameTarget = args.int("frames", DEFAULT_FRAME_TARGET),
                timeoutMs = args.long("timeout_ms", DEFAULT_TIMEOUT_MS),
                openTimeoutMs = args.long("open_timeout_ms", DEFAULT_OPEN_TIMEOUT_MS),
                template = RequestTemplate.fromWire(args.string("template") ?: RequestTemplate.PREVIEW.wireName),
                sessionType = args.intAnyRadix("session_type", SessionConfiguration.SESSION_REGULAR),
                aeTargetFps = args.intOrNull("ae_fps"),
                manualExposure = manual,
                requestKeys = VendorKeyValue.parseList(args.string("req_keys")),
                sessionKeys = VendorKeyValue.parseList(args.string("sess_keys")),
                resultKeys = DEFAULT_RESULT_KEYS + VendorResultKey.parseList(args.string("result_keys")),
                contentEveryNth = args.int("content_every", 0),
                contentBandCount = args.int("content_bands", DEFAULT_CONTENT_BANDS),
                contentBlockSize = args.int("content_block", DEFAULT_CONTENT_BLOCK),
            )
        }

        /** `4096x2304` → Size. */
        fun parseSize(raw: String): Size {
            val parts = raw.lowercase().split('x')
            require(parts.size == 2) { "размер ожидается как ШxВ, получено '$raw'" }
            val width = parts[0].toIntOrNull()
            val height = parts[1].toIntOrNull()
            require(width != null && height != null && width > 0 && height > 0) { "неверный размер '$raw'" }
            return Size(width, height)
        }
    }
}
