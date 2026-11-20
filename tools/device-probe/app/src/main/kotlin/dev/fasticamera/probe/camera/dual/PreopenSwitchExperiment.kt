package dev.fasticamera.probe.camera.dual

import android.os.Handler
import android.os.SystemClock
import dev.fasticamera.probe.camera.CameraSessionRunner
import org.json.JSONArray
import org.json.JSONObject

/**
 * Эксперимент 3: сколько стоит переключение потока с камеры A на камеру B.
 *
 * Два способа, чередуются от повтора к повтору (чтобы дрейф нагрева не достался одному из них):
 * - [Variant.PREOPEN]: пока A стримит, открываем устройство B (без сессии); замер от
 *   «закрыть сессию A» до первого кадра B;
 * - [Variant.BASELINE]: как делает основное приложение — закрыть сессию и устройство A,
 *   открыть B, создать сессию, первый кадр. Замер от того же момента.
 */
class PreopenSwitchExperiment(
    private val runner: CameraSessionRunner,
    private val imageHandler: Handler,
    private val progress: (stage: String, details: JSONObject) -> Unit,
) {
    enum class Variant(val wireName: String) { PREOPEN("preopen"), BASELINE("baseline") }

    data class Plan(
        val streaming: LegSpec,
        val target: LegSpec,
        val aeTargetFps: Int?,
        val repetitions: Int,
        val stepTimeoutMs: Long,
        /** Прогрев A до замера: к моменту переключения поток уже установился. */
        val warmupMs: Long,
    )

    fun run(plan: Plan): JSONObject {
        val attempts = JSONArray()
        val millisByVariant = Variant.entries.associateWith { ArrayList<Double>() }
        repeat(plan.repetitions) { index ->
            // Чётные повторы начинают с preopen, нечётные — с baseline.
            val order = if (index % 2 == 0) Variant.entries else Variant.entries.reversed()
            for (variant in order) {
                val attempt = runAttempt(plan, variant).put("repetition", index)
                attempts.put(attempt)
                progress("attempt", attempt)
                attempt.optDouble("switch_ms").takeIf { !it.isNaN() }?.let { millisByVariant.getValue(variant) += it }
                runner.deviceErrors.clear()
                Thread.sleep(PAUSE_BETWEEN_ATTEMPTS_MS)
            }
        }
        val summary = JSONObject()
        Variant.entries.forEach { summary.put(it.wireName, SampleSummary.toJsonOrNull(millisByVariant.getValue(it))) }
        return JSONObject()
            .put("streaming", plan.streaming.label).put("target", plan.target.label)
            .put("attempts", attempts).put("switch_ms_summary", summary)
    }

    private fun runAttempt(plan: Plan, variant: Variant): JSONObject {
        val attempt = JSONObject().put("variant", variant.wireName)
        val source = CameraRig(plan.streaming.cameraId, listOf(plan.streaming), RigProfile.plain(plan.aeTargetFps), runner, imageHandler, plan.stepTimeoutMs)
        val target = CameraRig(plan.target.cameraId, listOf(plan.target), RigProfile.plain(plan.aeTargetFps), runner, imageHandler, plan.stepTimeoutMs)
        try {
            if (!startSource(source, plan, attempt)) return attempt
            if (variant == Variant.PREOPEN && !preopenTarget(source, target, attempt)) return attempt
            val startedAtNs = SystemClock.elapsedRealtimeNanos()
            source.closeSession()
            if (variant == Variant.BASELINE) {
                source.closeDevice()
                val openOutcome = target.open()
                attempt.put("target_open", openOutcome.toJson()).put("open_done_ms", elapsedMs(startedAtNs))
                if (openOutcome !is CameraSessionRunner.OpenOutcome.Opened) return attempt.put("failed_at", "target_open")
            }
            val sessionOutcome = target.configureSession()
            attempt.put("target_session", sessionOutcome.toJson()).put("session_done_ms", elapsedMs(startedAtNs))
            if (sessionOutcome !is CameraSessionRunner.SessionOutcome.Configured) return attempt.put("failed_at", "target_session")
            target.startStreaming()
            if (!target.awaitFirstFrame()) return attempt.put("failed_at", "target_first_frame")
            val firstFrameAtNs = checkNotNull(target.firstFrameAtNs)
            attempt.put("switch_ms", (firstFrameAtNs - startedAtNs) / 1_000_000.0)
        } catch (error: Exception) {
            attempt.put("exception", "${error.javaClass.name}: ${error.message}")
        } finally {
            attempt.put("device_errors", JSONArray(runner.deviceErrors.toList()))
            target.close()
            source.close()
        }
        return attempt
    }

    private fun startSource(source: CameraRig, plan: Plan, attempt: JSONObject): Boolean {
        val openOutcome = source.open()
        if (openOutcome !is CameraSessionRunner.OpenOutcome.Opened) {
            attempt.put("source_open", openOutcome.toJson()).put("failed_at", "source_open")
            return false
        }
        val sessionOutcome = source.configureSession()
        if (sessionOutcome !is CameraSessionRunner.SessionOutcome.Configured) {
            attempt.put("source_session", sessionOutcome.toJson()).put("failed_at", "source_session")
            return false
        }
        source.startStreaming()
        if (!source.awaitFirstFrame()) {
            attempt.put("failed_at", "source_first_frame")
            return false
        }
        Thread.sleep(plan.warmupMs)
        return true
    }

    /** Открывает B пока A стримит; пишет, сколько это стоило и не остановило ли поток A. */
    private fun preopenTarget(source: CameraRig, target: CameraRig, attempt: JSONObject): Boolean {
        val framesBefore = source.framesReceived
        val startedAtNs = SystemClock.elapsedRealtimeNanos()
        val openOutcome = target.open()
        attempt.put("preopen", openOutcome.toJson()).put("preopen_ms", elapsedMs(startedAtNs))
        if (openOutcome !is CameraSessionRunner.OpenOutcome.Opened) return false.also { attempt.put("failed_at", "preopen") }
        Thread.sleep(SOURCE_ALIVE_CHECK_MS)
        val framesGained = source.framesReceived - framesBefore
        attempt.put("source_frames_during_preopen", framesGained)
        return framesGained > 0 || false.also { attempt.put("failed_at", "source_stalled_after_preopen") }
    }

    private companion object {
        const val PAUSE_BETWEEN_ATTEMPTS_MS = 1_000L
        const val SOURCE_ALIVE_CHECK_MS = 500L
    }
}
