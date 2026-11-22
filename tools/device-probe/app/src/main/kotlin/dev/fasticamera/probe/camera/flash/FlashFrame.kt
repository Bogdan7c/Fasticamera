package dev.fasticamera.probe.camera.flash

import dev.fasticamera.probe.camera.threea.ThreeATag
import org.json.JSONArray
import org.json.JSONObject

/** Поля результата кадра, важные для разведки вспышки. null = ключ не пришёл в результате. */
class FlashResultInfo(
    val tag: ThreeATag,
    val frameNumber: Long,
    val sensorTimestampNs: Long,
    val exposureNs: Long?,
    val iso: Int?,
    /** CaptureResult.FLASH_STATE: 0 UNAVAILABLE, 1 CHARGING, 2 READY, 3 FIRED, 4 PARTIAL. */
    val flashState: Int?,
    /** Эхо FLASH_MODE: 0 OFF, 1 SINGLE, 2 TORCH. */
    val flashMode: Int?,
    /** Вендорский `current_mode` сенсора, как его вернул HAL (число или текст "<absent>"). */
    val sensorMode: Any?,
    val captureIntent: Int?,
    /** CaptureResult.FLASH_STRENGTH_LEVEL (API 35), если HAL его отдаёт. */
    val flashStrength: Int? = null,
    /** elapsedRealtimeNanos прихода результата в приложение. */
    val arrivedNs: Long,
)

/** Кадр: результат плюс (если RAW успел прийти) профиль яркости. */
class FlashFrame(val result: FlashResultInfo, val profile: RawBandProfile?) {

    /** Тип запроса для отчёта: repeating или номер одиночного запроса (метка). */
    fun requestLabel(): String = when (result.tag.kind) {
        ThreeATag.Kind.TRIGGER -> "oneshot:${result.tag.phase}#${result.tag.round}"
        else -> "repeating:${result.tag.phase}"
    }

    fun toJson(relativeTo: Long? = null): JSONObject = JSONObject()
        .put("frame_number", result.frameNumber)
        .put("offset", relativeTo?.let { result.frameNumber - it } ?: JSONObject.NULL)
        .put("sensor_timestamp", result.sensorTimestampNs)
        .put("request", requestLabel())
        .put("exposure_ns", result.exposureNs ?: JSONObject.NULL)
        .put("iso", result.iso ?: JSONObject.NULL)
        .put("flash_state", result.flashState ?: JSONObject.NULL)
        .put("flash_mode", result.flashMode ?: JSONObject.NULL)
        .put("sensor_mode", result.sensorMode ?: JSONObject.NULL)
        .put("capture_intent", result.captureIntent ?: JSONObject.NULL)
        .put("flash_strength", result.flashStrength ?: JSONObject.NULL)
        .put("mean", profile?.overallMean?.let(::round1) ?: JSONObject.NULL)
        .put("clip_fraction", profile?.clipFraction?.let { Math.round(it * 10000) / 10000.0 } ?: JSONObject.NULL)
        .put("bands", profile?.let { JSONArray(it.bandMeans.map(::round1)) } ?: JSONObject.NULL)

    private fun round1(value: Double): Double = Math.round(value * 10.0) / 10.0
}
