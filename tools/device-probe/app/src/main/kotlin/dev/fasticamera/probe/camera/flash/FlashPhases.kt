package dev.fasticamera.probe.camera.flash

import android.hardware.camera2.CameraMetadata
import android.hardware.camera2.CaptureRequest
import android.os.SystemClock
import dev.fasticamera.probe.camera.threea.ThreeASession
import dev.fasticamera.probe.camera.threea.ThreeATag
import dev.fasticamera.probe.report.ProbeLog
import org.json.JSONArray
import org.json.JSONObject

/** Настройки фаз разведки вспышки (всё приходит из аргументов сценария). */
class FlashSettings(
    val iso: Int,
    val baselineExposureNs: Long,
    val singleExposuresNs: List<Long>,
    val torchExposuresNs: List<Long>,
    val frameDurationNs: Long,
    val baselineFrames: Int,
    val settleFrames: Int,
    val shotsPerExposure: Int,
    val intentShots: Int,
    /** Минимальный интервал между выстрелами: перезарядка вспышки. */
    val shotGapMs: Long,
    val framesBeforeShot: Int,
    val framesAfterShot: Int,
    val torchOnFrames: Int,
    val torchOffFrames: Int,
    /** Выдержка для фазы single_intent. */
    val intentExposureNs: Long,
    /** Выдержка кадров предвспышки (torch) в фазах последовательности. */
    val preflashExposureNs: Long,
    /** Сколько освещённых кадров torch дождаться перед гашением (продукт держит torch ≈ 20 кадров). */
    val preflashLitFrames: Int,
    /** Паузы между гашением torch и выстрелом для фазы fire_after_off_gap, мс. */
    val offGapsMs: List<Long>,
)

/**
 * Фазы сценария `flash`: baseline, single, single_intent, torch и производные величины.
 * Владеет только порядком опытов и записью событий; камеру ведёт [ThreeASession], кадры хранит [FlashFrameStore].
 */
class FlashPhases(
    private val session: ThreeASession,
    private val store: FlashFrameStore,
    private val log: ProbeLog,
    private val settings: FlashSettings,
) {
    private val sequences = FlashSequencePhases(this, session, store, log, settings)
    private val shotSummaries = mutableListOf<ShotSummary>()
    private val torchSummaries = mutableListOf<TorchSummary>()

    fun runAll(only: Set<String>?) {
        fun enabled(name: String) = only == null || name in only
        if (enabled("baseline")) baseline()
        if (enabled("single")) single()
        if (enabled("single_intent")) singleIntent()
        if (enabled("control_off")) controlOff()
        if (enabled("torch_latency")) sequences.torchLatency()
        if (enabled("preflash_fire")) sequences.preflashFire(FlashSequencePhases.Order.SHOT_THEN_OFF)
        if (enabled("fire_after_off_gap")) {
            for (gapMs in settings.offGapsMs) sequences.preflashFire(FlashSequencePhases.Order.OFF_THEN_SHOT, gapMs)
        }
        if (enabled("fire_right_after_torch_off")) sequences.preflashFire(FlashSequencePhases.Order.OFF_THEN_SHOT)
        if (enabled("torch")) torch()
        reportDerived()
    }

    /** Гасит вспышку и torch повторяющимся запросом; вызывается на любом пути выхода. */
    fun forceFlashOff() {
        session.setRepeating(repeatingRequest("flash_off_exit", settings.baselineExposureNs, FLASH_OFF))
        Thread.sleep(EXIT_SETTLE_MS)
    }

    private fun baseline() {
        startRepeating("baseline", settings.baselineExposureNs, FLASH_OFF)
        store.awaitPhaseFrames("baseline", settings.baselineFrames, frameWaitMs(settings.baselineFrames))
        val ambient = FlashAnalysis.ambientOf(store.framesOfPhase("baseline"))
        log.result("baseline", JSONObject().put("ambient_mean", ambient?.mean ?: JSONObject.NULL)
            .put("ambient_bands", ambient?.bandMeans?.let { JSONArray(it.toList()) } ?: JSONObject.NULL)
            .put("frames", store.framesOfPhase("baseline").size)
            .put("cadence", FlashAnalysis.cadence(store.framesOfPhase("baseline"), 0L))
            .put("flash_state_runs", FlashAnalysis.runLengths(store.framesOfPhase("baseline").map { it.result.flashState })))
    }

    private fun single() {
        for (exposureNs in settings.singleExposuresNs) {
            val phase = "single_${exposureNs / NS_PER_MS}ms"
            settle(phase, exposureNs)
            var lastSubmitNs = 0L
            for (round in 1..settings.shotsPerExposure) {
                lastSubmitNs = waitRecharge(lastSubmitNs)
                lastSubmitNs = fireOneShot(phase, round, exposureNs, FLASH_SINGLE, captureIntent = null)
            }
        }
    }

    /** Контроль: такой же одиночный capture(), но FLASH_MODE=OFF — отделяет эффекты вспышки от эффектов самого capture() поверх repeating. */
    private fun controlOff() {
        val phase = "control_off_${settings.intentExposureNs / NS_PER_MS}ms"
        settle(phase, settings.intentExposureNs)
        var lastSubmitNs = 0L
        for (round in 1..settings.intentShots) {
            lastSubmitNs = waitRecharge(lastSubmitNs)
            lastSubmitNs = fireOneShot(phase, round, settings.intentExposureNs, FLASH_OFF, captureIntent = null)
        }
    }

    private fun singleIntent() {
        val phase = "single_intent_${settings.intentExposureNs / NS_PER_MS}ms"
        settle(phase, settings.intentExposureNs)
        var lastSubmitNs = 0L
        for (round in 1..settings.intentShots) {
            lastSubmitNs = waitRecharge(lastSubmitNs)
            lastSubmitNs = fireOneShot(phase, round, settings.intentExposureNs, FLASH_SINGLE, captureIntent = CameraMetadata.CONTROL_CAPTURE_INTENT_STILL_CAPTURE)
        }
    }

    private fun torch() {
        for (exposureNs in settings.torchExposuresNs) {
            val label = "${exposureNs / NS_PER_MS}ms"
            val settlePhase = "torch_settle_$label"
            settle(settlePhase, exposureNs)
            val ambient = FlashAnalysis.ambientOf(store.framesOfPhase(settlePhase).takeLast(AMBIENT_TAIL_FRAMES))
            if (ambient == null) {
                log.error("torch_$label", "нет кадров фона")
                continue
            }
            startRepeating("torch_on_$label", exposureNs, FLASH_TORCH)
            store.awaitPhaseFrames("torch_on_$label", settings.torchOnFrames, frameWaitMs(settings.torchOnFrames))
            startRepeating("torch_off_$label", exposureNs, FLASH_OFF)
            store.awaitPhaseFrames("torch_off_$label", settings.torchOffFrames, frameWaitMs(settings.torchOffFrames))
            val summary = FlashAnalysis.summarizeTorch(
                exposureNs, ambient,
                store.framesOfPhase("torch_on_$label").take(settings.torchOnFrames),
                store.framesOfPhase("torch_off_$label").take(settings.torchOffFrames),
            )
            torchSummaries += summary
            log.result("torch_$label", summary.json)
        }
    }

    /** Одиночный выстрел поверх повторяющегося запроса; возвращает момент отправки для расчёта перезарядки. */
    private fun fireOneShot(phase: String, round: Int, exposureNs: Long, flashMode: Int, captureIntent: Int?): Long {
        val tag = ThreeATag(phase, ThreeATag.Kind.TRIGGER, round = round, requestedExposureNs = exposureNs)
        val request = manualRequest(tag, exposureNs, flashMode) { builder ->
            captureIntent?.let { builder.set(CaptureRequest.CONTROL_CAPTURE_INTENT, it) }
        }
        val submitNs = SystemClock.elapsedRealtimeNanos()
        session.captureOnce(request)
        val result = store.awaitOneShotResult(phase, round, ONE_SHOT_TIMEOUT_MS)
        if (result == null) {
            log.error("one_shot_$phase#$round", "результат одиночного запроса не пришёл за $ONE_SHOT_TIMEOUT_MS мс")
            return submitNs
        }
        val from = result.frameNumber - settings.framesBeforeShot
        val to = result.frameNumber + settings.framesAfterShot
        val complete = store.awaitWindowComplete(from, to, WINDOW_TIMEOUT_MS)
        val window = store.window(from, to)
        val summary = FlashAnalysis.summarizeShot(phase, round, window, result.frameNumber, settings.framesBeforeShot, submitNs, result.arrivedNs)
        shotSummaries += summary
        log.result("shot_${phase}_$round", summary.json.put("window_complete", complete).put("intent_requested", captureIntent ?: JSONObject.NULL))
        log.event("shot_window_${phase}_$round", JSONObject().put("frames", JSONArray(window.map { it.toJson(result.frameNumber) })))
        return submitNs
    }

    /** Повторяющийся запрос на выдержку [exposureNs] без вспышки и ожидание, пока яркость и режим устоятся. */
    internal fun settle(phase: String, exposureNs: Long) {
        startRepeating(phase, exposureNs, FLASH_OFF)
        store.awaitPhaseFrames(phase, settings.settleFrames, frameWaitMs(settings.settleFrames))
    }

    internal fun startRepeating(phase: String, exposureNs: Long, flashMode: Int) {
        session.setRepeating(repeatingRequest(phase, exposureNs, flashMode))
    }

    internal fun repeatingRequest(phase: String, exposureNs: Long, flashMode: Int) =
        manualRequest(ThreeATag(phase, ThreeATag.Kind.REPEATING, requestedExposureNs = exposureNs), exposureNs, flashMode) { }

    /** Ручной AE (OFF) с заданной выдержкой/ISO/длительностью кадра и режимом вспышки; вендорские ключи 60 к/с добавляет сессия. */
    internal fun manualRequest(tag: ThreeATag, exposureNs: Long, flashMode: Int, extra: (CaptureRequest.Builder) -> Unit): CaptureRequest =
        session.newRequest(tag) { builder ->
            builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_OFF)
            builder.set(CaptureRequest.SENSOR_EXPOSURE_TIME, exposureNs)
            builder.set(CaptureRequest.SENSOR_SENSITIVITY, settings.iso)
            builder.set(CaptureRequest.SENSOR_FRAME_DURATION, settings.frameDurationNs)
            builder.set(CaptureRequest.FLASH_MODE, flashMode)
            extra(builder)
        }

    /** Ждём, чтобы с прошлой отправки прошло не меньше [FlashSettings.shotGapMs]; возвращает прежнее значение. */
    internal fun waitRecharge(lastSubmitNs: Long): Long {
        if (lastSubmitNs == 0L) return lastSubmitNs
        val elapsedMs = (SystemClock.elapsedRealtimeNanos() - lastSubmitNs) / NS_PER_MS
        if (elapsedMs < settings.shotGapMs) Thread.sleep(settings.shotGapMs - elapsedMs)
        return lastSubmitNs
    }

    /** Производные: R = (прирост single − фон) / (прирост torch − фон) при равной выдержке/ISO; разброс выстрелов. */
    private fun reportDerived() {
        val derived = JSONArray()
        for (exposureNs in settings.singleExposuresNs.distinct()) {
            val gains = shotSummaries.filter { it.exposureNs == exposureNs && it.label == "single_${exposureNs / NS_PER_MS}ms" }.mapNotNull { it.peakGain }
            if (gains.isEmpty()) continue
            val mean = gains.average()
            val spreadPercent = if (mean > 0) (gains.max() - gains.min()) / mean * PERCENT else null
            val torchGain = torchSummaries.firstOrNull { it.exposureNs == exposureNs }?.steadyGain
            derived.put(JSONObject().put("exposure_ns", exposureNs).put("single_peak_gains", JSONArray(gains.map { Math.round(it * 10) / 10.0 }))
                .put("single_mean_gain", mean).put("single_spread_percent", spreadPercent ?: JSONObject.NULL)
                .put("torch_steady_gain", torchGain ?: JSONObject.NULL)
                .put("ratio_single_to_torch", if (torchGain != null && torchGain > 0) mean / torchGain else JSONObject.NULL))
        }
        log.result("derived", JSONObject().put("per_exposure", derived))
    }

    private fun frameWaitMs(frames: Int): Long = frames * FRAME_WAIT_BUDGET_MS + FRAME_WAIT_SLACK_MS

    private companion object {
        const val NS_PER_MS = 1_000_000L
        const val PERCENT = 100.0
        const val FLASH_OFF = CameraMetadata.FLASH_MODE_OFF
        const val FLASH_SINGLE = CameraMetadata.FLASH_MODE_SINGLE
        const val FLASH_TORCH = CameraMetadata.FLASH_MODE_TORCH
        const val AMBIENT_TAIL_FRAMES = 20
        /** Паузы между гашением torch и одиночным выстрелом (мс) для фазы fire_after_off_gap. */
        val OFF_GAPS_MS = listOf(50L, 100L, 200L)
        const val ONE_SHOT_TIMEOUT_MS = 3_000L
        const val WINDOW_TIMEOUT_MS = 4_000L
        const val EXIT_SETTLE_MS = 400L
        /** С запасом на 30 к/с: кадры в разгоне потока могут идти реже. */
        const val FRAME_WAIT_BUDGET_MS = 50L
        const val FRAME_WAIT_SLACK_MS = 3_000L
    }
}
