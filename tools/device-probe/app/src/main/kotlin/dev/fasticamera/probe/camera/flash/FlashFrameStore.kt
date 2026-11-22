package dev.fasticamera.probe.camera.flash

import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.media.Image
import dev.fasticamera.probe.camera.threea.ThreeAFrameObserver
import dev.fasticamera.probe.camera.threea.ThreeAFrameRow
import dev.fasticamera.probe.camera.threea.ThreeATag
import dev.fasticamera.probe.report.ProbeLog
import java.util.concurrent.ConcurrentHashMap

/**
 * Хранилище кадров сценария `flash`: клеит результат и RAW-картинку по SENSOR_TIMESTAMP.
 * Владеет только данными кадров; решения «что считать вспышкой» принимает [FlashAnalysis].
 * Результаты и картинки приходят на разных потоках, поэтому внутри потокобезопасные карты.
 */
class FlashFrameStore(
    private val log: ProbeLog,
    private val bandSettings: RawBandSettings,
) : ThreeAFrameObserver {

    private val resultsByTimestamp = ConcurrentHashMap<Long, FlashResultInfo>()
    private val profilesByTimestamp = ConcurrentHashMap<Long, RawBandProfile>()

    override fun onResult(row: ThreeAFrameRow, result: TotalCaptureResult, arrivedNs: Long) {
        val timestamp = row.sensorTimestampNs ?: return // без метки кадр не склеить с картинкой
        resultsByTimestamp[timestamp] = FlashResultInfo(
            tag = row.tag,
            frameNumber = row.frameNumber,
            sensorTimestampNs = timestamp,
            exposureNs = row.exposureNs,
            iso = row.iso,
            flashState = result.get(CaptureResult.FLASH_STATE),
            flashMode = result.get(CaptureResult.FLASH_MODE),
            sensorMode = row.json.opt("v_current_mode"),
            captureIntent = result.get(CaptureResult.CONTROL_CAPTURE_INTENT),
            flashStrength = result.get(CaptureResult.FLASH_STRENGTH_LEVEL),
            arrivedNs = arrivedNs,
        )
    }

    override fun onImage(image: Image, arrivedNs: Long) {
        val plane = image.planes.firstOrNull() ?: return
        try {
            profilesByTimestamp[image.timestamp] =
                RawBandAnalyzer.analyze(plane.buffer.duplicate(), plane.rowStride, image.width, image.height, bandSettings)
        } catch (error: RuntimeException) {
            // Ошибку чтения кадра не глотаем: в отчёте будет и событие, и кадр без профиля.
            log.error("flash_profile", error)
        }
    }

    /** Все кадры, у которых есть результат, по возрастанию номера кадра. */
    fun allFrames(): List<FlashFrame> =
        resultsByTimestamp.values.sortedBy { it.frameNumber }.map { FlashFrame(it, profilesByTimestamp[it.sensorTimestampNs]) }

    fun framesOfPhase(phase: String): List<FlashFrame> = allFrames().filter { it.result.tag.phase == phase }

    /** Кадры с номерами от [fromFrame] до [toFrame] включительно (отсутствующие номера просто не попадают). */
    fun window(fromFrame: Long, toFrame: Long): List<FlashFrame> =
        allFrames().filter { it.result.frameNumber in fromFrame..toFrame }

    /** Результат одиночного запроса (kind=TRIGGER) данной фазы и круга, если уже пришёл. */
    fun oneShotResult(phase: String, round: Int): FlashResultInfo? =
        resultsByTimestamp.values.firstOrNull { it.tag.kind == ThreeATag.Kind.TRIGGER && it.tag.phase == phase && it.tag.round == round }

    /** Ждёт, пока в фазе накопится [count] кадров вместе с RAW-профилем; false — не дождались за [timeoutMs]. */
    fun awaitPhaseFrames(phase: String, count: Int, timeoutMs: Long): Boolean =
        pollUntil(timeoutMs) { framesOfPhase(phase).count { it.profile != null } >= count }

    /** Ждёт одиночный результат (см. [oneShotResult]). */
    fun awaitOneShotResult(phase: String, round: Int, timeoutMs: Long): FlashResultInfo? {
        pollUntil(timeoutMs) { oneShotResult(phase, round) != null }
        return oneShotResult(phase, round)
    }

    /** Ждёт, пока по окну [fromFrame]..[toFrame] придут результат и RAW всех кадров. */
    fun awaitWindowComplete(fromFrame: Long, toFrame: Long, timeoutMs: Long): Boolean = pollUntil(timeoutMs) {
        val frames = window(fromFrame, toFrame)
        frames.any { it.result.frameNumber >= toFrame } && frames.all { it.profile != null }
    }

    private fun pollUntil(timeoutMs: Long, condition: () -> Boolean): Boolean {
        val deadline = System.nanoTime() + timeoutMs * NS_PER_MS
        while (System.nanoTime() < deadline) {
            if (condition()) return true
            Thread.sleep(POLL_INTERVAL_MS)
        }
        return condition()
    }

    private companion object {
        const val NS_PER_MS = 1_000_000L
        const val POLL_INTERVAL_MS = 20L
    }
}
