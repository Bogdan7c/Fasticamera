package dev.fasticamera.probe.camera.shading

import kotlin.math.roundToInt
import kotlin.math.roundToLong

/** Ручная экспозиция: выдержка и ISO (длительность кадра задаёт режим камеры, не поиск). */
data class ExposureSetting(val exposureNs: Long, val iso: Int)

/** Пределы сенсора из характеристик камеры. */
data class SensorLimits(val minExposureNs: Long, val maxExposureNs: Long, val minIso: Int, val maxIso: Int)

/**
 * Подбор экспозиции под плоское поле: масштабирует «выдержка x ISO» так, чтобы центр дал целевой уровень,
 * не загоняя самый яркий блок в клиппинг. Сначала меняется выдержка при прежнем ISO; упёрлась в потолок
 * (длительность кадра режима) или в минимум — остаток уходит в ISO.
 */
object ExposureSearch {
    /** Самый яркий блок обязан быть ниже этого (спецификация: < 0.9). */
    const val CLIP_LIMIT = 0.9

    /** Целимся в яркий блок с запасом ниже CLIP_LIMIT, чтобы следующая итерация не оказалась на пределе. */
    private const val BRIGHTEST_SAFE_LEVEL = 0.85

    /** Допуск сходимости центра относительно цели (10%). */
    private const val CENTRE_TOLERANCE = 0.10

    /** За один шаг не меняем яркость более чем в 16 раз: защита от нулевого/шумового замера. */
    private const val MAX_STEP_FACTOR = 16.0

    /** Практически чёрный центр: коэффициент считать нельзя, поднимаем яркость максимальным шагом. */
    private const val DARK_CENTRE_EPSILON = 1e-4

    fun isConverged(levels: FrameLevels, target: Double): Boolean =
        levels.brightestBlock < CLIP_LIMIT && kotlin.math.abs(levels.centreGreen / target - 1.0) <= CENTRE_TOLERANCE

    /** Множитель яркости: по центру к цели, но не выше, чем позволяет яркий блок. */
    fun brightnessFactor(levels: FrameLevels, target: Double): Double {
        val byCentre = if (levels.centreGreen < DARK_CENTRE_EPSILON) MAX_STEP_FACTOR else target / levels.centreGreen
        val byClipping = if (levels.brightestBlock < DARK_CENTRE_EPSILON) MAX_STEP_FACTOR else BRIGHTEST_SAFE_LEVEL / levels.brightestBlock
        return minOf(byCentre, byClipping).coerceIn(1.0 / MAX_STEP_FACTOR, MAX_STEP_FACTOR)
    }

    /** Следующая экспозиция для измеренных [levels]; [exposureCapNs] — потолок выдержки режима. */
    fun nextSetting(current: ExposureSetting, levels: FrameLevels, target: Double, limits: SensorLimits, exposureCapNs: Long): ExposureSetting {
        val product = current.exposureNs.toDouble() * current.iso * brightnessFactor(levels, target)
        return settingForProduct(product, current.iso, limits, exposureCapNs)
    }

    /** Приводит произвольную экспозицию к допустимой (потолок, диапазоны), сохраняя «выдержка x ISO». */
    fun constrain(setting: ExposureSetting, limits: SensorLimits, exposureCapNs: Long): ExposureSetting =
        settingForProduct(setting.exposureNs.toDouble() * setting.iso, setting.iso, limits, exposureCapNs)

    /** [product] = выдержка(нс) x ISO. Выдержка при [preferredIso], если укладывается в пределы; иначе упирается в предел, остаток — в ISO. */
    fun settingForProduct(product: Double, preferredIso: Int, limits: SensorLimits, exposureCapNs: Long): ExposureSetting {
        val exposureCeiling = minOf(exposureCapNs, limits.maxExposureNs)
        val exposureNs = (product / preferredIso).coerceIn(limits.minExposureNs.toDouble(), exposureCeiling.toDouble()).roundToLong()
        val iso = (product / exposureNs).roundToInt().coerceIn(limits.minIso, limits.maxIso)
        return ExposureSetting(exposureNs, iso)
    }
}
