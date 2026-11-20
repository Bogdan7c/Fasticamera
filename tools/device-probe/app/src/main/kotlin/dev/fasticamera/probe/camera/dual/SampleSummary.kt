package dev.fasticamera.probe.camera.dual

import org.json.JSONObject

/**
 * Итог ряда измерений (миллисекунды, мкА и т. п.): медиана, минимум, максимум, среднее.
 * Пустой ряд — отдельный исход, а не нули: «не измеряли» нельзя путать с «0 мс».
 */
data class SampleSummary(
    val count: Int,
    val median: Double,
    val min: Double,
    val max: Double,
    val mean: Double,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("count", count).put("median", median).put("min", min).put("max", max).put("mean", mean)

    companion object {
        /** null — измерений не было. Для чётного числа значений медиана — среднее двух центральных. */
        fun of(values: List<Double>): SampleSummary? {
            if (values.isEmpty()) return null
            val sorted = values.sorted()
            val middle = sorted.size / 2
            val median = if (sorted.size % 2 == 1) sorted[middle] else (sorted[middle - 1] + sorted[middle]) / 2.0
            return SampleSummary(sorted.size, median, sorted.first(), sorted.last(), sorted.average())
        }

        fun toJsonOrNull(values: List<Double>): Any = of(values)?.toJson() ?: JSONObject.NULL
    }
}
