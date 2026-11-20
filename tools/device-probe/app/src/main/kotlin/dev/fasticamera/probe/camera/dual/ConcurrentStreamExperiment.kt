package dev.fasticamera.probe.camera.dual

import android.os.Handler
import android.os.SystemClock
import dev.fasticamera.probe.camera.CameraSessionRunner
import org.json.JSONArray
import org.json.JSONObject

/**
 * Эксперименты 1, 2 и 4: открыть несколько камер сразу, создать по сессии на каждую,
 * держать потоки [seconds] секунд и записать по каждой ноге кадры, к/с, ошибки и ток батареи.
 *
 * Ноги с одним id камеры сходятся в одну сессию — так же описывается логическая камера 0
 * с физическими потоками (`0:yuv:640x480:2`, `0:yuv:640x480:3`).
 */
class ConcurrentStreamExperiment(
    private val runner: CameraSessionRunner,
    private val imageHandler: Handler,
    private val currentSampler: CurrentSampler?,
    private val progress: (stage: String, details: JSONObject) -> Unit,
) {
    data class Plan(
        val legs: List<LegSpec>,
        val aeTargetFps: Int?,
        val seconds: Int,
        val stepTimeoutMs: Long,
        /** Пауза после всех открытий: за это время вытесненная камера успевает прислать onDisconnected/onError. */
        val settleMs: Long,
    )

    fun run(plan: Plan): JSONObject {
        val report = JSONObject().put("legs", JSONArray(plan.legs.map(LegSpec::label)))
        val rigs = plan.legs.groupBy(LegSpec::cameraId).map { (cameraId, cameraLegs) ->
            CameraRig(cameraId, cameraLegs, RigProfile.plain(plan.aeTargetFps), runner, imageHandler, plan.stepTimeoutMs)
        }
        try {
            if (!openAll(rigs, report)) return report.put("stage_reached", "open")
            Thread.sleep(plan.settleMs)
            report.put("device_errors_after_open", JSONArray(runner.deviceErrors.toList()))
            if (!configureAll(rigs, report)) return report.put("stage_reached", "session")
            rigs.forEach(CameraRig::startStreaming)
            report.put("stage_reached", "streaming")
            progress("streaming", JSONObject())
            report.put("first_frames_arrived", JSONObject(rigs.associate { it.cameraId to it.awaitFirstFrame() } as Map<*, *>))
            measureWindow(plan.seconds)
            report.put("device_errors_final", JSONArray(runner.deviceErrors.toList()))
        } catch (error: Exception) {
            // Сбой посреди потока (например, камеру вытеснили) — часть результата, а не падение зонда.
            report.put("stream_exception", "${error.javaClass.name}: ${error.message}")
        } finally {
            rigs.forEach(CameraRig::close)
        }
        return report.put("cameras", JSONArray(rigs.map(CameraRig::toJson)))
            .put("current", currentSampler?.toJson() ?: JSONObject.NULL)
    }

    /** Открывает по очереди; false — какая-то камера не открылась (исходы всех попыток уже в отчёте). */
    private fun openAll(rigs: List<CameraRig>, report: JSONObject): Boolean {
        val outcomes = JSONArray()
        var allOpened = true
        for (rig in rigs) {
            val startedAtNs = SystemClock.elapsedRealtimeNanos()
            val outcome = rig.open()
            val entry = outcome.toJson().put("camera", rig.cameraId).put("ms", elapsedMs(startedAtNs))
            outcomes.put(entry)
            progress("open", entry)
            if (outcome !is CameraSessionRunner.OpenOutcome.Opened) {
                allOpened = false
                break
            }
        }
        report.put("open", outcomes)
        return allOpened
    }

    private fun configureAll(rigs: List<CameraRig>, report: JSONObject): Boolean {
        val outcomes = JSONArray()
        var allConfigured = true
        for (rig in rigs) {
            val startedAtNs = SystemClock.elapsedRealtimeNanos()
            val outcome = rig.configureSession()
            val entry = outcome.toJson().put("camera", rig.cameraId).put("ms", elapsedMs(startedAtNs))
            outcomes.put(entry)
            progress("session", entry)
            if (outcome !is CameraSessionRunner.SessionOutcome.Configured) {
                allConfigured = false
                break
            }
        }
        report.put("session", outcomes)
        return allConfigured
    }

    /** Ждёт окно замера, снимая ток батареи каждые [CURRENT_SAMPLE_PERIOD_MS]. */
    private fun measureWindow(seconds: Int) {
        val deadlineNs = SystemClock.elapsedRealtimeNanos() + seconds * NANOS_PER_SECOND
        while (SystemClock.elapsedRealtimeNanos() < deadlineNs) {
            currentSampler?.sample()
            Thread.sleep(CURRENT_SAMPLE_PERIOD_MS)
        }
    }

    private companion object {
        const val CURRENT_SAMPLE_PERIOD_MS = 500L
        const val NANOS_PER_SECOND = 1_000_000_000L
    }
}

internal fun elapsedMs(startedAtNs: Long): Double = (SystemClock.elapsedRealtimeNanos() - startedAtNs) / 1_000_000.0
