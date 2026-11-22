package dev.fasticamera.probe.camera.flash

import android.hardware.camera2.CameraMetadata
import android.os.SystemClock
import dev.fasticamera.probe.camera.threea.ThreeASession
import dev.fasticamera.probe.camera.threea.ThreeATag
import dev.fasticamera.probe.report.ProbeLog
import org.json.JSONArray
import org.json.JSONObject

/**
 * Фазы второго раунда: задержки torch и продуктовая последовательность «предвспышка torch -> одиночный выстрел».
 * Все моменты — elapsedRealtimeNanos той же шкалы, что SENSOR_TIMESTAMP, поэтому «от вызова до начала экспозиции» считается вычитанием.
 */
class FlashSequencePhases(
    private val base: FlashPhases,
    private val session: ThreeASession,
    private val store: FlashFrameStore,
    private val log: ProbeLog,
    private val settings: FlashSettings,
) {
    /** Порядок команд после предвспышки: выстрел и сразу гашение, либо гашение и сразу выстрел. */
    enum class Order { SHOT_THEN_OFF, OFF_THEN_SHOT }

    /** Отметка вызова setRepeating/capture: время и последний уже известный номер кадра. */
    private class CallMark(val callNs: Long, val lastFrameNumber: Long)

    /** A. Задержка включения и выключения torch повторяющимся запросом, [REPETITIONS] раз. */
    fun torchLatency() {
        val exposureNs = settings.baselineExposureNs
        val repetitions = JSONArray()
        for (round in 1..REPETITIONS) {
            val settlePhase = "tl_settle_$round"
            base.settle(settlePhase, exposureNs)
            val ambient = ambientOfTail(settlePhase) ?: run { log.error("torch_latency", "нет фона"); return }
            val onMark = markedRepeating(base.repeatingRequest("tl_on_$round", exposureNs, FLASH_TORCH))
            val onFrames = frameWindow(onMark, LATENCY_WINDOW_FRAMES)
            // Ждём установившегося torch: хвост окна считаем «после включения».
            val offMark = markedRepeating(base.repeatingRequest("tl_off_$round", exposureNs, FLASH_OFF))
            val offFrames = frameWindow(offMark, LATENCY_WINDOW_FRAMES)
            val item = JSONObject().put("round", round).put("ambient_mean", ambient.mean)
                .put("on", latencyReport(onMark, onFrames, ambient, targetMode = FLASH_TORCH, lit = true))
                .put("off", latencyReport(offMark, offFrames, ambient, targetMode = FLASH_OFF, lit = false))
            repetitions.put(item)
        }
        log.result("torch_latency", JSONObject().put("repetitions", repetitions))
    }

    /** B/C. Предвспышка torch с другой выдержкой, затем одиночный выстрел и гашение в заданном [order]. */
    fun preflashFire(order: Order, gapAfterOffMs: Long = 0L) {
        val label = when {
            order == Order.SHOT_THEN_OFF -> "preflash_fire"
            gapAfterOffMs == 0L -> "fire_right_after_torch_off"
            else -> "fire_after_off_gap_${gapAfterOffMs}ms"
        }
        val shotExposureNs = settings.baselineExposureNs
        val preflashExposureNs = PREFLASH_EXPOSURE_NS
        // Фон на выдержке предвспышки измеряем один раз: нужен, чтобы отличить освещённые кадры torch.
        base.settle("${label}_ambient_pre", preflashExposureNs)
        val ambientPre = ambientOfTail("${label}_ambient_pre") ?: run { log.error(label, "нет фона предвспышки"); return }
        val results = JSONArray()
        var lastSubmitNs = 0L
        for (round in 1..REPETITIONS) {
            lastSubmitNs = base.waitRecharge(lastSubmitNs)
            base.settle("${label}_settle_$round", shotExposureNs)
            val ambientShot = ambientOfTail("${label}_settle_$round") ?: continue
            val torchPhase = "${label}_torch_$round"
            val torchMark = markedRepeating(base.repeatingRequest(torchPhase, preflashExposureNs, FLASH_TORCH))
            val litTorchFrames = awaitLitFrames(torchPhase, ambientPre, count = LIT_TORCH_FRAMES_REQUIRED)
            val shotTag = ThreeATag("${label}_shot", ThreeATag.Kind.TRIGGER, round = round, requestedExposureNs = shotExposureNs)
            val shotRequest = base.manualRequest(shotTag, shotExposureNs, FLASH_SINGLE) { }
            val offRequest = base.repeatingRequest("${label}_off_$round", shotExposureNs, FLASH_OFF)
            val shotCallNs: Long
            val offCallNs: Long
            if (order == Order.SHOT_THEN_OFF) {
                shotCallNs = SystemClock.elapsedRealtimeNanos(); session.captureOnce(shotRequest)
                offCallNs = SystemClock.elapsedRealtimeNanos(); session.setRepeating(offRequest)
            } else {
                offCallNs = SystemClock.elapsedRealtimeNanos(); session.setRepeating(offRequest)
                if (gapAfterOffMs > 0) Thread.sleep(gapAfterOffMs) // пауза между гашением torch и выстрелом
                shotCallNs = SystemClock.elapsedRealtimeNanos(); session.captureOnce(shotRequest)
            }
            lastSubmitNs = shotCallNs
            val shot = store.awaitOneShotResult("${label}_shot", round, ONE_SHOT_TIMEOUT_MS)
            if (shot == null) { log.error("${label}_$round", "результат выстрела не пришёл"); continue }
            val from = torchMark.lastFrameNumber + 1
            val to = shot.frameNumber + POST_SHOT_FRAMES
            store.awaitWindowComplete(from, to, WINDOW_TIMEOUT_MS)
            val window = store.window(from, to)
            results.put(sequenceReport(round, window, shot, torchMark, shotCallNs, offCallNs, ambientPre, ambientShot, litTorchFrames.size))
        }
        log.result(label, JSONObject().put("repetitions", results).put("ambient_at_preflash_exposure", ambientPre.mean))
    }

    // --- отчёты --------------------------------------------------------------------------------

    /** Что было между вызовом и целевым кадром: первый кадр с нужным эхо режима, первый освещённый (или первый тёмный), состояния между. */
    private fun latencyReport(mark: CallMark, frames: List<FlashFrame>, ambient: Ambient, targetMode: Int, lit: Boolean): JSONObject {
        fun isLitFrame(frame: FlashFrame) = frame.profile?.let { FlashAnalysis.isLit(it, ambient) } == true
        val modeFrame = frames.firstOrNull { it.result.flashMode == targetMode }
        val stateFrame = frames.firstOrNull { isLitFrame(it) == lit && it.profile != null }
        val json = JSONObject()
        fun put(prefix: String, frame: FlashFrame?) {
            json.put("${prefix}_frames", frame?.let { it.result.frameNumber - mark.lastFrameNumber } ?: JSONObject.NULL)
                .put("${prefix}_exposure_start_ms", frame?.let { (it.result.sensorTimestampNs - mark.callNs) / NS_PER_MS } ?: JSONObject.NULL)
                .put("${prefix}_result_ms", frame?.let { (it.result.arrivedNs - mark.callNs) / NS_PER_MS } ?: JSONObject.NULL)
        }
        put("first_mode_echo", modeFrame)
        put(if (lit) "first_lit" else "first_unlit", stateFrame)
        // Кадры от вызова до целевого: состояния, режимы и яркость — видно, что происходит «посередине».
        val until = stateFrame?.result?.frameNumber ?: Long.MAX_VALUE
        json.put("between", JSONArray(frames.filter { it.result.frameNumber <= until }.map { frameJson(it, mark, ambient) }))
        json.put("cadence", FlashAnalysis.cadence(frames, mark.lastFrameNumber))
        return json
    }

    private fun sequenceReport(
        round: Int, window: List<FlashFrame>, shot: FlashResultInfo, torchMark: CallMark,
        shotCallNs: Long, offCallNs: Long, ambientPre: Ambient, ambientShot: Ambient, litTorchFramesAtShot: Int,
    ): JSONObject {
        val shotFrame = window.first { it.result.frameNumber == shot.frameNumber }
        val torchLit = window.filter { it.result.flashMode == FLASH_TORCH && it.profile?.let { p -> FlashAnalysis.isLit(p, ambientPre) } == true }
        val torchGain4 = torchLit.takeIf { it.isNotEmpty() }?.let { FlashAnalysis.median(it.map { f -> f.profile!!.overallMean }) - ambientPre.mean }
        val shotGain = shotFrame.profile?.let { it.overallMean - ambientShot.mean }
        val exposureRatio = shotFrame.result.exposureNs?.let { it.toDouble() / PREFLASH_EXPOSURE_NS } ?: (settings.baselineExposureNs.toDouble() / PREFLASH_EXPOSURE_NS)
        val after = window.filter { it.result.frameNumber > shot.frameNumber }
        val firstUnlitAfter = after.firstOrNull { f -> f.profile?.let { !FlashAnalysis.isLit(it, ambientShot) } == true }
        val json = JSONObject().put("round", round).put("lit_torch_frames_when_shot_sent", litTorchFramesAtShot)
            .put("shot_frame", shot.frameNumber)
            .put("single_echo_frames", JSONArray(window.filter { it.result.flashMode == FLASH_SINGLE }.map { it.result.frameNumber - shot.frameNumber }))
            .put("fired_frames", JSONArray(window.filter { it.result.flashState == FIRED }.map { it.result.frameNumber - shot.frameNumber }))
            .put("torch_echo_frames", JSONArray(window.filter { it.result.flashMode == FLASH_TORCH }.map { it.result.frameNumber - shot.frameNumber }))
            .put("torch_gain_at_preflash_exposure", torchGain4 ?: JSONObject.NULL)
            .put("shot_gain_over_ambient", shotGain ?: JSONObject.NULL)
            .put("predicted_gain_1_87x_scaled_torch", torchGain4?.let { 1.87 * it * exposureRatio } ?: JSONObject.NULL)
            .put("shot_gain_over_scaled_torch", if (torchGain4 != null && shotGain != null) shotGain / (torchGain4 * exposureRatio) else JSONObject.NULL)
            .put("first_unlit_frame_after_shot_offset", firstUnlitAfter?.let { it.result.frameNumber - shot.frameNumber } ?: JSONObject.NULL)
            .put("torch_call_to_shot_exposure_start_ms", (shot.sensorTimestampNs - torchMark.callNs) / NS_PER_MS)
            .put("torch_call_to_shot_result_ms", (shot.arrivedNs - torchMark.callNs) / NS_PER_MS)
            .put("torch_call_to_shot_call_ms", (shotCallNs - torchMark.callNs) / NS_PER_MS)
            .put("off_call_minus_shot_call_ms", (offCallNs - shotCallNs) / NS_PER_MS)
            .put("shot_call_to_exposure_start_ms", (shot.sensorTimestampNs - shotCallNs) / NS_PER_MS)
            .put("cadence", FlashAnalysis.cadence(window, shot.frameNumber))
            .put("sensor_mode_runs", FlashAnalysis.runLengths(window.map { it.result.sensorMode }))
        val frames = JSONArray()
        for (frame in window) frames.put(frameJson(frame, torchMark, ambientShot).put("offset_to_shot", frame.result.frameNumber - shot.frameNumber))
        log.event("sequence_window_$round", JSONObject().put("frames", frames))
        return json
    }

    /** Кадр для отчёта: базовый JSON + прирост над фоном, кадры и миллисекунды после вызова. */
    private fun frameJson(frame: FlashFrame, mark: CallMark, ambient: Ambient): JSONObject = frame.toJson()
        .put("gain", frame.profile?.let { Math.round((it.overallMean - ambient.mean) * 10) / 10.0 } ?: JSONObject.NULL)
        .put("frames_after_call", frame.result.frameNumber - mark.lastFrameNumber)
        .put("exposure_start_ms_after_call", (frame.result.sensorTimestampNs - mark.callNs) / NS_PER_MS)

    // --- помощники -----------------------------------------------------------------------------

    private fun markedRepeating(request: android.hardware.camera2.CaptureRequest): CallMark {
        val lastFrame = store.allFrames().lastOrNull()?.result?.frameNumber ?: -1L
        val callNs = SystemClock.elapsedRealtimeNanos()
        session.setRepeating(request)
        return CallMark(callNs, lastFrame)
    }

    private fun frameWindow(mark: CallMark, frames: Long): List<FlashFrame> {
        val from = mark.lastFrameNumber + 1
        val to = mark.lastFrameNumber + frames
        store.awaitWindowComplete(from, to, WINDOW_TIMEOUT_MS)
        return store.window(from, to)
    }

    private fun ambientOfTail(phase: String): Ambient? = FlashAnalysis.ambientOf(store.framesOfPhase(phase).takeLast(AMBIENT_TAIL_FRAMES))

    /** Опрашивает хранилище, пока в фазе не наберётся [count] освещённых кадров; возвращает то, что набралось. */
    private fun awaitLitFrames(phase: String, ambient: Ambient, count: Int): List<FlashFrame> {
        val deadline = System.nanoTime() + LIT_WAIT_TIMEOUT_MS * NS_PER_MS
        while (System.nanoTime() < deadline) {
            val lit = store.framesOfPhase(phase).filter { f -> f.profile?.let { FlashAnalysis.isLit(it, ambient) } == true }
            if (lit.size >= count) return lit
            Thread.sleep(POLL_MS)
        }
        return store.framesOfPhase(phase).filter { f -> f.profile?.let { FlashAnalysis.isLit(it, ambient) } == true }
    }

    private companion object {
        const val REPETITIONS = 3
        const val NS_PER_MS = 1_000_000L
        const val FLASH_OFF = CameraMetadata.FLASH_MODE_OFF
        const val FLASH_SINGLE = CameraMetadata.FLASH_MODE_SINGLE
        const val FLASH_TORCH = CameraMetadata.FLASH_MODE_TORCH
        const val FIRED = CameraMetadata.FLASH_STATE_FIRED
        const val PREFLASH_EXPOSURE_NS = 4_000_000L
        const val LATENCY_WINDOW_FRAMES = 25L
        const val POST_SHOT_FRAMES = 20L
        const val LIT_TORCH_FRAMES_REQUIRED = 2
        const val AMBIENT_TAIL_FRAMES = 20
        const val ONE_SHOT_TIMEOUT_MS = 3_000L
        const val WINDOW_TIMEOUT_MS = 4_000L
        const val LIT_WAIT_TIMEOUT_MS = 3_000L
        const val POLL_MS = 3L
    }
}
