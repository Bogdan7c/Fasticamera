package dev.fasticamera.probe.camera.threea

import android.media.Image
import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer

/**
 * Статистика упакованного RAW10 по четырём CFA-каналам (позиции 2x2 решётки Байера).
 * Цель: показать, что RAW с вспомогательным потоком и без него одинаков по уровням и средним.
 *
 * RAW10 в Android: 4 пикселя = 5 байт; первые 4 байта — старшие 8 бит пикселей, пятый — младшие 2 бита каждого.
 * Берётся разреженная сетка (шаг [GRID_STEP]) по всему кадру плюс плотный блок в центре.
 * Анализируется каждый [everyNth] кадр после пропуска [skipFirst] кадров (разгон потока), не более [maxSamples].
 */
class ThreeARaw10Probe(private val everyNth: Int, private val skipFirst: Int, private val maxSamples: Int) {

    private val samples = mutableListOf<JSONObject>()
    private val frameChannelMeans = mutableListOf<DoubleArray>()
    private var seen = 0

    @Synchronized
    fun maybeAnalyze(image: Image) {
        seen += 1
        if (seen <= skipFirst || (seen - skipFirst) % everyNth != 0 || samples.size >= maxSamples) return
        val plane = image.planes.firstOrNull() ?: return
        val buffer = plane.buffer.duplicate()
        val whole = Accumulator()
        for (y in 0 until image.height - 1 step GRID_STEP) {
            for (x in 0 until image.width - 1 step GRID_STEP) accumulate2x2(buffer, plane.rowStride, x, y, whole)
        }
        val centerLeft = (image.width / 2 - CENTER_BLOCK / 2) and 1.inv()
        val centerTop = (image.height / 2 - CENTER_BLOCK / 2) and 1.inv()
        val center = Accumulator()
        for (y in centerTop until centerTop + CENTER_BLOCK step 2) {
            for (x in centerLeft until centerLeft + CENTER_BLOCK step 2) accumulate2x2(buffer, plane.rowStride, x, y, center)
        }
        frameChannelMeans += whole.means()
        samples += JSONObject().put("timestamp_ns", image.timestamp).put("whole", whole.toJson()).put("center", center.toJson())
    }

    /** Сводка по всем проанализированным кадрам: среднее средних по каналам и разброс между кадрами. */
    @Synchronized
    fun toJson(): JSONObject {
        val channelSummary = JSONArray()
        for (channel in 0 until CHANNELS) {
            val perFrame = frameChannelMeans.map { it[channel] }
            val mean = if (perFrame.isEmpty()) Double.NaN else perFrame.average()
            val std = if (perFrame.size < 2) 0.0 else Math.sqrt(perFrame.sumOf { (it - mean) * (it - mean) } / (perFrame.size - 1))
            channelSummary.put(JSONObject().put("channel", channel).put("mean_of_frame_means", mean).put("std_between_frames", std))
        }
        val globalMin = samples.minOfOrNull { s -> (0 until CHANNELS).minOf { s.getJSONObject("whole").getJSONArray("min").getInt(it) } }
        val globalMax = samples.maxOfOrNull { s -> (0 until CHANNELS).maxOf { s.getJSONObject("whole").getJSONArray("max").getInt(it) } }
        return JSONObject().put("analyzed", samples.size).put("channels_gr_r_b_gb_order_is_cfa_position", channelSummary)
            .put("global_min", globalMin ?: JSONObject.NULL).put("global_max", globalMax ?: JSONObject.NULL)
            .put("samples", JSONArray(samples))
    }

    /** Читает блок 2x2 с левым верхним углом (x, y); x чётный, поэтому пары не пересекают границу 5-байтовой группы. */
    private fun accumulate2x2(buffer: ByteBuffer, rowStride: Int, x: Int, y: Int, target: Accumulator) {
        for (dy in 0..1) for (dx in 0..1) {
            val px = x + dx
            val rowBase = (y + dy) * rowStride + (px / 4) * 5
            val high = buffer.get(rowBase + px % 4).toInt() and 0xFF
            val low = (buffer.get(rowBase + 4).toInt() and 0xFF) shr ((px % 4) * 2) and 0x3
            target.add(dy * 2 + dx, (high shl 2) or low)
        }
    }

    private class Accumulator {
        val sum = DoubleArray(CHANNELS)
        val squares = DoubleArray(CHANNELS)
        val count = IntArray(CHANNELS)
        val min = IntArray(CHANNELS) { Int.MAX_VALUE }
        val max = IntArray(CHANNELS) { Int.MIN_VALUE }

        fun add(channel: Int, value: Int) {
            sum[channel] += value; squares[channel] += value.toDouble() * value; count[channel] += 1
            if (value < min[channel]) min[channel] = value
            if (value > max[channel]) max[channel] = value
        }

        fun means(): DoubleArray = DoubleArray(CHANNELS) { sum[it] / count[it] }

        fun toJson(): JSONObject {
            val mean = means()
            val std = DoubleArray(CHANNELS) { Math.sqrt(squares[it] / count[it] - mean[it] * mean[it]) }
            return JSONObject().put("mean", JSONArray(mean.toList())).put("std", JSONArray(std.toList()))
                .put("min", JSONArray(min.toList())).put("max", JSONArray(max.toList()))
        }
    }

    private companion object {
        const val CHANNELS = 4
        const val GRID_STEP = 16
        const val CENTER_BLOCK = 256
    }
}
