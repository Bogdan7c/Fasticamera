package dev.fasticamera.probe.camera.shading

import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.util.Size
import dev.fasticamera.probe.camera.VendorResultKey
import dev.fasticamera.probe.camera.VendorValueType
import dev.fasticamera.probe.camera.dual.afStateName
import org.json.JSONArray
import org.json.JSONObject

/**
 * Сборка JSON для отчёта shading: статика камеры и запись одного кадра.
 * Поле отсутствует, если HAL его не отдал (org.json не хранит null) — это «нет данных», а не ноль.
 */
object ShadingJson {
    private val sensorModeKey = VendorResultKey("org.codeaurora.qcamera3.sensor_meta_data.current_mode", VendorValueType.INT32)

    /** Характеристики камеры, нужные для разбора карты затенения; размер буфера дописывается позже по первому кадру. */
    fun characteristics(characteristics: CameraCharacteristics): JSONObject = JSONObject()
        .put("pixel_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE)?.toString())
        .put("active_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)?.flattenToString())
        .put("pre_correction_active_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE)?.flattenToString())
        .put("lens_shading_applied", characteristics.get(CameraCharacteristics.SENSOR_INFO_LENS_SHADING_APPLIED))
        .put("lens_shading_map_size", readShadingMapSize(characteristics))
        .put("cfa", characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT))
        .put("black_level_pattern", characteristics.get(CameraCharacteristics.SENSOR_BLACK_LEVEL_PATTERN)?.let { pattern ->
            // Порядок = позиции в буфере: (0,0), (0,1), (1,0), (1,1) — строка*2 + столбец.
            JSONArray((0 until 2).flatMap { row -> (0 until 2).map { column -> pattern.getOffsetForIndex(column, row) } })
        })
        .put("white_level", characteristics.get(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL))
        .put("min_focus_distance", characteristics.get(CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE))
        .put("af_available_modes", characteristics.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)?.let { JSONArray(it.toList()) })
        .put("exposure_time_range_ns", characteristics.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)?.toString())
        .put("sensitivity_range", characteristics.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)?.toString())

    /**
     * LENS_INFO_SHADING_MAP_SIZE скрыт из публичного SDK, поэтому читаем его по имени ключа.
     * Если ключ не читается, в отчёт идёт причина (а не пропуск): реальный размер всё равно виден в карте каждого кадра.
     */
    private fun readShadingMapSize(characteristics: CameraCharacteristics): String = try {
        characteristics.get(CameraCharacteristics.Key("android.lens.info.shadingMapSize", Size::class.java))?.toString() ?: "absent"
    } catch (error: RuntimeException) {
        "unreadable: ${error.javaClass.simpleName}: ${error.message}"
    }

    /** Запись одного собранного кадра: применённые параметры из результата и полная карта затенения. */
    fun frameRecord(result: CaptureResult, generation: Int, validRows: Int): JSONObject {
        val record = JSONObject()
            .put("timestamp_ns", result.get(CaptureResult.SENSOR_TIMESTAMP))
            .put("frame_number", result.frameNumber)
            .put("generation", generation)
            .put("valid_rows", validRows)
            .put("exposure_ns", result.get(CaptureResult.SENSOR_EXPOSURE_TIME))
            .put("iso", result.get(CaptureResult.SENSOR_SENSITIVITY))
            .put("frame_duration_ns", result.get(CaptureResult.SENSOR_FRAME_DURATION))
            .put("ae_mode", result.get(CaptureResult.CONTROL_AE_MODE))
            .put("af_mode", result.get(CaptureResult.CONTROL_AF_MODE))
            .put("focus_distance", result.get(CaptureResult.LENS_FOCUS_DISTANCE))
            .put("af_state", result.get(CaptureResult.CONTROL_AF_STATE)?.let(::afStateName))
            .put("lens_state", result.get(CaptureResult.LENS_STATE))
            .put("shading_mode", result.get(CaptureResult.SHADING_MODE))
            .put("lens_shading_map_mode", result.get(CaptureResult.STATISTICS_LENS_SHADING_MAP_MODE))
            .put("noise_reduction_mode", result.get(CaptureResult.NOISE_REDUCTION_MODE))
            .put("hot_pixel_mode", result.get(CaptureResult.HOT_PIXEL_MODE))
            .put("awb_gains", result.get(CaptureResult.COLOR_CORRECTION_GAINS)?.let { JSONArray(listOf(it.red, it.greenEven, it.greenOdd, it.blue)) })
            .put("noise_profile", result.get(CaptureResult.SENSOR_NOISE_PROFILE)?.let { profile -> JSONArray(profile.map { JSONArray(listOf(it.first, it.second)) }) })
            .put("dynamic_black_level", result.get(CaptureResult.SENSOR_DYNAMIC_BLACK_LEVEL)?.let { JSONArray(it.toList()) })
            .put("dynamic_white_level", result.get(CaptureResult.SENSOR_DYNAMIC_WHITE_LEVEL))
        putVendorSensorMode(record, result)
        putLensShadingMap(record, result)
        return record
    }

    /** Режим сенсора вендора: значение, либо причина отсутствия — три исхода не сливаем. */
    private fun putVendorSensorMode(record: JSONObject, result: CaptureResult) {
        when (val outcome = sensorModeKey.read(result)) {
            is VendorResultKey.ReadOutcome.Value -> record.put("vendor_sensor_mode", outcome.value)
            VendorResultKey.ReadOutcome.Absent -> record.put("vendor_sensor_mode_status", "absent")
            is VendorResultKey.ReadOutcome.Failed -> record.put("vendor_sensor_mode_status", outcome.reason)
        }
    }

    /** Карта затенения: columns/rows/gains, либо null с причиной (запрошена ли карта и что вернул HAL). */
    private fun putLensShadingMap(record: JSONObject, result: CaptureResult) {
        val map = result.get(CaptureResult.STATISTICS_LENS_SHADING_CORRECTION_MAP)
        if (map != null) {
            record.put(
                "lens_shading_map",
                JSONObject().put("columns", map.columnCount).put("rows", map.rowCount)
                    .put("channel_order", "R,G_even,G_odd,B interleaved per point, row-major")
                    .put("gains", JSONArray(LensShadingGains.flatten(map).toList())),
            )
            return
        }
        val modeOff = result.get(CaptureResult.STATISTICS_LENS_SHADING_MAP_MODE) == CaptureRequest.STATISTICS_LENS_SHADING_MAP_MODE_OFF
        record.put("lens_shading_map", JSONObject.NULL)
        record.put(
            "lens_shading_map_absent_reason",
            if (modeOff) "STATISTICS_LENS_SHADING_MAP_MODE=OFF в результате" else "HAL не вернул карту при запрошенном режиме",
        )
    }
}
