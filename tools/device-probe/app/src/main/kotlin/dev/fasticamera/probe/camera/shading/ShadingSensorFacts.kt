package dev.fasticamera.probe.camera.shading

import android.hardware.camera2.CameraCharacteristics

/** Статические факты камеры, нужные сценарию: пределы экспозиции, чёрный/белый уровни, CFA, фокус. */
class ShadingSensorFacts(
    val limits: SensorLimits,
    /** Чёрный уровень по CFA-плоскостям буфера: индекс = строка*2 + столбец. */
    val blackByPlane: DoubleArray,
    val whiteLevel: Int,
    val greenPlanes: IntArray,
    val focus: FocusCapabilities,
) {
    companion object {
        /** Отсутствие обязательного поля — исключение: без него замер не имеет смысла, и подставлять нечего. */
        fun read(characteristics: CameraCharacteristics): ShadingSensorFacts {
            val exposureRange = requireNotNull(characteristics.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)) { "нет диапазона выдержки" }
            val isoRange = requireNotNull(characteristics.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)) { "нет диапазона ISO" }
            val pattern = requireNotNull(characteristics.get(CameraCharacteristics.SENSOR_BLACK_LEVEL_PATTERN)) { "нет чёрного уровня" }
            val cfa = requireNotNull(characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT)) { "нет раскладки CFA" }
            val black = DoubleArray(CFA_PLANE_COUNT) { plane -> pattern.getOffsetForIndex(plane % 2, plane / 2).toDouble() }
            return ShadingSensorFacts(
                limits = SensorLimits(exposureRange.lower, exposureRange.upper, isoRange.lower, isoRange.upper),
                blackByPlane = black,
                whiteLevel = requireNotNull(characteristics.get(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL)) { "нет уровня белого" },
                greenPlanes = BlockStatistics.greenPlanes(cfa),
                focus = FocusCapabilities(
                    characteristics.get(CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE),
                    characteristics.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES),
                ),
            )
        }
    }
}
