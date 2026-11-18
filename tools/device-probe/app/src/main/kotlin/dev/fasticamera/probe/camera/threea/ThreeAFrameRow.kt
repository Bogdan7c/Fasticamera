package dev.fasticamera.probe.camera.threea

import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.MeteringRectangle
import dev.fasticamera.probe.camera.VendorResultKey
import org.json.JSONArray
import org.json.JSONObject

/**
 * Один результат кадра: числовые поля, нужные для сводок, плюс готовый JSON-ряд для отчёта.
 * Отсутствующее в результате значение остаётся null (а не 0): «нет данных» ≠ «ноль».
 */
class ThreeAFrameRow(
    val tag: ThreeATag,
    val frameNumber: Long,
    val sensorTimestampNs: Long?,
    val exposureNs: Long?,
    val iso: Int?,
    val afState: Int?,
    val aeState: Int?,
    val aeLock: Boolean?,
    val focusDistance: Float?,
    val json: JSONObject,
) {
    companion object {
        /** Вендорские ключи результата приоритета ISO/выдержки; читаем, если HAL их отдаёт. */
        val VENDOR_RESULT_KEYS: List<VendorResultKey> = listOf(
            VendorResultKey("org.codeaurora.qcamera3.iso_exp_priority.use_iso_exp_priority", dev.fasticamera.probe.camera.VendorValueType.INT64),
            VendorResultKey("org.codeaurora.qcamera3.iso_exp_priority.use_iso_value", dev.fasticamera.probe.camera.VendorValueType.INT32),
            VendorResultKey("org.codeaurora.qcamera3.iso_exp_priority.select_priority", dev.fasticamera.probe.camera.VendorValueType.INT32),
            VendorResultKey("org.codeaurora.qcamera3.sensor_meta_data.current_mode", dev.fasticamera.probe.camera.VendorValueType.INT32),
            VendorResultKey("org.quic.camera.afdData.AntiBandingMode", dev.fasticamera.probe.camera.VendorValueType.INT32),
            VendorResultKey("com.oplus.enable.Antibanding", dev.fasticamera.probe.camera.VendorValueType.INT32),
            VendorResultKey("org.codeaurora.qcamera3.iso_exp_priority.use_gain_value", dev.fasticamera.probe.camera.VendorValueType.FLOAT),
        )

        fun from(tag: ThreeATag, result: TotalCaptureResult): ThreeAFrameRow {
            val exposure = result.get(CaptureResult.SENSOR_EXPOSURE_TIME)
            val iso = result.get(CaptureResult.SENSOR_SENSITIVITY)
            val afState = result.get(CaptureResult.CONTROL_AF_STATE)
            val aeState = result.get(CaptureResult.CONTROL_AE_STATE)
            val aeLock = result.get(CaptureResult.CONTROL_AE_LOCK)
            val focus = result.get(CaptureResult.LENS_FOCUS_DISTANCE)
            val timestamp = result.get(CaptureResult.SENSOR_TIMESTAMP)
            val json = JSONObject()
                .put("phase", tag.phase).put("kind", tag.kind.name).put("round", tag.round).put("index", tag.index)
                .put("requested_exposure_ns", tag.requestedExposureNs ?: JSONObject.NULL)
                .put("frame_number", result.frameNumber)
                .put("sensor_timestamp", timestamp ?: JSONObject.NULL)
                .put("af_mode", result.get(CaptureResult.CONTROL_AF_MODE) ?: JSONObject.NULL)
                .put("af_state", afState ?: JSONObject.NULL)
                .put("af_trigger", result.get(CaptureResult.CONTROL_AF_TRIGGER) ?: JSONObject.NULL)
                .put("ae_mode", result.get(CaptureResult.CONTROL_AE_MODE) ?: JSONObject.NULL)
                .put("ae_state", aeState ?: JSONObject.NULL)
                .put("ae_lock", aeLock ?: JSONObject.NULL)
                .put("ae_comp", result.get(CaptureResult.CONTROL_AE_EXPOSURE_COMPENSATION) ?: JSONObject.NULL)
                .put("ae_antibanding", result.get(CaptureResult.CONTROL_AE_ANTIBANDING_MODE) ?: JSONObject.NULL)
                .put("control_mode", result.get(CaptureResult.CONTROL_MODE) ?: JSONObject.NULL)
                .put("scene_flicker", result.get(CaptureResult.STATISTICS_SCENE_FLICKER) ?: JSONObject.NULL)
                .put("dynamic_black_level", result.get(CaptureResult.SENSOR_DYNAMIC_BLACK_LEVEL)?.joinToString("|") ?: JSONObject.NULL)
                .put("shading_mode", result.get(CaptureResult.SHADING_MODE) ?: JSONObject.NULL)
                .put("lens_shading_map_mode", result.get(CaptureResult.STATISTICS_LENS_SHADING_MAP_MODE) ?: JSONObject.NULL)
                .put("noise_reduction_mode", result.get(CaptureResult.NOISE_REDUCTION_MODE) ?: JSONObject.NULL)
                .put("awb_mode", result.get(CaptureResult.CONTROL_AWB_MODE) ?: JSONObject.NULL)
                .put("awb_state", result.get(CaptureResult.CONTROL_AWB_STATE) ?: JSONObject.NULL)
                .put("focus_distance", focus ?: JSONObject.NULL)
                .put("lens_state", result.get(CaptureResult.LENS_STATE) ?: JSONObject.NULL)
                .put("exposure_ns", exposure ?: JSONObject.NULL)
                .put("iso", iso ?: JSONObject.NULL)
                .put("frame_duration_ns", result.get(CaptureResult.SENSOR_FRAME_DURATION) ?: JSONObject.NULL)
                .put("af_regions", regions(result.get(CaptureResult.CONTROL_AF_REGIONS)))
                .put("ae_regions", regions(result.get(CaptureResult.CONTROL_AE_REGIONS)))
                .put("color_gains", result.get(CaptureResult.COLOR_CORRECTION_GAINS)?.toString() ?: JSONObject.NULL)
            for (key in VENDOR_RESULT_KEYS) {
                val shortName = key.name.substringAfterLast('.')
                json.put("v_$shortName", when (val outcome = key.read(result)) {
                    is VendorResultKey.ReadOutcome.Value -> outcome.value
                    VendorResultKey.ReadOutcome.Absent -> "<absent>"
                    is VendorResultKey.ReadOutcome.Failed -> "<failed: ${outcome.reason}>"
                })
            }
            return ThreeAFrameRow(tag, result.frameNumber, timestamp, exposure, iso, afState, aeState, aeLock, focus, json)
        }

        private fun regions(rectangles: Array<MeteringRectangle>?): Any =
            rectangles?.let { JSONArray(it.map(MeteringRectangle::toString)) } ?: JSONObject.NULL
    }
}
