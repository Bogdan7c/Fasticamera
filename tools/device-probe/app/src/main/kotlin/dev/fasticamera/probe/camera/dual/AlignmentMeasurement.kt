package dev.fasticamera.probe.camera.dual

import android.hardware.camera2.CaptureResult
import android.os.SystemClock
import dev.fasticamera.probe.camera.ManualExposure
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import kotlin.math.abs
import kotlin.math.ln

/** Параметры замера совмещения кадров (из аргументов сценария). */
data class AlignmentPlan(
    val pairs: Int,
    /** Коэффициент уменьшения в СЫРЫХ пикселях (чётный): 4 = блок 4x4 raw. */
    val downsample: Int,
    val settleMs: Long,
    val mainExposureNs: Long,
    val iso: Int,
    val order: BringUpOrder,
    /** Запас вокруг эквивалентного поля ширика: размер вырезки = k * (1 + marginFraction). */
    val marginFraction: Double,
    val outputDirectory: File,
)

/** Одна камера замера совмещения: поток, оптика, область вырезки и кольцо сырых кадров. */
class AlignmentSubject(
    val label: String,
    val rig: CameraRig,
    val optics: SensorOptics,
    val region: PixelRegion,
    val opticalCenter: OpticalCenter,
    val ring: AlignmentFrameRing,
    val rawWidth: Int,
    val rawHeight: Int,
    val exposure: ExposureStep,
)

/**
 * Замер геометрического совмещения: обе камеры стримят одновременно, ждём фокус, затем [AlignmentPlan.pairs] раз
 * берём пару кадров с ближайшими метками сенсора и пишем зелёные плоскости на диск для анализа на ПК
 * (`pc/align_analyze.py`). Здесь нет оценки сдвига — только честный сбор данных и метаданных кадров.
 */
class AlignmentMeasurement(
    private val main: AlignmentSubject,
    private val wide: AlignmentSubject,
    private val frameDurationNs: Long,
    private val plan: AlignmentPlan,
) {
    private val subjects = listOf(main, wide)

    fun run(): JSONObject {
        plan.outputDirectory.mkdirs()
        val report = JSONObject()
        subjects.forEach { it.rig.applyManualExposure(ManualExposure(it.exposure.exposureNs, it.exposure.iso, frameDurationNs)) }
        report.put("applied_after_ms", awaitExposureApplied())
        Thread.sleep(plan.settleMs)
        report.put("af_wait", awaitFocused())
        val pairReports = JSONArray()
        repeat(plan.pairs) { index ->
            pairReports.put(capturePair(index))
            Thread.sleep(PAUSE_BETWEEN_PAIRS_MS)
        }
        return report.put("pairs", pairReports)
            .put("ring_failures", JSONArray(subjects.map { it.ring.failureOrNull() ?: JSONObject.NULL }))
            .put("static", staticFacts())
    }

    /** Ждёт, пока результаты обеих камер подтвердят заказанную экспозицию; мс до подтверждения или null по таймауту. */
    private fun awaitExposureApplied(): JSONObject {
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        val appliedMs = arrayOfNulls<Long>(subjects.size)
        while (appliedMs.any { it == null } && elapsedMs(startedAtNs) < APPLY_TIMEOUT_MS) {
            subjects.forEachIndexed { index, subject ->
                if (appliedMs[index] == null && SensitivityAnalysis.isApplied(subject.rig.latestResultInfo(), subject.exposure)) {
                    appliedMs[index] = elapsedMs(startedAtNs)
                }
            }
            Thread.sleep(POLL_MS)
        }
        return JSONObject().put("main", appliedMs[0] ?: JSONObject.NULL).put("wide", appliedMs[1] ?: JSONObject.NULL)
    }

    /** Ждёт, пока обе камеры одновременно покажут PASSIVE_FOCUSED; по таймауту идём дальше и честно сообщаем. */
    private fun awaitFocused(): JSONObject {
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        var reached = false
        while (!reached && elapsedMs(startedAtNs) < AF_TIMEOUT_MS) {
            reached = subjects.all { it.rig.latestResultInfo()?.afState == AF_PASSIVE_FOCUSED }
            if (!reached) Thread.sleep(POLL_MS)
        }
        return JSONObject().put("reached", reached).put("waited_ms", elapsedMs(startedAtNs))
            .put("af_state_main", afName(main)).put("af_state_wide", afName(wide))
    }

    private fun afName(subject: AlignmentSubject): Any =
        subject.rig.latestResultInfo()?.afState?.let(::afStateName) ?: JSONObject.NULL

    /** Заполняет кольца, замораживает их, берёт ближайшую по времени пару и пишет оба файла. */
    private fun capturePair(index: Int): JSONObject {
        val report = JSONObject().put("index", index)
        subjects.forEach { it.ring.thaw() }
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        while (subjects.any { it.ring.storedCount < RING_CAPACITY } && elapsedMs(startedAtNs) < FILL_TIMEOUT_MS) {
            Thread.sleep(POLL_MS)
        }
        val copies = subjects.map { it.ring.freeze() }
        Thread.sleep(RESULT_GRACE_MS) // результаты кадров приходят чуть позже самих кадров
        val best = closestPair(copies[0], copies[1]) ?: return report.put("error", "нет кадров: main=${copies[0].size}, wide=${copies[1].size}")
        report.put("skew_ns", best.skewNs).put("skew_ms", best.skewNs / NS_PER_MS)
        val frames = JSONObject()
        listOf(main to best.first, wide to best.second).forEach { (subject, copy) ->
            frames.put(subject.label, writeFrame(subject, copy, index))
        }
        return report.put("frames", frames)
    }

    private class BestPair(val first: RawFrameCopy, val second: RawFrameCopy, val skewNs: Long)

    /** Пара (основная, ширик) с минимальной разницей меток сенсора; null, если хоть одно кольцо пусто. */
    private fun closestPair(mainFrames: List<RawFrameCopy>, wideFrames: List<RawFrameCopy>): BestPair? =
        mainFrames.flatMap { m -> wideFrames.map { w -> BestPair(m, w, abs(m.timestampNs - w.timestampNs)) } }.minByOrNull { it.skewNs }

    /** Конвертирует кадр в зелёную плоскость, пишет `.f32` + `.json`; возвращает метаданные кадра для отчёта. */
    private fun writeFrame(subject: AlignmentSubject, copy: RawFrameCopy, pairIndex: Int): JSONObject {
        val info = subject.rig.resultInfoFor(copy.timestampNs)
        val dynamicBlack = info?.dynamicBlackLevel?.takeIf { it.size == RawRegionSampler.CHANNELS }
        val black = DoubleArray(RawRegionSampler.CHANNELS) { dynamicBlack?.get(it)?.toDouble() ?: subject.optics.blackLevelByReadoutIndex[it].toDouble() }
        val greens = GreenLayout(
            subject.optics.readoutIndexOf(BayerChannel.GR), subject.optics.readoutIndexOf(BayerChannel.GB),
        )
        val plane = GreenPlane.extract(copy.bytes, copy.rowStrideBytes, subject.region, greens, black, subject.optics.whiteLevel, plan.downsample)
        val baseName = "pair${pairIndex}_${subject.label}"
        File(plan.outputDirectory, "$baseName.f32").writeBytes(plane.toLittleEndianBytes())
        val array = subject.optics.preCorrectionActiveArray
        val scaleX = array.width().toDouble() / subject.rawWidth
        val scaleY = array.height().toDouble() / subject.rawHeight
        val meta = JSONObject()
            .put("file", "$baseName.f32").put("dtype", "float32_le").put("camera", subject.rig.cameraId)
            .put("width", plane.width).put("height", plane.height)
            .put("downsample_raw_px", plan.downsample)
            .put("raw_buffer_size", JSONArray(listOf(subject.rawWidth, subject.rawHeight)))
            .put("crop_buffer_left_top_width_height", JSONArray(listOf(subject.region.left, subject.region.top, subject.region.width, subject.region.height)))
            .put("buffer_to_active_scale_xy", JSONArray(listOf(scaleX, scaleY)))
            // Координаты в системе preCorrectionActiveArray, в которой записана LENS_INTRINSIC_CALIBRATION.
            .put("crop_origin_active_px", JSONArray(listOf(subject.region.left * scaleX, subject.region.top * scaleY)))
            .put("output_px_step_active_px", JSONArray(listOf(plan.downsample * scaleX, plan.downsample * scaleY)))
            .put("optical_center_active_px", JSONArray(listOf(subject.opticalCenter.fractionX * array.width(), subject.opticalCenter.fractionY * array.height())))
            .put("optical_center_source", subject.opticalCenter.source.wireName)
            .put("sensor_timestamp_ns", copy.timestampNs)
            .put("clipped_fraction", plane.clippedFraction).put("mean_level_black_subtracted", plane.meanLevel)
            .put("black_levels_readout_order", JSONArray(black.toList()))
            .put("black_source", if (dynamicBlack != null) "dynamic_black_level" else "static_black_level_pattern")
            .put("result", resultJson(info))
        File(plan.outputDirectory, "$baseName.json").writeText(meta.toString(1))
        return meta
    }

    private fun resultJson(info: FrameResultInfo?): Any = info?.let {
        JSONObject()
            .put("exposure_ns", it.exposureTimeNs ?: JSONObject.NULL).put("iso", it.sensitivityIso ?: JSONObject.NULL)
            .put("af_state", it.afState?.let(::afStateName) ?: JSONObject.NULL)
            .put("focus_distance_diopters", it.focusDistanceDiopters?.toDouble() ?: JSONObject.NULL)
            .put("lens_state", it.lensState ?: JSONObject.NULL)
    } ?: JSONObject.NULL

    private fun staticFacts(): JSONObject {
        val zoom = SensitivityMeasurement.zoomRatioWide(main.optics, wide.optics)
        val evTerm = SensitivityGeometry.expectedEvFromAperture(main.optics.aperture, wide.optics.aperture) +
            SensitivityGeometry.expectedEvFromPixelPitch(main.optics.outputPixelPitchUm(main.rawWidth), wide.optics.outputPixelPitchUm(wide.rawWidth))
        return JSONObject()
            .put("main", main.optics.toJson(main.rawWidth)).put("wide", wide.optics.toJson(wide.rawWidth))
            .put("zoom_ratio_wide_k", zoom).put("expected_ev_main_minus_wide", evTerm)
            .put("regions", JSONObject().put("main", regionJson(main)).put("wide", regionJson(wide)))
            .put("requested_exposure", JSONObject()
                .put("main", exposureJson(main.exposure)).put("wide", exposureJson(wide.exposure)))
    }

    private fun exposureJson(step: ExposureStep) = JSONObject().put("exposure_ns", step.exposureNs).put("iso", step.iso)

    private fun regionJson(subject: AlignmentSubject) = JSONObject()
        .put("left_top_width_height", JSONArray(listOf(subject.region.left, subject.region.top, subject.region.width, subject.region.height)))
        .put("optical_center_fraction", JSONArray(listOf(subject.opticalCenter.fractionX, subject.opticalCenter.fractionY)))

    private fun elapsedMs(startedAtNs: Long): Long = (SystemClock.elapsedRealtimeNanos() - startedAtNs) / NS_PER_MS.toLong()

    companion object {
        /** Размер кольца на камеру: 4 кадра = 66 мс при 60 к/с, ближайшая пара не хуже ~4 мс по ожиданию. */
        const val RING_CAPACITY = 4

        /** Множитель экспозиции ширика: 2^(EV-разница по диафрагме и площади пикселя), чтобы яркость сошлась. */
        fun wideExposureFactor(mainOptics: SensorOptics, wideOptics: SensorOptics, mainRawWidth: Int, wideRawWidth: Int): Double {
            val ev = SensitivityGeometry.expectedEvFromAperture(mainOptics.aperture, wideOptics.aperture) +
                SensitivityGeometry.expectedEvFromPixelPitch(mainOptics.outputPixelPitchUm(mainRawWidth), wideOptics.outputPixelPitchUm(wideRawWidth))
            return Math.exp(ev * ln(2.0))
        }

        private const val AF_PASSIVE_FOCUSED = CaptureResult.CONTROL_AF_STATE_PASSIVE_FOCUSED
        private const val AF_TIMEOUT_MS = 10_000L
        private const val APPLY_TIMEOUT_MS = 4_000L
        private const val FILL_TIMEOUT_MS = 2_000L
        private const val RESULT_GRACE_MS = 200L
        private const val PAUSE_BETWEEN_PAIRS_MS = 300L
        private const val POLL_MS = 20L
        private const val NS_PER_MS = 1_000_000.0
    }
}
