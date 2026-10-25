package dev.fasticamera.probe.camera

import org.json.JSONObject

/**
 * Статистика темпа кадров по меткам времени сенсора (`SENSOR_TIMESTAMP`, нс).
 *
 * Почему по меткам сенсора, а не по колбэкам: колбэки приходят пачками и с джиттером
 * планировщика, а метка сенсора — момент начала экспозиции, единственный честный
 * источник фактических к/с (общие правила README, «Замеры на телефоне»).
 *
 * Пропуск кадра — интервал длиннее медианы в [dropFactor] раз; число пропущенных
 * кадров оценивается округлением интервала к целому числу медиан.
 */
data class FrameTimingStats(
    val frameCount: Int,
    val medianIntervalNs: Long,
    val meanIntervalNs: Double,
    val minIntervalNs: Long,
    val maxIntervalNs: Long,
    val fpsFromMedian: Double,
    val fpsFromSpan: Double,
    val dropEvents: Int,
    val estimatedDroppedFrames: Int,
    val nonMonotonicCount: Int,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("frames", frameCount)
        .put("median_interval_ns", medianIntervalNs)
        .put("mean_interval_ns", meanIntervalNs)
        .put("min_interval_ns", minIntervalNs)
        .put("max_interval_ns", maxIntervalNs)
        .put("fps_median", fpsFromMedian)
        .put("fps_span", fpsFromSpan)
        .put("drop_events", dropEvents)
        .put("estimated_dropped_frames", estimatedDroppedFrames)
        .put("non_monotonic", nonMonotonicCount)

    companion object {
        private const val NANOS_PER_SECOND = 1_000_000_000.0

        /** Порог «пропуска»: интервал больше 1,5 медианы. */
        const val DEFAULT_DROP_FACTOR = 1.5

        /**
         * Считает статистику. Меньше двух меток — интервалов нет, это отдельный исход ([Insufficient]),
         * а не нули, которые выглядели бы как «0 к/с».
         */
        fun compute(timestampsNs: List<Long>, dropFactor: Double = DEFAULT_DROP_FACTOR): Outcome {
            if (timestampsNs.size < 2) {
                return Outcome.Insufficient(timestampsNs.size)
            }
            val intervals = timestampsNs.zipWithNext { previous, next -> next - previous }
            // Немонотонные метки (повтор или откат) — признак бага/переупорядочивания, считаем отдельно.
            val nonMonotonic = intervals.count { it <= 0 }
            val positive = intervals.filter { it > 0 }
            if (positive.isEmpty()) {
                return Outcome.Insufficient(timestampsNs.size)
            }
            val sorted = positive.sorted()
            val median = sorted[sorted.size / 2]
            val dropThreshold = median * dropFactor
            val drops = positive.filter { it > dropThreshold }
            // Интервал в 2,9 медианы = 2 пропущенных кадра (между ними должно было быть ещё 2 метки).
            val dropped = drops.sumOf { (Math.round(it.toDouble() / median) - 1).toInt().coerceAtLeast(1) }
            val span = timestampsNs.last() - timestampsNs.first()
            return Outcome.Computed(
                FrameTimingStats(
                    frameCount = timestampsNs.size,
                    medianIntervalNs = median,
                    meanIntervalNs = positive.average(),
                    minIntervalNs = sorted.first(),
                    maxIntervalNs = sorted.last(),
                    fpsFromMedian = NANOS_PER_SECOND / median,
                    fpsFromSpan = if (span > 0) (timestampsNs.size - 1) * NANOS_PER_SECOND / span else 0.0,
                    dropEvents = drops.size,
                    estimatedDroppedFrames = dropped,
                    nonMonotonicCount = nonMonotonic,
                ),
            )
        }
    }

    sealed interface Outcome {
        data class Computed(val stats: FrameTimingStats) : Outcome
        data class Insufficient(val timestampCount: Int) : Outcome

        fun toJson(): JSONObject = when (this) {
            is Computed -> stats.toJson().put("status", "computed")
            is Insufficient -> JSONObject().put("status", "insufficient").put("frames", timestampCount)
        }
    }
}
