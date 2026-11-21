package dev.fasticamera.probe.camera.dual

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.ln

/** Прямоугольная область RAW-буфера в пикселях; левый верхний угол и размеры всегда чётные (фаза CFA 2x2). */
data class PixelRegion(val left: Int, val top: Int, val width: Int, val height: Int)

/** Откуда взят центр области: оптическая ось из калибровки линзы или запасной вариант — центр массива. */
enum class OpticalCenterSource(val wireName: String) {
    LENS_INTRINSIC_CALIBRATION("lens_intrinsic_calibration"),
    ARRAY_CENTER("array_center"),
}

/** Центр области в долях ширины/высоты (0..1) и источник этих долей. */
data class OpticalCenter(val fractionX: Double, val fractionY: Double, val source: OpticalCenterSource)

/**
 * Чистая математика замера чувствительности: сопоставление поля зрения двух камер и ожидаемая разница в EV.
 * Без обращений к Android, чтобы проверяться обычными JVM-тестами.
 */
object SensitivityGeometry {
    /** Диагональ полнокадрового сенсора 36x24 мм — база «эквивалентного» фокусного расстояния. */
    const val FULL_FRAME_DIAGONAL_MM = 43.27

    /** Эквивалентное фокусное расстояние (35 мм) по физическому фокусному и размеру сенсора. */
    fun equivalentFocalLengthMm(focalLengthMm: Double, sensorWidthMm: Double, sensorHeightMm: Double): Double =
        focalLengthMm * FULL_FRAME_DIAGONAL_MM / hypot(sensorWidthMm, sensorHeightMm)

    /**
     * Центр оптической оси из `LENS_INTRINSIC_CALIBRATION` ([fx, fy, cx, cy, s]) в долях массива.
     * Калибровка «правдоподобна», если cx и cy ненулевые и лежат внутри массива; иначе — центр массива.
     */
    fun opticalCenter(calibration: FloatArray?, arrayWidth: Int, arrayHeight: Int): OpticalCenter {
        val centerX = calibration?.getOrNull(CALIBRATION_CX_INDEX)?.toDouble()
        val centerY = calibration?.getOrNull(CALIBRATION_CY_INDEX)?.toDouble()
        val plausible = centerX != null && centerY != null &&
            centerX > 0.0 && centerY > 0.0 && centerX < arrayWidth && centerY < arrayHeight
        return if (plausible) {
            OpticalCenter(centerX!! / arrayWidth, centerY!! / arrayHeight, OpticalCenterSource.LENS_INTRINSIC_CALIBRATION)
        } else {
            OpticalCenter(CENTER_FRACTION, CENTER_FRACTION, OpticalCenterSource.ARRAY_CENTER)
        }
    }

    /**
     * Область с долей [sizeFraction] ширины и высоты буфера вокруг [center]. Сдвигается внутрь буфера, если
     * вылезает за край; координаты приводятся к чётным, размеры — к чётным вниз.
     */
    fun centeredRegion(bufferWidth: Int, bufferHeight: Int, sizeFraction: Double, center: OpticalCenter): PixelRegion {
        require(sizeFraction > 0.0 && sizeFraction <= 1.0) { "доля области должна быть в (0, 1], получено $sizeFraction" }
        val width = evenFloor((bufferWidth * sizeFraction).toInt()).coerceAtLeast(2)
        val height = evenFloor((bufferHeight * sizeFraction).toInt()).coerceAtLeast(2)
        val left = evenFloor((center.fractionX * bufferWidth - width / 2.0).toInt()).coerceIn(0, evenFloor(bufferWidth - width))
        val top = evenFloor((center.fractionY * bufferHeight - height / 2.0).toInt()).coerceIn(0, evenFloor(bufferHeight - height))
        return PixelRegion(left, top, width, height)
    }

    /** EV-разница «основная минус широкая» по средним сигналам: log2(main / wide). */
    fun evMainMinusWide(meanMain: Double, meanWide: Double): Double = ln(meanMain / meanWide) / LN_2

    /** Ожидаемая разница в EV только от диафрагмы: 2*log2(N_wide / N_main); положительна, если у основной светосильнее. */
    fun expectedEvFromAperture(apertureMain: Double, apertureWide: Double): Double =
        2.0 * ln(apertureWide / apertureMain) / LN_2

    /** Добавка от площади пикселя: log2(площадь_main / площадь_wide) = 2*log2(pitch_main / pitch_wide). */
    fun expectedEvFromPixelPitch(pitchMainUm: Double, pitchWideUm: Double): Double =
        2.0 * ln(pitchMainUm / pitchWideUm) / LN_2

    /** Совпадают ли значения в пределах относительного допуска (для сверки заказанной и применённой экспозиции). */
    fun nearlyEqual(actual: Double, expected: Double, relativeTolerance: Double): Boolean =
        abs(actual - expected) <= abs(expected) * relativeTolerance

    private fun evenFloor(value: Int): Int = value and 1.inv()

    private const val CALIBRATION_CX_INDEX = 2
    private const val CALIBRATION_CY_INDEX = 3
    private const val CENTER_FRACTION = 0.5
    private val LN_2 = ln(2.0)
}
