package dev.fasticamera.probe.camera.dual

import android.media.Image
import dev.fasticamera.probe.camera.Raw10Pixels
import java.nio.ByteBuffer

/** Потребитель кадра ног эксперимента: вызывается на потоке ImageReader до возврата буфера. */
fun interface RawFrameSink {
    fun onFrame(image: Image)
}

/**
 * Результат замера одного кадра: суммы/счётчики по четырём CFA-позициям 2x2 (индекс = строка*2 + столбец
 * в порядке чтения сенсора, как у `BlackLevelPattern`) и число пикселей на уровне белого и выше.
 */
class RegionFrameSample(
    val timestampNs: Long,
    val channelMeans: DoubleArray,
    val pixelsPerChannel: Int,
    val clippedPerChannel: IntArray,
)

/**
 * Копит средние RAW10 по каналам в [region] для кадров, пока включён сбор.
 *
 * Читается разреженная сетка блоков 2x2 с шагом [BLOCK_STEP] блоков: полный проход по ~2 Мпикс на 60 к/с
 * не успел бы за потоком ImageReader. Сцена статична и гладка, так что для среднего и доли клиппинга
 * разреженной выборки достаточно (это допущение попадает в отчёт).
 * Ошибка анализа не глотается: она запоминается и уходит в отчёт через [failureOrNull].
 */
class RawRegionSampler(private val region: PixelRegion, private val whiteLevel: Int) : RawFrameSink {
    private val lock = Any()
    private val samples = ArrayList<RegionFrameSample>()
    private var collecting = false
    private var failure: String? = null

    /** Начинает новый сбор: прежние кадры отбрасываются. */
    fun startCollecting() = synchronized(lock) {
        samples.clear()
        collecting = true
    }

    /** Завершает сбор и отдаёт кадры в порядке прихода. */
    fun stopCollecting(): List<RegionFrameSample> = synchronized(lock) {
        collecting = false
        samples.toList()
    }

    val collectedCount: Int get() = synchronized(lock) { samples.size }

    fun failureOrNull(): String? = synchronized(lock) { failure }

    override fun onFrame(image: Image) {
        if (!synchronized(lock) { collecting }) return
        try {
            val plane = image.planes.first()
            val sample = measure(plane.buffer.duplicate(), plane.rowStride, image.timestamp)
            synchronized(lock) { if (collecting) samples += sample }
        } catch (error: RuntimeException) {
            synchronized(lock) { failure = "${error.javaClass.name}: ${error.message}" }
        }
    }

    /** Замер одного кадра; открыт для JVM-тестов на синтетическом буфере. */
    internal fun measure(buffer: ByteBuffer, rowStrideBytes: Int, timestampNs: Long): RegionFrameSample {
        val sums = DoubleArray(CHANNELS)
        val clipped = IntArray(CHANNELS)
        var blocks = 0
        for (y in region.top until region.top + region.height - 1 step BLOCK_STEP * 2) {
            for (x in region.left until region.left + region.width - 1 step BLOCK_STEP * 2) {
                blocks++
                for (channel in 0 until CHANNELS) {
                    val value = Raw10Pixels.read(buffer, rowStrideBytes, x + channel % 2, y + channel / 2)
                    sums[channel] += value.toDouble()
                    if (value >= whiteLevel) clipped[channel]++
                }
            }
        }
        require(blocks > 0) { "область $region слишком мала для выборки" }
        return RegionFrameSample(timestampNs, DoubleArray(CHANNELS) { sums[it] / blocks }, blocks, clipped)
    }

    companion object {
        const val CHANNELS = 4

        /** Шаг выборки в блоках 2x2: каждый 4-й блок по обеим осям (1/16 пикселей). */
        const val BLOCK_STEP = 4
    }
}
