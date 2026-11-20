package dev.fasticamera.probe.camera.dual

import android.content.Context
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

    private fun mainRig() = CameraRig("2", listOf(mainLeg), mainProfile(), runner, imageHandler, stepTimeoutMs)
    private fun wideRig(discardFrames: Boolean) =
        CameraRig("3", listOf(wideLeg), wideProfile(discardFrames), runner, imageHandler, stepTimeoutMs)

    /** Открывает, настраивает и запускает поток; null — успех, иначе описание шага, на котором остановились. */
    private fun bringUp(rig: CameraRig): String? {
        if (rig.open() !is CameraSessionRunner.OpenOutcome.Opened) return "open cam${rig.cameraId}"
        if (rig.configureSession() !is CameraSessionRunner.SessionOutcome.Configured) return "session cam${rig.cameraId}"
        rig.startStreaming()
        return null
    }

    /** Пункт 1: обе камеры [seconds] секунд; [discardWide] — кадры ширика возвращаются «в никуда». */
    fun streamBoth(seconds: Int, discardWide: Boolean): JSONObject {
        val report = JSONObject().put("discard_wide_frames", discardWide)
        val main = mainRig()
        val wide = wideRig(discardWide)
        try {
            val failedStep = bringUp(main) ?: bringUp(wide)
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
