package dev.fasticamera.probe.camera.dual

import android.os.SystemClock
import dev.fasticamera.probe.camera.ManualExposure
import org.json.JSONArray
import org.json.JSONObject

/** Порядок подъёма камер: кто стартует первым, пока вторая ещё закрыта. */
enum class BringUpOrder(val wireName: String) {
    MAIN_FIRST("main_first"),
    WIDE_FIRST("wide_first"),
    ;

    companion object {
        fun fromWire(raw: String): BringUpOrder = entries.firstOrNull { it.wireName == raw }
            ?: throw IllegalArgumentException("неизвестный order '$raw' (${entries.joinToString("|") { it.wireName }})")
    }
}

/** Одна ступень замера: ручная выдержка и ISO, одинаковые для обеих камер. */
data class ExposureStep(val exposureNs: Long, val iso: Int) {
    companion object {
        /** Список `выдержка_нс:iso` через запятую; любая неточность — исключение с исходным текстом. */
        fun parseList(raw: List<String>): List<ExposureStep> = raw.map { item ->
            val parts = item.split(':')
            val exposureNs = parts.getOrNull(0)?.toLongOrNull()
            val iso = parts.getOrNull(1)?.toIntOrNull()
            require(parts.size == 2 && exposureNs != null && iso != null && exposureNs > 0 && iso > 0) {
                "ступень '$item': ожидалось выдержка_нс:iso, например 2000000:100"
            }
            ExposureStep(exposureNs, iso)
        }
    }
}

/** Параметры замера чувствительности (из аргументов сценария). */
data class SensitivityPlan(
    val steps: List<ExposureStep>,
    val settleFrames: Int,
    val sampleFrames: Int,
    val fovFraction: Double,
    val order: BringUpOrder,
)

/** Итог одной камеры на одной ступени: средние по каналам Байера [R, Gr, Gb, B] до и после вычета чёрного. */
class CameraStepStats(
    val framesCollected: Int,
    val framesUsed: Int,
    val rawMeans: DoubleArray,
    val blackLevels: DoubleArray,
    val blackSource: String,
    val blackSubtractedMeans: DoubleArray,
    val clippedFraction: Double,
    val clippedFractionMaxChannel: Double,
    val actualExposureNs: Long?,
    val actualIso: Int?,
    val afState: Int?,
)

/** Чистая сводка кадров одной камеры: вычет чёрного, клиппинг, фактические выдержка/ISO. */
object SensitivityAnalysis {
    /** Допуск при сверке заказанной и применённой выдержки/ISO: HAL округляет значения. */
    const val APPLIED_TOLERANCE = 0.02

    fun isApplied(info: FrameResultInfo?, step: ExposureStep): Boolean = info != null &&
        info.exposureTimeNs != null && info.sensitivityIso != null &&
        SensitivityGeometry.nearlyEqual(info.exposureTimeNs.toDouble(), step.exposureNs.toDouble(), APPLIED_TOLERANCE) &&
        SensitivityGeometry.nearlyEqual(info.sensitivityIso.toDouble(), step.iso.toDouble(), APPLIED_TOLERANCE)

    /**
     * [matched] — кадры, чей результат подтвердил нужные значения; [collected] — всего собрано кадров.
     * null, если подходящих кадров нет.
     */
    fun summarize(
        matched: List<Pair<RegionFrameSample, FrameResultInfo>>,
        collected: Int,
        optics: SensorOptics,
        latestAfState: Int?,
    ): CameraStepStats? {
        if (matched.isEmpty()) return null
        val channels = BayerChannel.entries
        val readoutMeans = DoubleArray(RawRegionSampler.CHANNELS) { index -> matched.map { it.first.channelMeans[index] }.average() }
        val dynamic = matched.map { it.second.dynamicBlackLevel }
        val useDynamic = dynamic.all { it != null && it.size == RawRegionSampler.CHANNELS }
        val readoutBlack = DoubleArray(RawRegionSampler.CHANNELS) { index ->
            if (useDynamic) dynamic.map { it!![index].toDouble() }.average() else optics.blackLevelByReadoutIndex[index].toDouble()
        }
        val totalPixels = matched.sumOf { it.first.pixelsPerChannel.toLong() }
        val clippedPerChannel = DoubleArray(RawRegionSampler.CHANNELS) { index ->
            matched.sumOf { it.first.clippedPerChannel[index].toLong() }.toDouble() / totalPixels
        }
        val raw = DoubleArray(channels.size) { readoutMeans[optics.readoutIndexOf(channels[it])] }
        val black = DoubleArray(channels.size) { readoutBlack[optics.readoutIndexOf(channels[it])] }
        return CameraStepStats(
            framesCollected = collected,
            framesUsed = matched.size,
            rawMeans = raw,
            blackLevels = black,
            blackSource = if (useDynamic) "dynamic_black_level" else "static_black_level_pattern",
            blackSubtractedMeans = DoubleArray(channels.size) { raw[it] - black[it] },
            clippedFraction = clippedPerChannel.average(),
            clippedFractionMaxChannel = clippedPerChannel.max(),
            actualExposureNs = matched.mapNotNull { it.second.exposureTimeNs }.sorted().let { it[it.size / 2] },
            actualIso = matched.mapNotNull { it.second.sensitivityIso }.sorted().let { it[it.size / 2] },
            afState = latestAfState,
        )
    }

    /** Среднее зелёного (Gr, Gb) после вычета чёрного. */
    fun greenMean(stats: CameraStepStats): Double =
        (stats.blackSubtractedMeans[BayerChannel.GR.ordinal] + stats.blackSubtractedMeans[BayerChannel.GB.ordinal]) / 2.0
}

/** Одна камера замера: поток, статические факты, область и сборщик кадров. */
class SensitivitySubject(
    val rig: CameraRig,
    val optics: SensorOptics,
    val region: PixelRegion,
    val opticalCenter: OpticalCenter,
    val sampler: RawRegionSampler,
    val rawWidth: Int,
    val rawHeight: Int,
)

/**
 * Замер «одна и та же экспозиция на двух камерах → разница сигналов в EV» в совпадающем поле зрения.
 * Кадры допускаются в среднее только после того, как результат кадра подтвердил применённые выдержку и ISO
 * (HAL применяет новую экспозицию с задержкой в несколько кадров).
 */
class SensitivityMeasurement(
    private val main: SensitivitySubject,
    private val wide: SensitivitySubject,
    private val frameDurationNs: Long,
    private val plan: SensitivityPlan,
) {
    private val subjects = listOf(main, wide)

    fun run(): JSONObject {
        val steps = JSONArray()
        for (step in plan.steps) steps.put(measureStep(step))
        return JSONObject().put("steps", steps).put("static", staticFacts())
    }

    private fun measureStep(step: ExposureStep): JSONObject {
        val report = JSONObject().put("requested_exposure_ns", step.exposureNs).put("requested_iso", step.iso)
        val manual = ManualExposure(step.exposureNs, step.iso, frameDurationNs)
        val framesAtChange = subjects.map { it.rig.framesReceived }
        subjects.forEach { it.rig.applyManualExposure(manual) }
        val appliedAfterMs = awaitApplied(step)
        report.put("applied_after_ms", JSONObject().put("main", appliedAfterMs[0] ?: JSONObject.NULL).put("wide", appliedAfterMs[1] ?: JSONObject.NULL))
        awaitFramesSince(subjects.map { it.rig.framesReceived }, plan.settleFrames)
        report.put("frames_during_settle", JSONArray(subjects.mapIndexed { i, s -> s.rig.framesReceived - framesAtChange[i] }))

        subjects.forEach { it.sampler.startCollecting() }
        awaitSamples()
        val collected = subjects.map { it.sampler.stopCollecting() }
        Thread.sleep(RESULT_GRACE_MS) // результаты последних кадров приходят чуть позже самих кадров
        val stats = subjects.mapIndexed { index, subject -> summarizeSubject(subject, step, collected[index]) }

        report.put("main", stats[0]?.let(::statsJson) ?: JSONObject.NULL)
        report.put("wide", stats[1]?.let(::statsJson) ?: JSONObject.NULL)
        report.put("sampler_failures", JSONArray(subjects.map { it.sampler.failureOrNull() ?: JSONObject.NULL }))
        val clipped = stats.any { it != null && it.clippedFractionMaxChannel > CLIPPED_LIMIT }
        val status = when {
            stats.any { it == null } -> "no_matched_frames"
            clipped -> "clipped"
            else -> "ok"
        }
        report.put("status", status)
        if (status == "ok") report.put("ev_main_minus_wide", evJson(stats[0]!!, stats[1]!!))
        android.util.Log.i(LOG_TAG, "sensitivity_step $report")
        return report
    }

    private fun summarizeSubject(subject: SensitivitySubject, step: ExposureStep, collected: List<RegionFrameSample>): CameraStepStats? {
        val matched = collected.mapNotNull { sample ->
            subject.rig.resultInfoFor(sample.timestampNs)?.takeIf { SensitivityAnalysis.isApplied(it, step) }?.let { sample to it }
        }.take(plan.sampleFrames)
        return SensitivityAnalysis.summarize(matched, collected.size, subject.optics, subject.rig.latestResultInfo()?.afState)
    }

    /** Ждёт, пока последний результат каждой камеры покажет нужную экспозицию; мс до подтверждения или null по таймауту. */
    private fun awaitApplied(step: ExposureStep): List<Long?> {
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        val appliedMs = arrayOfNulls<Long>(subjects.size)
        while (appliedMs.any { it == null } && elapsedMs(startedAtNs) < APPLY_TIMEOUT_MS) {
            subjects.forEachIndexed { index, subject ->
                if (appliedMs[index] == null && SensitivityAnalysis.isApplied(subject.rig.latestResultInfo(), step)) {
                    appliedMs[index] = elapsedMs(startedAtNs)
                }
            }
            Thread.sleep(POLL_MS)
        }
        return appliedMs.toList()
    }

    private fun awaitFramesSince(baseline: List<Int>, frames: Int) {
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        while (subjects.indices.any { subjects[it].rig.framesReceived - baseline[it] < frames } &&
            elapsedMs(startedAtNs) < FRAMES_TIMEOUT_MS
        ) {
            Thread.sleep(POLL_MS)
        }
    }

    /** Собираем с запасом: часть кадров в начале может ещё нести старую экспозицию и будет отброшена. */
    private fun awaitSamples() {
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        val wanted = plan.sampleFrames + SAMPLE_MARGIN_FRAMES
        while (subjects.any { it.sampler.collectedCount < wanted } && elapsedMs(startedAtNs) < COLLECT_TIMEOUT_MS) {
            Thread.sleep(POLL_MS)
        }
    }

    private fun elapsedMs(startedAtNs: Long): Long = (SystemClock.elapsedRealtimeNanos() - startedAtNs) / NS_PER_MS

    private fun channelsJson(values: DoubleArray) =
        JSONObject().also { json -> BayerChannel.entries.forEach { json.put(it.wireName, values[it.ordinal]) } }

    private fun statsJson(stats: CameraStepStats): JSONObject = JSONObject()
        .put("frames_collected", stats.framesCollected)
        .put("frames_used", stats.framesUsed)
        .put("actual_exposure_ns", stats.actualExposureNs ?: JSONObject.NULL)
        .put("actual_iso", stats.actualIso ?: JSONObject.NULL)
        .put("af_state", stats.afState?.let(::afStateName) ?: JSONObject.NULL)
        .put("raw_means", channelsJson(stats.rawMeans))
        .put("black_levels", channelsJson(stats.blackLevels))
        .put("black_source", stats.blackSource)
        .put("means_black_subtracted", channelsJson(stats.blackSubtractedMeans))
        .put("clipped_fraction", stats.clippedFraction)
        .put("clipped_fraction_max_channel", stats.clippedFractionMaxChannel)

    private fun evJson(mainStats: CameraStepStats, wideStats: CameraStepStats): JSONObject {
        val perChannel = DoubleArray(BayerChannel.entries.size) {
            SensitivityGeometry.evMainMinusWide(mainStats.blackSubtractedMeans[it], wideStats.blackSubtractedMeans[it])
        }
        return channelsJson(perChannel).put(
            "green_avg",
            SensitivityGeometry.evMainMinusWide(SensitivityAnalysis.greenMean(mainStats), SensitivityAnalysis.greenMean(wideStats)),
        )
    }

    private fun regionJson(subject: SensitivitySubject): JSONObject = JSONObject()
        .put("left_top_width_height", JSONArray(listOf(subject.region.left, subject.region.top, subject.region.width, subject.region.height)))
        .put("optical_center_fraction", JSONArray(listOf(subject.opticalCenter.fractionX, subject.opticalCenter.fractionY)))
        .put("optical_center_source", subject.opticalCenter.source.wireName)

    /** Статика и ожидания: что должно получиться только от диафрагмы и от диафрагмы + площади пикселя. */
    private fun staticFacts(): JSONObject {
        val pitchMain = main.optics.outputPixelPitchUm(main.rawWidth)
        val pitchWide = wide.optics.outputPixelPitchUm(wide.rawWidth)
        val apertureEv = SensitivityGeometry.expectedEvFromAperture(main.optics.aperture, wide.optics.aperture)
        val pitchEv = SensitivityGeometry.expectedEvFromPixelPitch(pitchMain, pitchWide)
        return JSONObject()
            .put("main", main.optics.toJson(main.rawWidth))
            .put("wide", wide.optics.toJson(wide.rawWidth))
            .put("zoom_ratio_wide", zoomRatioWide(main.optics, wide.optics))
            .put("fov_fraction_main", plan.fovFraction)
            .put("fov_fraction_wide", plan.fovFraction * zoomRatioWide(main.optics, wide.optics))
            .put("regions", JSONObject().put("main", regionJson(main)).put("wide", regionJson(wide)))
            .put("expected_ev_aperture_only", apertureEv)
            .put("expected_ev_aperture_and_pixel_area", apertureEv + pitchEv)
            .put("pixel_pitch_ev_term", pitchEv)
            .put(
                "assumption",
                "координаты RAW-буфера = долям активного массива (без смещения биннинга), центр из калибровки в долях " +
                    "preCorrectionActiveArray; выборка — каждый 4-й блок 2x2",
            )
    }

    companion object {
        /** Отношение эквивалентных фокусных ширика к основной: во сколько раз ширик «шире». */
        fun zoomRatioWide(mainOptics: SensorOptics, wideOptics: SensorOptics): Double =
            wideOptics.equivalentFocalLengthMm / mainOptics.equivalentFocalLengthMm

        private const val LOG_TAG = "FasticameraProbe"
        private const val CLIPPED_LIMIT = 0.01
        private const val SAMPLE_MARGIN_FRAMES = 10
        private const val APPLY_TIMEOUT_MS = 4_000L
        private const val FRAMES_TIMEOUT_MS = 4_000L
        private const val COLLECT_TIMEOUT_MS = 10_000L
        private const val RESULT_GRACE_MS = 300L
        private const val POLL_MS = 20L
        private const val NS_PER_MS = 1_000_000L
    }
}
