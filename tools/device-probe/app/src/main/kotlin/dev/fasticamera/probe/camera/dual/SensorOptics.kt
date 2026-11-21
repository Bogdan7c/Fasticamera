package dev.fasticamera.probe.camera.dual

import android.graphics.Rect
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraMetadata
import org.json.JSONArray
import org.json.JSONObject

/** Имена каналов Байера в том порядке, в котором замер сравнивает камеры с разной раскладкой CFA. */
enum class BayerChannel(val wireName: String) { R("R"), GR("Gr"), GB("Gb"), B("B") }

/**
 * Статические факты о камере, нужные для замера чувствительности: оптика, геометрия сенсора, уровни.
 * Читается из `CameraCharacteristics` один раз; дальше — чистые данные.
 */
class SensorOptics(
    val cameraId: String,
    val apertures: List<Double>,
    val focalLengthMm: Double,
    val sensorWidthMm: Double,
    val sensorHeightMm: Double,
    val pixelArrayWidth: Int,
    val pixelArrayHeight: Int,
    val activeArray: Rect,
    val preCorrectionActiveArray: Rect,
    val whiteLevel: Int,
    /** Чёрный уровень по CFA-позициям 2x2 в порядке чтения сенсора (строка*2 + столбец). */
    val blackLevelByReadoutIndex: IntArray,
    val colorFilterArrangement: Int,
    val intrinsicCalibration: FloatArray?,
) {
    /** Главная диафрагма (первая из доступных): у обеих камер телефона она фиксированная. */
    val aperture: Double get() = apertures.first()

    val equivalentFocalLengthMm: Double
        get() = SensitivityGeometry.equivalentFocalLengthMm(focalLengthMm, sensorWidthMm, sensorHeightMm)

    /** Шаг нативного пикселя массива, мкм. */
    val nativePixelPitchUm: Double get() = sensorWidthMm * UM_PER_MM / pixelArrayWidth

    /** Шаг «выходного» пикселя (после биннинга) для RAW-буфера шириной [rawOutputWidth], мкм. */
    fun outputPixelPitchUm(rawOutputWidth: Int): Double = sensorWidthMm * UM_PER_MM / rawOutputWidth

    /** Индекс CFA-позиции (в порядке чтения) для каждого именованного канала. */
    fun readoutIndexOf(channel: BayerChannel): Int = READOUT_INDEX_BY_ARRANGEMENT.getValue(colorFilterArrangement)[channel.ordinal]

    fun toJson(rawOutputWidth: Int): JSONObject = JSONObject()
        .put("camera", cameraId)
        .put("apertures", JSONArray(apertures))
        .put("focal_length_mm", focalLengthMm)
        .put("equivalent_focal_length_mm", equivalentFocalLengthMm)
        .put("sensor_physical_mm", JSONArray(listOf(sensorWidthMm, sensorHeightMm)))
        .put("pixel_array", JSONArray(listOf(pixelArrayWidth, pixelArrayHeight)))
        .put("active_array", rectJson(activeArray))
        .put("pre_correction_active_array", rectJson(preCorrectionActiveArray))
        .put("native_pixel_pitch_um", nativePixelPitchUm)
        .put("output_pixel_pitch_um", outputPixelPitchUm(rawOutputWidth))
        .put("raw_output_width", rawOutputWidth)
        .put("white_level", whiteLevel)
        .put("black_level_pattern_readout_order", JSONArray(blackLevelByReadoutIndex.toList()))
        .put("color_filter_arrangement", colorFilterArrangement)
        .put("lens_intrinsic_calibration", intrinsicCalibration?.let { JSONArray(it.map(Float::toDouble)) } ?: JSONObject.NULL)

    companion object {
        private const val UM_PER_MM = 1000.0

        /** Порядок чтения 2x2 → индексы каналов [R, Gr, Gb, B] для каждой раскладки CFA. */
        private val READOUT_INDEX_BY_ARRANGEMENT = mapOf(
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_RGGB to intArrayOf(0, 1, 2, 3),
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GRBG to intArrayOf(1, 0, 3, 2),
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GBRG to intArrayOf(2, 3, 0, 1),
            CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_BGGR to intArrayOf(3, 2, 1, 0),
        )

        private fun rectJson(rect: Rect) = JSONArray(listOf(rect.left, rect.top, rect.width(), rect.height()))

        /** Любое отсутствующее обязательное поле — исключение с именем поля, а не подстановка. */
        fun fromCharacteristics(cameraId: String, characteristics: CameraCharacteristics): SensorOptics {
            fun <T : Any> required(key: CameraCharacteristics.Key<T>): T =
                characteristics.get(key) ?: error("камера $cameraId: нет характеристики ${key.name}")
            val physicalSize = required(CameraCharacteristics.SENSOR_INFO_PHYSICAL_SIZE)
            val pixelArray = required(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE)
            val blackPattern = required(CameraCharacteristics.SENSOR_BLACK_LEVEL_PATTERN)
            val arrangement = required(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT)
            check(arrangement in READOUT_INDEX_BY_ARRANGEMENT) { "камера $cameraId: CFA $arrangement не Байер" }
            return SensorOptics(
                cameraId = cameraId,
                apertures = required(CameraCharacteristics.LENS_INFO_AVAILABLE_APERTURES).map(Float::toDouble),
                focalLengthMm = required(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS).first().toDouble(),
                sensorWidthMm = physicalSize.width.toDouble(),
                sensorHeightMm = physicalSize.height.toDouble(),
                pixelArrayWidth = pixelArray.width,
                pixelArrayHeight = pixelArray.height,
                activeArray = required(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE),
                preCorrectionActiveArray = required(CameraCharacteristics.SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE),
                whiteLevel = required(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL),
                blackLevelByReadoutIndex = IntArray(RAW_CHANNELS) { index ->
                    blackPattern.getOffsetForIndex(index % 2, index / 2)
                },
                colorFilterArrangement = arrangement,
                intrinsicCalibration = characteristics.get(CameraCharacteristics.LENS_INTRINSIC_CALIBRATION),
            )
        }

        private const val RAW_CHANNELS = 4
    }
}
