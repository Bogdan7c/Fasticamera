package dev.fasticamera.probe.camera

import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.hardware.camera2.params.ColorSpaceTransform
import dev.fasticamera.probe.camera.shading.LensShadingGains
import org.json.JSONArray
import org.json.JSONObject

/**
 * «Паспорт» RAW-кадра (C7): всё, что нашему ISP в сессии 06 нужно знать о кадре,
 * снятое из характеристик камеры и результата именно этого кадра.
 *
 * Поле отсутствует в JSON, если HAL не отдал значение (org.json не хранит null) — это «нет данных»,
 * а не ноль.
 *
 * Отдельно фиксируем, что вендор уже сделал с RAW (lensShadingApplied, режимы NR/shading),
 * чтобы инвариант 2 README («вендорская обработка честно отмечается») опирался на факты.
 */
object RawPassport {

    fun build(characteristics: CameraCharacteristics, result: CaptureResult): JSONObject = JSONObject()
        .put("sensor", sensorStatic(characteristics))
        .put("color", colorStatic(characteristics))
        .put("frame", frameDynamic(result))

    private fun sensorStatic(characteristics: CameraCharacteristics): JSONObject = JSONObject()
        .put("pixel_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE)?.toString())
        .put("active_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)?.flattenToString())
        .put("cfa", characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT))
        .put("white_level", characteristics.get(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL))
        .put("black_level_pattern", characteristics.get(CameraCharacteristics.SENSOR_BLACK_LEVEL_PATTERN)?.let { pattern ->
            JSONArray((0 until 2).flatMap { row -> (0 until 2).map { column -> pattern.getOffsetForIndex(column, row) } })
        })
        .put("optical_black_regions", characteristics.get(CameraCharacteristics.SENSOR_OPTICAL_BLACK_REGIONS)?.size ?: 0)
        .put("lens_shading_applied", characteristics.get(CameraCharacteristics.SENSOR_INFO_LENS_SHADING_APPLIED))
        .put("timestamp_source", characteristics.get(CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE))
        .put("physical_size_mm", characteristics.get(CameraCharacteristics.SENSOR_INFO_PHYSICAL_SIZE)?.toString())
        .put("focal_lengths", JSONArray(characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.toList() ?: emptyList<Float>()))
        .put("apertures", JSONArray(characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_APERTURES)?.toList() ?: emptyList<Float>()))

    private fun colorStatic(characteristics: CameraCharacteristics): JSONObject = JSONObject()
        .put("reference_illuminant1", characteristics.get(CameraCharacteristics.SENSOR_REFERENCE_ILLUMINANT1))
        .put("reference_illuminant2", characteristics.get(CameraCharacteristics.SENSOR_REFERENCE_ILLUMINANT2)?.toInt())
        .put("color_matrix1", matrix(characteristics.get(CameraCharacteristics.SENSOR_COLOR_TRANSFORM1)))
        .put("color_matrix2", matrix(characteristics.get(CameraCharacteristics.SENSOR_COLOR_TRANSFORM2)))
        .put("forward_matrix1", matrix(characteristics.get(CameraCharacteristics.SENSOR_FORWARD_MATRIX1)))
        .put("forward_matrix2", matrix(characteristics.get(CameraCharacteristics.SENSOR_FORWARD_MATRIX2)))
        .put("calibration1", matrix(characteristics.get(CameraCharacteristics.SENSOR_CALIBRATION_TRANSFORM1)))
        .put("calibration2", matrix(characteristics.get(CameraCharacteristics.SENSOR_CALIBRATION_TRANSFORM2)))

    private fun frameDynamic(result: CaptureResult): JSONObject = JSONObject()
        .put("timestamp_ns", result.get(CaptureResult.SENSOR_TIMESTAMP))
        .put("exposure_ns", result.get(CaptureResult.SENSOR_EXPOSURE_TIME))
        .put("iso", result.get(CaptureResult.SENSOR_SENSITIVITY))
        .put("frame_duration_ns", result.get(CaptureResult.SENSOR_FRAME_DURATION))
        .put("rolling_shutter_skew_ns", result.get(CaptureResult.SENSOR_ROLLING_SHUTTER_SKEW))
        .put("dynamic_black_level", result.get(CaptureResult.SENSOR_DYNAMIC_BLACK_LEVEL)?.let { JSONArray(it.toList()) })
        .put("dynamic_white_level", result.get(CaptureResult.SENSOR_DYNAMIC_WHITE_LEVEL))
        .put("neutral_color_point", result.get(CaptureResult.SENSOR_NEUTRAL_COLOR_POINT)?.let { point ->
            JSONArray(point.map { it.toDouble() })
        })
        .put("noise_profile", result.get(CaptureResult.SENSOR_NOISE_PROFILE)?.let { profile ->
            JSONArray(profile.map { JSONArray(listOf(it.first, it.second)) })
        })
        .put("green_split", result.get(CaptureResult.SENSOR_GREEN_SPLIT))
        .put("raw_binning_factor_used", result.get(CaptureResult.SENSOR_RAW_BINNING_FACTOR_USED))
        .put("awb_gains", result.get(CaptureResult.COLOR_CORRECTION_GAINS)?.let {
            JSONArray(listOf(it.red, it.greenEven, it.greenOdd, it.blue))
        })
        .put("ccm", matrix(result.get(CaptureResult.COLOR_CORRECTION_TRANSFORM)))
        .put("shading_mode", result.get(CaptureResult.SHADING_MODE))
        .put("noise_reduction_mode", result.get(CaptureResult.NOISE_REDUCTION_MODE))
        .put("hot_pixel_mode", result.get(CaptureResult.HOT_PIXEL_MODE))
        .put("lens_shading_map_mode", result.get(CaptureResult.STATISTICS_LENS_SHADING_MAP_MODE))
        .put("lens_shading_map", result.get(CaptureResult.STATISTICS_LENS_SHADING_CORRECTION_MAP)?.let { map ->
            JSONObject().put("columns", map.columnCount).put("rows", map.rowCount)
                .put("center_gain_g", map.getGainFactor(1, map.columnCount / 2, map.rowCount / 2))
                .put("corner_gain_g", map.getGainFactor(1, 0, 0))
                // Полная карта: row-major, в каждой точке [R, G_even, G_odd, B] (порядок Camera2).
                .put("gains", JSONArray(LensShadingGains.flatten(map).toList()))
        })
        .put("focus_distance", result.get(CaptureResult.LENS_FOCUS_DISTANCE))
        .put("aperture", result.get(CaptureResult.LENS_APERTURE))
        .put("focal_length", result.get(CaptureResult.LENS_FOCAL_LENGTH))
        .put("ois_mode", result.get(CaptureResult.LENS_OPTICAL_STABILIZATION_MODE))
        .put("ae_state", result.get(CaptureResult.CONTROL_AE_STATE))
        .put("awb_state", result.get(CaptureResult.CONTROL_AWB_STATE))
        .put("af_state", result.get(CaptureResult.CONTROL_AF_STATE))

    /** Матрица 3×3 из рациональных чисел → плоский список double (по строкам). */
    private fun matrix(transform: ColorSpaceTransform?): JSONArray? = transform?.let {
        JSONArray((0 until 3).flatMap { row -> (0 until 3).map { column -> it.getElement(column, row).toDouble() } })
    }

    /** Режимы запроса, которыми снимаем эталон: вендорскую обработку по возможности выключаем. */
    fun applyCleanRawControls(builder: CaptureRequest.Builder) {
        builder.set(CaptureRequest.NOISE_REDUCTION_MODE, CaptureRequest.NOISE_REDUCTION_MODE_OFF)
        builder.set(CaptureRequest.HOT_PIXEL_MODE, CaptureRequest.HOT_PIXEL_MODE_OFF)
        builder.set(CaptureRequest.STATISTICS_LENS_SHADING_MAP_MODE, CaptureRequest.STATISTICS_LENS_SHADING_MAP_MODE_ON)
    }
}
