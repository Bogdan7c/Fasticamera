package dev.fasticamera.probe.camera.dual

import android.content.Context
import android.hardware.camera2.CameraManager
import android.os.Handler
import android.os.SystemClock
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.ManualExposure
import dev.fasticamera.probe.camera.VendorKeyValue
import org.json.JSONArray
import org.json.JSONObject

/**
 * Продуктовая конфигурация «тёплого standby»: камера 2 (основная) стримит RAW10 4:3 @60 через
 * вендорские ключи CamX, камера 3 (ширик) — RAW10 с AE [60,60] без ключей CamX; на обеих —
 * ручная экспозиция, непрерывный AF и YUV-компаньон в сессии (не цель запроса).
 */
class ProductDualExperiment(
    private val runner: CameraSessionRunner,
    private val imageHandler: Handler,
    private val appContext: Context,
    private val stepTimeoutMs: Long,
    private val progress: (stage: String, details: JSONObject) -> Unit,
) {
    private val mainLeg = LegSpec.parse("2:raw10:4096x3072")
    private val wideLeg = LegSpec.parse("3:raw10:4000x3000")
    private val companion = { id: String -> LegSpec.parse("$id:yuv:640x480") }

    private val manualExposure = ManualExposure(EXPOSURE_NS, ISO, FRAME_DURATION_NS)

    private fun mainProfile() = RigProfile(
        aeTargetFps = null, manualExposure = manualExposure, afContinuous = true,
        vendorKeys = VendorKeyValue.parseList(
            "org.quic.camera.manualExposure.disableFPSLimits:byte=1;" +
                "org.quic.camera.manualExposure.overrideSensorFrameDuration:long=$FRAME_DURATION_NS",
        ),
        companionYuv = companion("2"), discardFrames = false,
    )

    private fun wideProfile(discardFrames: Boolean) = RigProfile(
        aeTargetFps = 60, manualExposure = manualExposure, afContinuous = true,
        vendorKeys = emptyList(), companionYuv = companion("3"), discardFrames = discardFrames,
    )

    private fun mainRig(frameSink: RawFrameSink? = null) =
        CameraRig("2", listOf(mainLeg), mainProfile(), runner, imageHandler, stepTimeoutMs, frameSink)

    private fun wideRig(discardFrames: Boolean, frameSink: RawFrameSink? = null) =
        CameraRig("3", listOf(wideLeg), wideProfile(discardFrames), runner, imageHandler, stepTimeoutMs, frameSink)

    /** Поднимает обе камеры в заданном порядке; вторая стартует, пока первая уже стримит. null — успех. */
    private fun bringUpBoth(order: BringUpOrder, main: CameraRig, wide: CameraRig): String? = when (order) {
        BringUpOrder.MAIN_FIRST -> bringUp(main) ?: bringUp(wide)
        BringUpOrder.WIDE_FIRST -> bringUp(wide) ?: bringUp(main)
    }

    /** Открывает, настраивает и запускает поток; null — успех, иначе описание шага, на котором остановились. */
    private fun bringUp(rig: CameraRig): String? {
        if (rig.open() !is CameraSessionRunner.OpenOutcome.Opened) return "open cam${rig.cameraId}"
        if (rig.configureSession() !is CameraSessionRunner.SessionOutcome.Configured) return "session cam${rig.cameraId}"
        rig.startStreaming()
        return null
    }

    /**
     * Пункт 1: обе камеры [seconds] секунд; [discardWide] — кадры ширика возвращаются «в никуда»;
     * [order] — какая камера поднимается первой (вторая стартует при уже идущем потоке первой).
     */
    fun streamBoth(seconds: Int, discardWide: Boolean, order: BringUpOrder = BringUpOrder.MAIN_FIRST): JSONObject {
        val report = JSONObject().put("discard_wide_frames", discardWide).put("order", order.wireName)
        val main = mainRig()
        val wide = wideRig(discardWide)
        try {
            val failedStep = bringUpBoth(order, main, wide)
            report.put("failed_step", failedStep ?: JSONObject.NULL)
            if (failedStep == null) {
                report.put("first_frames", JSONObject().put("2", main.awaitFirstFrame()).put("3", wide.awaitFirstFrame()))
                Thread.sleep(seconds * MS_PER_SECOND)
            }
            report.put("device_errors", JSONArray(runner.deviceErrors.toList()))
        } catch (error: Exception) {
            report.put("exception", "${error.javaClass.name}: ${error.message}")
        } finally {
            report.put("cameras", JSONArray(listOf(main.toJson(), wide.toJson())))
            wide.close()
            main.close()
        }
        return report
    }

    /**
     * Замер чувствительности: обе камеры в продуктовой конфигурации стримят одновременно, на каждой ступени
     * [SensitivityPlan.steps] выставляется одна и та же ручная экспозиция и сравниваются средние RAW
     * в совпадающем поле зрения. Статические факты и регионы — в отчёте.
     */
    fun measureSensitivity(plan: SensitivityPlan): JSONObject {
        val report = JSONObject().put("order", plan.order.wireName)
        val cameraManager = checkNotNull(appContext.getSystemService(CameraManager::class.java)) { "нет CameraManager" }
        val mainOptics = SensorOptics.fromCharacteristics("2", cameraManager.getCameraCharacteristics("2"))
        val wideOptics = SensorOptics.fromCharacteristics("3", cameraManager.getCameraCharacteristics("3"))
        val mainSampler = createSampler(mainOptics, mainLeg, plan.fovFraction)
        val wideSampler = createSampler(wideOptics, wideLeg, plan.fovFraction * SensitivityMeasurement.zoomRatioWide(mainOptics, wideOptics))
        val main = mainRig(mainSampler.sampler)
        val wide = wideRig(discardFrames = false, frameSink = wideSampler.sampler)
        try {
            val failedStep = bringUpBoth(plan.order, main, wide)
            report.put("failed_step", failedStep ?: JSONObject.NULL)
            if (failedStep == null) {
                report.put("first_frames", JSONObject().put("2", main.awaitFirstFrame()).put("3", wide.awaitFirstFrame()))
                Thread.sleep(WARMUP_MS)
                val measurement = SensitivityMeasurement(
                    SensitivitySubject(main, mainOptics, mainSampler.region, mainSampler.center, mainSampler.sampler, mainLeg.width, mainLeg.height),
                    SensitivitySubject(wide, wideOptics, wideSampler.region, wideSampler.center, wideSampler.sampler, wideLeg.width, wideLeg.height),
                    FRAME_DURATION_NS, plan,
                )
                val result = measurement.run()
                result.keys().forEach { key -> report.put(key, result.get(key)) }
            }
            report.put("device_errors", JSONArray(runner.deviceErrors.toList()))
        } catch (error: Exception) {
            report.put("exception", "${error.javaClass.name}: ${error.message}")
        } finally {
            report.put("cameras", JSONArray(listOf(main.toJson(), wide.toJson())))
            wide.close()
            main.close()
        }
        return report
    }

    /**
     * Замер совмещения кадров: обе камеры стримят в продуктовой конфигурации, экспозиция ширика поднята на
     * [AlignmentMeasurement.wideExposureFactor], чтобы яркость сошлась; зелёные плоскости пар кадров пишутся на диск
     * (основная — весь массив, ширик — область k*(1+запас) вокруг оптической оси), статика линз — в отчёт.
     */
    fun measureAlignment(plan: AlignmentPlan): JSONObject {
        val report = JSONObject().put("order", plan.order.wireName)
        val cameraManager = checkNotNull(appContext.getSystemService(CameraManager::class.java)) { "нет CameraManager" }
        val mainCharacteristics = cameraManager.getCameraCharacteristics("2")
        val wideCharacteristics = cameraManager.getCameraCharacteristics("3")
        val mainOptics = SensorOptics.fromCharacteristics("2", mainCharacteristics)
        val wideOptics = SensorOptics.fromCharacteristics("3", wideCharacteristics)
        val zoomRatio = SensitivityMeasurement.zoomRatioWide(mainOptics, wideOptics)
        val factor = AlignmentMeasurement.wideExposureFactor(mainOptics, wideOptics, mainLeg.width, wideLeg.width)
        val mainExposure = ExposureStep(plan.mainExposureNs, plan.iso)
        val wideExposure = ExposureStep((plan.mainExposureNs * factor).toLong(), plan.iso)
        val mainSetup = createSampler(mainOptics, mainLeg, 1.0)
        val wideSetup = createSampler(wideOptics, wideLeg, (zoomRatio * (1.0 + plan.marginFraction)).coerceAtMost(1.0))
        val wideRegion = wideSetup.region.fitToMultipleOf(plan.downsample)
        val mainRing = AlignmentFrameRing(AlignmentMeasurement.RING_CAPACITY, ringBytes(mainLeg))
        val wideRing = AlignmentFrameRing(AlignmentMeasurement.RING_CAPACITY, ringBytes(wideLeg))
        val main = mainRig(mainRing)
        val wide = wideRig(discardFrames = false, frameSink = wideRing)
        report.put("wide_exposure_factor", factor)
        try {
            val failedStep = bringUpBoth(plan.order, main, wide)
            report.put("failed_step", failedStep ?: JSONObject.NULL)
            if (failedStep == null) {
                report.put("first_frames", JSONObject().put("2", main.awaitFirstFrame()).put("3", wide.awaitFirstFrame()))
                Thread.sleep(WARMUP_MS)
                val measurement = AlignmentMeasurement(
                    AlignmentSubject("main", main, mainOptics, mainSetup.region.fitToMultipleOf(plan.downsample), mainSetup.center, mainRing, mainLeg.width, mainLeg.height, mainExposure),
                    AlignmentSubject("wide", wide, wideOptics, wideRegion, wideSetup.center, wideRing, wideLeg.width, wideLeg.height, wideExposure),
                    FRAME_DURATION_NS, plan,
                )
                val result = measurement.run()
                result.getJSONObject("static")
                    .put("lens_main", LensGeometryFacts.fromCharacteristics(mainCharacteristics))
                    .put("lens_wide", LensGeometryFacts.fromCharacteristics(wideCharacteristics))
                result.keys().forEach { key -> report.put(key, result.get(key)) }
            }
            report.put("device_errors", JSONArray(runner.deviceErrors.toList()))
        } catch (error: Exception) {
            report.put("exception", "${error.javaClass.name}: ${error.stackTraceToString().take(1500)}")
        } finally {
            report.put("cameras", JSONArray(listOf(main.toJson(), wide.toJson())))
            wide.close()
            main.close()
        }
        return report
    }

    /** Размер слота кольца: буфер кадра RAW10 (5 байт на 4 пикселя) с запасом 50% на выравнивание строки. */
    private fun ringBytes(leg: LegSpec): Int = leg.width * leg.height * 3 / 2

    /** Область, у которой ширина и высота кратны [step] (левый верхний угол прежний, чётный). */
    private fun PixelRegion.fitToMultipleOf(step: Int) = copy(width = width / step * step, height = height / step * step)

    /** Область и сборщик для одной камеры: центр — оптическая ось из калибровки, размер — доля буфера. */
    private class SamplerSetup(val sampler: RawRegionSampler, val region: PixelRegion, val center: OpticalCenter)

    private fun createSampler(optics: SensorOptics, leg: LegSpec, sizeFraction: Double): SamplerSetup {
        val array = optics.preCorrectionActiveArray
        val center = SensitivityGeometry.opticalCenter(optics.intrinsicCalibration, array.width(), array.height())
        val region = SensitivityGeometry.centeredRegion(leg.width, leg.height, sizeFraction, center)
        return SamplerSetup(RawRegionSampler(region, optics.whiteLevel), region, center)
    }

    /** Пункт 2: пока камера 2 стримит, время «открыть 3 → сессия → первый RAW-кадр». */
    fun standbyStart(repetitions: Int): JSONObject {
        val attempts = JSONArray()
        val totals = ArrayList<Double>()
        repeat(repetitions) { index ->
            val attempt = JSONObject().put("repetition", index)
            val main = mainRig()
            val wide = wideRig(discardFrames = false)
            try {
                val failedStep = bringUp(main)
                if (failedStep != null || !main.awaitFirstFrame()) {
                    attempt.put("failed_at", failedStep ?: "main_first_frame")
                } else {
                    Thread.sleep(WARMUP_MS)
                    measureStandby(wide, attempt)?.let(totals::add)
                }
            } catch (error: Exception) {
                attempt.put("exception", "${error.javaClass.name}: ${error.message}")
            } finally {
                wide.close()
                main.close()
            }
            attempts.put(attempt)
            progress("standby_attempt", attempt)
            runner.deviceErrors.clear()
            Thread.sleep(PAUSE_MS)
        }
        return JSONObject().put("attempts", attempts).put("total_ms_summary", SampleSummary.toJsonOrNull(totals))
    }

    private fun measureStandby(wide: CameraRig, attempt: JSONObject): Double? {
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        if (wide.open() !is CameraSessionRunner.OpenOutcome.Opened) return null.also { attempt.put("failed_at", "open3") }
        attempt.put("open_ms", elapsedMs(startedAtNs))
        if (wide.configureSession() !is CameraSessionRunner.SessionOutcome.Configured) {
            return null.also { attempt.put("failed_at", "session3") }
        }
        attempt.put("session_done_ms", elapsedMs(startedAtNs))
        wide.startStreaming()
        if (!wide.awaitFirstFrame()) return null.also { attempt.put("failed_at", "first_frame3") }
        val totalMs = (checkNotNull(wide.firstFrameAtNs) - startedAtNs) / NS_PER_MS
        attempt.put("first_frame_ms", totalMs)
        return totalMs
    }

    /**
     * Пункт 3: ABBA по току. [order] — например "ABBAABBA": A = камера 2 одна, B = камеры 2+3.
     * Каждое окно — полный перезапуск потоков, прогрев [WINDOW_WARMUP_MS] не измеряется.
     */
    fun currentAbba(order: String, windowSeconds: Int, source: CurrentSampler.Source): JSONObject {
        val windows = JSONArray()
        val medians = mapOf('A' to ArrayList<Double>(), 'B' to ArrayList<Double>())
        for ((index, kind) in order.withIndex()) {
            require(kind == 'A' || kind == 'B') { "порядок ABBA: только A и B, получено '$kind'" }
            val sampler = CurrentSampler(appContext, source)
            val window = JSONObject().put("index", index).put("kind", kind.toString())
            if (!sampler.isOnBattery) return JSONObject().put("skipped", "телефон на зарядке")
            val main = mainRig()
            val wide = wideRig(discardFrames = false)
            try {
                val failedStep = bringUp(main) ?: if (kind == 'B') bringUp(wide) else null
                if (failedStep != null) {
                    window.put("failed_step", failedStep)
                } else {
                    Thread.sleep(WINDOW_WARMUP_MS)
                    repeat(windowSeconds) {
                        sampler.sample()
                        Thread.sleep(MS_PER_SECOND)
                    }
                    window.put("frames_main", main.framesReceived).put("frames_wide", wide.framesReceived)
                }
            } catch (error: Exception) {
                window.put("exception", "${error.javaClass.name}: ${error.message}")
            } finally {
                wide.close()
                main.close()
            }
            window.put("current", sampler.toJson())
            sampler.medianOrNull()?.let { medians.getValue(kind) += it }
            windows.put(window)
            progress("abba_window", window)
            Thread.sleep(PAUSE_MS)
        }
        val summary = JSONObject()
        medians.forEach { (kind, values) -> summary.put(kind.toString(), SampleSummary.toJsonOrNull(values)) }
        val aMean = medians.getValue('A').takeIf { it.isNotEmpty() }?.average()
        val bMean = medians.getValue('B').takeIf { it.isNotEmpty() }?.average()
        return JSONObject().put("windows", windows).put("window_median_summary", summary)
            .put("b_minus_a_of_means", if (aMean != null && bMean != null) bMean - aMean else JSONObject.NULL)
    }

    private companion object {
        const val EXPOSURE_NS = 8_000_000L
        const val ISO = 400
        const val FRAME_DURATION_NS = 16_666_667L
        const val MS_PER_SECOND = 1_000L
        const val NS_PER_MS = 1_000_000.0
        const val WARMUP_MS = 800L
        const val WINDOW_WARMUP_MS = 3_000L
        const val PAUSE_MS = 3_000L
    }
}
