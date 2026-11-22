package dev.fasticamera.probe.camera.flash

import org.json.JSONArray
import org.json.JSONObject

/** Фон без вспышки: медиана средних кадров и средние по полосам. */
class Ambient(val mean: Double, val bandMeans: DoubleArray)

/** Итог одного одиночного выстрела вспышки. [peakGain] — прирост среднего самого яркого кадра окна над фоном. */
class ShotSummary(val label: String, val exposureNs: Long, val peakGain: Double?, val json: JSONObject)

/** Итог фазы torch на одной выдержке. [steadyGain] — установившийся прирост над фоном. */
class TorchSummary(val exposureNs: Long, val steadyGain: Double?, val json: JSONObject)

/**
 * Чистый анализ кадров вспышки: что считать «освещённым», как выглядит выстрел, как ведёт себя torch.
 * Ничего не знает ни о камере, ни о потоках — работает со списками [FlashFrame]; поэтому покрыт JVM-тестами.
 */
object FlashAnalysis {
    /** Кадр считается освещённым, если превышает фон минимум на столько единиц RAW10... */
    const val LIT_MIN_DELTA_DN = 1.0

    /** ...и минимум на такую долю фона. */
    const val LIT_RELATIVE_DELTA = 0.25

    /** Torch считается вышедшим на уровень, когда достиг этой доли установившегося прироста. */
    private const val RAMP_DONE_FRACTION = 0.9

    /** Доля прироста, ниже которой torch считается погасшим. */
    private const val RAMP_DOWN_FRACTION = 0.1

    /** Сколько последних кадров torch считаются «установившимися». */
    private const val STEADY_TAIL_FRAMES = 15

    /** Интервал между кадрами длиннее медианы во столько раз считается нарушением кадровой частоты. */
    private const val CADENCE_BREAK_FACTOR = 1.5

    private const val NS_PER_MS = 1_000_000.0

    fun ambientOf(frames: List<FlashFrame>): Ambient? {
        val profiles = frames.mapNotNull { it.profile }
        if (profiles.isEmpty()) return null
        val bandCount = profiles.first().bandMeans.size
        val bands = DoubleArray(bandCount) { band -> profiles.map { it.bandMeans[band] }.average() }
        return Ambient(median(profiles.map { it.overallMean }), bands)
    }

    fun isLit(profile: RawBandProfile, ambient: Ambient): Boolean =
        profile.overallMean > ambient.mean + maxOf(LIT_MIN_DELTA_DN, LIT_RELATIVE_DELTA * maxOf(ambient.mean, 0.0))

    /** Свёртка последовательности значений в пары [значение, сколько раз подряд] — для FLASH_STATE и режимов сенсора. */
    fun runLengths(values: List<Any?>): JSONArray {
        val result = JSONArray()
        var index = 0
        while (index < values.size) {
            var end = index
            while (end + 1 < values.size && values[end + 1] == values[index]) end += 1
            result.put(JSONArray().put(values[index] ?: JSONObject.NULL).put(end - index + 1))
            index = end + 1
        }
        return result
    }

    /**
     * Сводка одного выстрела. [window] — кадры от -pre до +post относительно кадра одиночного запроса [oneShotFrameNumber];
     * [preFrames] — сколько кадров до выстрела; [submitNs]/[oneShotArrivedNs]/[oneShotTimestampNs] — моменты в шкале elapsedRealtimeNanos.
     */
    fun summarizeShot(
        label: String,
        round: Int,
        window: List<FlashFrame>,
        oneShotFrameNumber: Long,
        preFrames: Int,
        submitNs: Long,
        oneShotArrivedNs: Long,
    ): ShotSummary {
        val oneShot = window.firstOrNull { it.result.frameNumber == oneShotFrameNumber }
        // Ключ группировки — ЗАКАЗАННАЯ выдержка (HAL округляет фактическую: 16 000 000 -> 15 992 884).
        val exposure = oneShot?.result?.tag?.requestedExposureNs ?: 0L
        // Фон — кадры до выстрела, кроме последнего перед ним (вдруг предвспышка влияет на соседа).
        val ambient = ambientOf(window.filter { it.result.frameNumber < oneShotFrameNumber - 1 && it.result.frameNumber >= oneShotFrameNumber - preFrames })
        val json = JSONObject().put("label", label).put("round", round)
            .put("one_shot_frame", oneShotFrameNumber).put("requested_exposure_ns", exposure)
            .put("exposure_ns", oneShot?.result?.exposureNs ?: JSONObject.NULL)
            .put("flash_state_by_offset", offsetMap(window, oneShotFrameNumber) { it.result.flashState })
            .put("flash_state_runs", runLengths(window.map { it.result.flashState }))
            .put("sensor_mode_runs", runLengths(window.map { it.result.sensorMode }))
            .put("cadence", cadence(window, oneShotFrameNumber))
            .put("submit_to_one_shot_result_ms", (oneShotArrivedNs - submitNs) / NS_PER_MS)
        oneShot?.let { json.put("submit_to_one_shot_exposure_start_ms", (it.result.sensorTimestampNs - submitNs) / NS_PER_MS) }
        if (ambient == null) return ShotSummary(label, exposure, null, json.put("error", "нет кадров фона до выстрела"))
        json.put("ambient_mean", ambient.mean).put("ambient_bands", roundedArray(ambient.bandMeans))
        val lit = window.filter { frame -> frame.profile?.let { isLit(it, ambient) } == true }
        val peak = window.filter { it.profile != null }.maxByOrNull { it.profile!!.overallMean }
        json.put("lit_offsets", JSONArray(lit.map { it.result.frameNumber - oneShotFrameNumber }))
            .put("gain_by_offset", offsetMap(window, oneShotFrameNumber) { frame -> frame.profile?.let { round1(it.overallMean - ambient.mean) } })
        lit.firstOrNull()?.let { first ->
            json.put("first_lit_offset", first.result.frameNumber - oneShotFrameNumber)
                .put("submit_to_first_lit_exposure_start_ms", (first.result.sensorTimestampNs - submitNs) / NS_PER_MS)
        }
        val peakGain = peak?.profile?.let { it.overallMean - ambient.mean }
        peak?.profile?.let { profile ->
            json.put("peak_offset", peak.result.frameNumber - oneShotFrameNumber).put("peak_gain", round1(profile.overallMean - ambient.mean))
                .put("peak_bands", roundedArray(profile.bandMeans))
                .put("peak_bands_minus_ambient", roundedArray(DoubleArray(profile.bandMeans.size) { profile.bandMeans[it] - ambient.bandMeans[it] }))
                .put("peak_clip_fraction", profile.clipFraction)
        }
        return ShotSummary(label, exposure, peakGain, json)
    }

    /**
     * Сводка torch. [onFrames] — кадры с включённым torch (с первого кадра с меткой torch), [offFrames] — после выключения.
     * Разгон = сколько первых кадров не дошли до 90% установившегося прироста; затухание — сколько кадров ещё выше 10%.
     */
    fun summarizeTorch(exposureNs: Long, ambient: Ambient, onFrames: List<FlashFrame>, offFrames: List<FlashFrame>): TorchSummary {
        val onMeans = onFrames.mapNotNull { it.profile?.overallMean }
        val steadyTail = onMeans.takeLast(STEADY_TAIL_FRAMES)
        val json = JSONObject().put("exposure_ns", exposureNs).put("ambient_mean", ambient.mean)
            .put("flash_state_on_runs", runLengths(onFrames.map { it.result.flashState }))
            .put("flash_state_off_runs", runLengths(offFrames.map { it.result.flashState }))
            .put("flash_mode_on_runs", runLengths(onFrames.map { it.result.flashMode }))
            .put("on_gains", roundedArray(onMeans.map { it - ambient.mean }.toDoubleArray()))
            .put("off_gains", roundedArray(offFrames.mapNotNull { it.profile?.overallMean }.map { it - ambient.mean }.toDoubleArray()))
            .put("sensor_mode_runs", runLengths((onFrames + offFrames).map { it.result.sensorMode }))
            .put("cadence_on", cadence(onFrames, onFrames.firstOrNull()?.result?.frameNumber ?: 0L))
        if (steadyTail.isEmpty()) return TorchSummary(exposureNs, null, json.put("error", "нет кадров torch с RAW"))
        val steadyGain = median(steadyTail) - ambient.mean
        json.put("steady_gain", round1(steadyGain)).put("steady_bands", roundedArray(onFrames.last { it.profile != null }.profile!!.bandMeans))
        if (steadyGain < LIT_MIN_DELTA_DN) {
            return TorchSummary(exposureNs, steadyGain, json.put("note", "torch не поднял яркость: разгон и затухание не определены"))
        }
        val rampUp = onMeans.indexOfFirst { it - ambient.mean >= RAMP_DONE_FRACTION * steadyGain }
        json.put("ramp_up_frames", rampUp)
        val offMeans = offFrames.mapNotNull { it.profile?.overallMean }
        val rampDown = offMeans.indexOfFirst { it - ambient.mean <= RAMP_DOWN_FRACTION * steadyGain }
        json.put("ramp_down_frames", rampDown)
        json.put("off_final_gain", offMeans.lastOrNull()?.let { round1(it - ambient.mean) } ?: JSONObject.NULL)
        return TorchSummary(exposureNs, steadyGain, json)
    }

    /** Интервалы между кадрами и пропущенные номера: ловит сброс кадров и смену режима сенсора. */
    fun cadence(frames: List<FlashFrame>, anchorFrameNumber: Long): JSONObject {
        val sorted = frames.sortedBy { it.result.frameNumber }
        val intervalsMs = sorted.zipWithNext { a, b -> (b.result.sensorTimestampNs - a.result.sensorTimestampNs) / NS_PER_MS }
        val medianInterval = if (intervalsMs.isEmpty()) 0.0 else median(intervalsMs)
        val missing = if (sorted.isEmpty()) emptyList() else
            (sorted.first().result.frameNumber..sorted.last().result.frameNumber).filter { number -> sorted.none { it.result.frameNumber == number } }
        val breaks = sorted.zipWithNext().filter { (a, b) ->
            (b.result.sensorTimestampNs - a.result.sensorTimestampNs) / NS_PER_MS > CADENCE_BREAK_FACTOR * medianInterval
        }.map { (_, b) -> b.result.frameNumber - anchorFrameNumber }
        return JSONObject().put("median_interval_ms", round1(medianInterval))
            .put("max_interval_ms", intervalsMs.maxOrNull()?.let(::round1) ?: JSONObject.NULL)
            .put("missing_frame_numbers_offsets", JSONArray(missing.map { it - anchorFrameNumber }))
            .put("long_interval_before_offsets", JSONArray(breaks))
    }

    fun median(values: List<Double>): Double {
        val sorted = values.sorted()
        return if (sorted.size % 2 == 1) sorted[sorted.size / 2] else (sorted[sorted.size / 2 - 1] + sorted[sorted.size / 2]) / 2.0
    }

    private fun offsetMap(frames: List<FlashFrame>, anchor: Long, value: (FlashFrame) -> Any?): JSONObject {
        val json = JSONObject()
        for (frame in frames) json.put((frame.result.frameNumber - anchor).toString(), value(frame) ?: JSONObject.NULL)
        return json
    }

    private fun roundedArray(values: DoubleArray): JSONArray = JSONArray(values.map(::round1))

    private fun round1(value: Double): Double = Math.round(value * 10.0) / 10.0
}
