package dev.fasticamera.probe.camera.dual

import android.hardware.camera2.CameraCharacteristics
import org.json.JSONArray
import org.json.JSONObject

/**
 * Статические геометрические факты линзы для разбора параллакса и совмещения камер:
 * поза относительно опорной системы, дисторсия, диапазон фокусировки. Отсутствующий ключ пишется как null,
 * а не подменяется нулём: «HAL не отдаёт» — тоже результат разведки.
 */
object LensGeometryFacts {
    fun fromCharacteristics(characteristics: CameraCharacteristics): JSONObject = JSONObject()
        .put("lens_pose_translation_m", floats(characteristics.get(CameraCharacteristics.LENS_POSE_TRANSLATION)))
        .put("lens_pose_rotation_quaternion_xyzw", floats(characteristics.get(CameraCharacteristics.LENS_POSE_ROTATION)))
        .put("lens_pose_reference", characteristics.get(CameraCharacteristics.LENS_POSE_REFERENCE) ?: JSONObject.NULL)
        .put("lens_distortion", floats(characteristics.get(CameraCharacteristics.LENS_DISTORTION)))
        .put("lens_intrinsic_calibration", floats(characteristics.get(CameraCharacteristics.LENS_INTRINSIC_CALIBRATION)))
        .put("minimum_focus_distance_diopters", characteristics.get(CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE) ?: JSONObject.NULL)
        .put("hyperfocal_distance_diopters", characteristics.get(CameraCharacteristics.LENS_INFO_HYPERFOCAL_DISTANCE) ?: JSONObject.NULL)
        .put("focus_distance_calibration", characteristics.get(CameraCharacteristics.LENS_INFO_FOCUS_DISTANCE_CALIBRATION) ?: JSONObject.NULL)

    private fun floats(values: FloatArray?): Any = values?.let { JSONArray(it.map(Float::toDouble)) } ?: JSONObject.NULL
}
