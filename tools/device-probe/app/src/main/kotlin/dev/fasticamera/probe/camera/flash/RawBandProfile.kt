package dev.fasticamera.probe.camera.flash

import dev.fasticamera.probe.camera.Raw10Pixels
import dev.fasticamera.probe.camera.dual.GreenLayout
import java.nio.ByteBuffer

/**
 * Профиль яркости одного RAW10-кадра: среднее зелёных за вычетом чёрного по всему кадру и по горизонтальным полосам
 * сверху вниз. Полосы нужны, чтобы увидеть частичную засветку вспышкой при скользящем затворе.
 */
class RawBandProfile(
    /** Среднее зелёных минус чёрный по всему кадру. */
    val overallMean: Double,
    /** Средние по полосам сверху вниз (одинаковое число отсчётов в каждой). */
    val bandMeans: DoubleArray,
    /** Доля зелёных отсчётов у потолка датчика (клип). */
    val clipFraction: Double,
)

/** Параметры измерения профиля; всё, что можно настроить, вынесено из кода. */
class RawBandSettings(
    val greens: GreenLayout,
    /** Чёрный уровень RAW10 (для OnePlus 11 — 64). */
    val blackLevel: Int,
    val bandCount: Int,
    /** Шаг сетки отсчётов по X и Y в пикселях (чётный: не сбивает фазу CFA). */
    val stepX: Int,
    val stepY: Int,
    /** Значение, начиная с которого отсчёт считается клипом. */
    val clipLevel: Int,
)

/** Чистая функция измерения: читает только буфер и настройки, ничего не пишет. */
object RawBandAnalyzer {

    fun analyze(buffer: ByteBuffer, rowStrideBytes: Int, width: Int, height: Int, settings: RawBandSettings): RawBandProfile {
        require(settings.stepX % 2 == 0 && settings.stepY % 2 == 0) { "шаг сетки должен быть чётным: CFA-ячейка 2x2" }
        val bandSums = DoubleArray(settings.bandCount)
        val bandCounts = IntArray(settings.bandCount)
        var clipped = 0L
        var total = 0L
        for (cellY in 0 until height - 1 step settings.stepY) {
            // Номер полосы по строке ячейки: полосы одной высоты, сверху вниз.
            val band = cellY * settings.bandCount / height
            for (cellX in 0 until width - 1 step settings.stepX) {
                for (readoutIndex in intArrayOf(settings.greens.firstReadoutIndex, settings.greens.secondReadoutIndex)) {
                    val raw = Raw10Pixels.read(buffer, rowStrideBytes, cellX + readoutIndex % 2, cellY + readoutIndex / 2)
                    bandSums[band] += (raw - settings.blackLevel).toDouble()
                    bandCounts[band] += 1
                    if (raw >= settings.clipLevel) clipped += 1
                    total += 1
                }
            }
        }
        val bandMeans = DoubleArray(settings.bandCount) { if (bandCounts[it] == 0) 0.0 else bandSums[it] / bandCounts[it] }
        val allCount = bandCounts.sum()
        val overall = if (allCount == 0) 0.0 else bandSums.sum() / allCount
        return RawBandProfile(overall, bandMeans, if (total == 0L) 0.0 else clipped.toDouble() / total)
    }
}
