package dev.fasticamera.probe.camera

import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.params.StreamConfigurationMap
import org.json.JSONArray
import org.json.JSONObject

/**
 * Выжимка CameraCharacteristics одной камеры для каталога C1.
 * Только чтение и форматирование: решения («видна/не видна») принимает отчёт, не этот код.
 */
object CharacteristicsReport {

    /** RAW-форматы, которые проверяем: RAW_SENSOR, RAW_PRIVATE, RAW10, RAW12. */
    private val RAW_FORMATS = mapOf(0x20 to "RAW_SENSOR", 0x24 to "RAW_PRIVATE", 0x25 to "RAW10", 0x26 to "RAW12")

    /** Имена capability из CameraMetadata (REQUEST_AVAILABLE_CAPABILITIES_*), чтобы отчёт читался без справочника. */
    private val CAPABILITY_NAMES = mapOf(
        0 to "BACKWARD_COMPATIBLE", 1 to "MANUAL_SENSOR", 2 to "MANUAL_POST_PROCESSING", 3 to "RAW",
        4 to "PRIVATE_REPROCESSING", 5 to "READ_SENSOR_SETTINGS", 6 to "BURST_CAPTURE", 7 to "YUV_REPROCESSING",
        8 to "DEPTH_OUTPUT", 9 to "CONSTRAINED_HIGH_SPEED_VIDEO", 10 to "MOTION_TRACKING", 11 to "LOGICAL_MULTI_CAMERA",
        12 to "MONOCHROME", 13 to "SECURE_IMAGE_DATA", 14 to "SYSTEM_CAMERA", 15 to "OFFLINE_PROCESSING",
        16 to "ULTRA_HIGH_RESOLUTION_SENSOR", 17 to "REMOSAIC_REPROCESSING", 18 to "DYNAMIC_RANGE_TEN_BIT",
        19 to "STREAM_USE_CASE", 20 to "COLOR_SPACE_PROFILES",
    )

    /**
     * Вендорские статические теги, которые ищем по имени среди ключей характеристик.
     * Их наличие для нашего пакета (а не только в dumpsys) — отдельный факт C1/C5.
     */
    private val INTERESTING_VENDOR_SUBSTRINGS = listOf(
        "SensorModeTable", "CustomHFRFpsTable", "StreamBasedFPSTable", "VideoConfigurationsTable",
        "custom.support.raw.size", "available.stream.fps.ranges", "streamConfigurationMap",
        "qcfa_dimension", "is_qcfa_sensor", "availableStreamMap", "vendor.tag.names",
    )

    fun build(characteristics: CameraCharacteristics): JSONObject {
        val capabilities = characteristics.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES) ?: IntArray(0)
        val streamMap = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val maxResolutionMap = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP_MAXIMUM_RESOLUTION)
        val allKeyNames = characteristics.keys.map { it.name }
        return JSONObject()
            .put("lens_facing", characteristics.get(CameraCharacteristics.LENS_FACING) ?: JSONObject.NULL)
            .put("capabilities", JSONArray(capabilities.map { CAPABILITY_NAMES[it] ?: "UNKNOWN_$it" }))
            .put("physical_ids", JSONArray(characteristics.physicalCameraIds.toList()))
            .put("hardware_level", characteristics.get(CameraCharacteristics.INFO_SUPPORTED_HARDWARE_LEVEL) ?: JSONObject.NULL)
            .put("ae_target_fps_ranges", JSONArray(characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)?.map { it.toString() } ?: emptyList<String>()))
            .put("raw_outputs", rawOutputs(streamMap))
            .put("raw_outputs_max_resolution", rawOutputs(maxResolutionMap))
            .put("high_speed", highSpeed(streamMap))
            .put("session_keys", JSONArray(characteristics.availableSessionKeys.map { it.name }))
            .put("request_key_count", characteristics.availableCaptureRequestKeys.size)
            .put("vendor_request_keys", JSONArray(characteristics.availableCaptureRequestKeys.map { it.name }.filter { !it.startsWith("android.") }))
            .put("characteristics_key_count", allKeyNames.size)
            .put("interesting_vendor_characteristics", JSONArray(allKeyNames.filter { name -> INTERESTING_VENDOR_SUBSTRINGS.any(name::contains) }))
            .put("shading_modes", JSONArray(characteristics.get(CameraCharacteristics.SHADING_AVAILABLE_MODES)?.toList() ?: emptyList<Int>()))
            .put("ois_modes", JSONArray(characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION)?.toList() ?: emptyList<Int>()))
            .put("hot_pixel_modes", JSONArray(characteristics.get(CameraCharacteristics.HOT_PIXEL_AVAILABLE_HOT_PIXEL_MODES)?.toList() ?: emptyList<Int>()))
            .put("noise_reduction_modes", JSONArray(characteristics.get(CameraCharacteristics.NOISE_REDUCTION_AVAILABLE_NOISE_REDUCTION_MODES)?.toList() ?: emptyList<Int>()))
            .put("timestamp_source", characteristics.get(CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE) ?: JSONObject.NULL)
            .put("pixel_array", characteristics.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE)?.toString() ?: JSONObject.NULL)
            .put("pixel_array_max_resolution", characteristics.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE_MAXIMUM_RESOLUTION)?.toString() ?: JSONObject.NULL)
            .put("cfa", characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT) ?: JSONObject.NULL)
            .put("white_level", characteristics.get(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL) ?: JSONObject.NULL)
            .put("focal_lengths", JSONArray(characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.toList() ?: emptyList<Float>()))
    }

    private fun rawOutputs(map: StreamConfigurationMap?): JSONArray {
        val outputs = JSONArray()
        if (map == null) return outputs
        for ((format, name) in RAW_FORMATS) {
            val sizes = try {
                map.getOutputSizes(format)
            } catch (_: IllegalArgumentException) {
                // Формат вообще не поддерживается картой — это не ошибка, а отсутствие.
                null
            } ?: continue
            for (size in sizes) {
                outputs.put(
                    JSONObject()
                        .put("format", name)
                        .put("size", size.toString())
                        .put("min_frame_duration_ns", map.getOutputMinFrameDuration(format, size))
                        .put("stall_duration_ns", map.getOutputStallDuration(format, size)),
                )
            }
        }
        return outputs
    }

    private fun highSpeed(map: StreamConfigurationMap?): JSONArray {
        val result = JSONArray()
        if (map == null) return result
        for (size in map.highSpeedVideoSizes) {
            result.put(JSONObject().put("size", size.toString()).put("fps_ranges", JSONArray(map.getHighSpeedVideoFpsRangesFor(size).map { it.toString() })))
        }
        return result
    }
}
