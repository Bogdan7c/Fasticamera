package dev.fasticamera.probe.camera.dual

import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CaptureFailure
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.os.Handler
import android.util.Range
import android.view.Surface
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.FrameTimingStats
import dev.fasticamera.probe.camera.ManualExposure
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * Одна камера эксперимента: устройство + сессия + приёмники кадров для всех её ног.
 *
 * Жизненный цикл разнесён по шагам ([open] → [configureSession] → [startStreaming] → [close]),
 * потому что эксперименты «предварительно открытая камера» меряют именно паузы между шагами.
 * Каждый шаг возвращает исход Camera2, а не бросает исключение: отказ — это результат разведки.
 */
class CameraRig(
    val cameraId: String,
    private val legs: List<LegSpec>,
    private val profile: RigProfile,
    private val runner: CameraSessionRunner,
    private val imageHandler: Handler,
    private val timeoutMs: Long,
    /** Потребитель содержимого кадров основной (первой) ноги камеры; null — только счётчики. */
    frameSink: RawFrameSink? = null,
) {
    private val recorders: List<LegRecorder> = legs.mapIndexed { index, leg ->
        LegRecorder(leg, imageHandler, profile.discardFrames, frameSink.takeIf { index == 0 })
    }

    /** Компаньон: выход в сессии, который никогда не бывает целью запроса (оживляет 3A Qualcomm). */
    private val companion: LegRecorder? = profile.companionYuv?.let { LegRecorder(it, imageHandler, discardFrames = true) }
    private val captureStats = CaptureStats()
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null

    /** Текущая ручная экспозиция: стартует из профиля и меняется через [applyManualExposure]. */
    private var currentManualExposure: ManualExposure? = profile.manualExposure

    val firstFrameAtNs: Long? get() = recorders.mapNotNull { it.firstFrameAtNs }.minOrNull()

    val framesReceived: Int get() = recorders.sumOf { it.framesReceived }

    fun open(): CameraSessionRunner.OpenOutcome {
        val outcome = runner.open(cameraId, timeoutMs)
        if (outcome is CameraSessionRunner.OpenOutcome.Opened) device = outcome.device
        return outcome
    }

    /** Сессия со всеми ногами камеры; физические потоки помечаются через setPhysicalCameraId. */
    fun configureSession(): CameraSessionRunner.SessionOutcome {
        val opened = checkNotNull(device) { "камера $cameraId не открыта" }
        val outputs = legs.zip(recorders).map { (spec, recorder) ->
            OutputConfiguration(recorder.surface).apply { spec.physicalCameraId?.let(::setPhysicalCameraId) }
        } + listOfNotNull(companion).map { OutputConfiguration(it.surface) }
        val outcome = runner.createSession(
            opened, outputs, SessionConfiguration.SESSION_REGULAR, buildSessionParameters(opened), timeoutMs,
        )
        if (outcome is CameraSessionRunner.SessionOutcome.Configured) session = outcome.session
        return outcome
    }

    /** Запускает повторяющийся запрос на все поверхности сессии. */
    fun startStreaming() {
        val activeSession = checkNotNull(session) { "сессия камеры $cameraId не создана" }
        val opened = checkNotNull(device)
        val request = buildStreamRequest(opened, recorders.map(LegRecorder::surface))
        activeSession.setRepeatingRequest(request, captureStats, imageHandler)
    }

    /**
     * Меняет ручную экспозицию на лету: пересобирает повторяющийся запрос и перезапускает его.
     * Сессия и вендорские ключи остаются теми же; HAL применит значения с задержкой в несколько кадров,
     * поэтому о применении судят по результатам ([latestResultInfo]), а не по этому вызову.
     */
    fun applyManualExposure(exposure: ManualExposure) {
        currentManualExposure = exposure
        startStreaming()
    }

    /** Применённые значения последнего результата или null, пока результатов нет. */
    fun latestResultInfo(): FrameResultInfo? = captureStats.latestResult()

    /** Результат кадра с меткой сенсора [timestampNs] (метка Image == SENSOR_TIMESTAMP) или null, если не пришёл/вытеснен. */
    fun resultInfoFor(timestampNs: Long): FrameResultInfo? = captureStats.resultFor(timestampNs)

    /** Закрывает только сессию (устройство остаётся открытым — вариант «предоткрытая камера»). */
    fun closeSession() {
        val activeSession = session ?: return
        session = null
        try {
            activeSession.stopRepeating()
        } catch (_: Exception) {
            // Сессия могла уже умереть вместе с устройством: тогда закрыть её всё равно нужно.
        }
        activeSession.close()
    }

    fun closeDevice() {
        device?.close()
        device = null
    }

    /** Полная остановка: сессия, устройство, приёмники. Безопасно вызывать повторно. */
    fun close() {
        closeSession()
        closeDevice()
        recorders.forEach(LegRecorder::close)
        companion?.close()
    }

    fun awaitFirstFrame(): Boolean = recorders.all { it.awaitFirstFrame(timeoutMs) }

    fun toJson(): JSONObject = JSONObject()
        .put("camera", cameraId)
        .put("legs", JSONArray(recorders.map(LegRecorder::toJson)))
        .put("results", captureStats.toJson())

    /** Сессионные параметры: fps-диапазон и вендорские ключи профиля (HAL выбирает режим сенсора по ним). */
    private fun buildSessionParameters(opened: CameraDevice): CaptureRequest? {
        if (!profile.hasSessionParameters) return null
        return buildStreamRequest(opened, emptyList())
    }

    private fun buildStreamRequest(opened: CameraDevice, targets: List<Surface>): CaptureRequest {
        val builder = opened.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
        targets.forEach(builder::addTarget)
        profile.aeTargetFps?.let { fps -> builder.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(fps, fps)) }
        currentManualExposure?.let { manual ->
            builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_OFF)
            builder.set(CaptureRequest.SENSOR_EXPOSURE_TIME, manual.exposureTimeNs)
            builder.set(CaptureRequest.SENSOR_SENSITIVITY, manual.sensitivityIso)
            builder.set(CaptureRequest.SENSOR_FRAME_DURATION, manual.frameDurationNs)
        }
        if (profile.afContinuous) {
            builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
        }
        // Ключи не проглатываем молча: ошибка фреймворка (неверный тип/тег) прерывает шаг и попадает в отчёт.
        profile.vendorKeys.forEach { key -> key.applyTo(builder) }
        return builder.build()
    }

    /** Колбэк результатов: метки сенсора, отказы, потерянные буферы и число частичных результатов по физическим камерам. */
    private class CaptureStats : CameraCaptureSession.CaptureCallback() {
        private val lock = Any()
        private val resultTimestampsNs = ArrayList<Long>()
        private val failureReasons = ConcurrentHashMap<String, AtomicInteger>()
        private val physicalResultCounts = ConcurrentHashMap<String, AtomicInteger>()
        private val lostBuffers = AtomicInteger(0)
        private val afTransitions = ArrayList<List<Any>>()
        private var lastAfState: Int? = null
        private var resultIndex = 0
        private val recentResults = object : LinkedHashMap<Long, FrameResultInfo>() {
            override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Long, FrameResultInfo>?) = size > RECENT_RESULTS_LIMIT
        }
        private var latestInfo: FrameResultInfo? = null

        fun latestResult(): FrameResultInfo? = synchronized(lock) { latestInfo }

        fun resultFor(timestampNs: Long): FrameResultInfo? = synchronized(lock) { recentResults[timestampNs] }

        /** Пишем только смены AF_STATE: [номер результата, имя состояния] — видно, сошёлся ли автофокус. */
        private fun recordAfState(state: Int?) {
            synchronized(lock) {
                resultIndex++
                if (state != null && state != lastAfState) {
                    lastAfState = state
                    afTransitions += listOf(resultIndex, afStateName(state))
                }
            }
        }

        override fun onCaptureCompleted(session: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
            recordAfState(result.get(CaptureResult.CONTROL_AF_STATE))
            val timestampNs = result.get(CaptureResult.SENSOR_TIMESTAMP)
            timestampNs?.let { synchronized(lock) { resultTimestampsNs += it } }
            val info = FrameResultInfo(
                exposureTimeNs = result.get(CaptureResult.SENSOR_EXPOSURE_TIME),
                sensitivityIso = result.get(CaptureResult.SENSOR_SENSITIVITY),
                dynamicBlackLevel = result.get(CaptureResult.SENSOR_DYNAMIC_BLACK_LEVEL),
                afState = result.get(CaptureResult.CONTROL_AF_STATE),
                focusDistanceDiopters = result.get(CaptureResult.LENS_FOCUS_DISTANCE),
                lensState = result.get(CaptureResult.LENS_STATE),
            )
            synchronized(lock) {
                latestInfo = info
                if (timestampNs != null) recentResults[timestampNs] = info
            }
            for (physicalId in result.physicalCameraTotalResults.keys) {
                physicalResultCounts.getOrPut(physicalId) { AtomicInteger(0) }.incrementAndGet()
            }
        }

        override fun onCaptureFailed(session: CameraCaptureSession, request: CaptureRequest, failure: CaptureFailure) {
            val reason = if (failure.reason == CaptureFailure.REASON_ERROR) "REASON_ERROR" else "REASON_FLUSHED"
            failureReasons.getOrPut(reason) { AtomicInteger(0) }.incrementAndGet()
        }

        override fun onCaptureBufferLost(session: CameraCaptureSession, request: CaptureRequest, target: Surface, frameNumber: Long) {
            lostBuffers.incrementAndGet()
        }

        fun toJson(): JSONObject = synchronized(lock) {
            JSONObject()
                .put("count", resultTimestampsNs.size)
                .put("af_transitions", JSONArray(afTransitions.map { JSONArray(it) }))
                .put("timing", FrameTimingStats.compute(resultTimestampsNs).toJson())
                .put("failures", JSONObject(failureReasons.mapValues { it.value.get() } as Map<*, *>))
                .put("buffers_lost", lostBuffers.get())
                .put("physical_result_counts", JSONObject(physicalResultCounts.mapValues { it.value.get() } as Map<*, *>))
        }
    }
}

/** Сколько последних результатов держим для сопоставления с кадрами по метке сенсора. */
private const val RECENT_RESULTS_LIMIT = 512

/** Применённые HAL параметры одного кадра (из результата захвата); null — поле не пришло. */
class FrameResultInfo(
    val exposureTimeNs: Long?,
    val sensitivityIso: Int?,
    /** Динамический чёрный уровень по CFA-позициям в порядке чтения сенсора, если HAL его отдаёт. */
    val dynamicBlackLevel: FloatArray?,
    val afState: Int?,
    /** LENS_FOCUS_DISTANCE: диоптрии (1/м), 0 = бесконечность; нужен для оценки параллакса. */
    val focusDistanceDiopters: Float? = null,
    /** LENS_STATE (STATIONARY/MOVING) на момент кадра. */
    val lensState: Int? = null,
)

/** Читаемое имя CONTROL_AF_STATE для отчёта. */
internal fun afStateName(state: Int): String = when (state) {
    CaptureResult.CONTROL_AF_STATE_INACTIVE -> "INACTIVE"
    CaptureResult.CONTROL_AF_STATE_PASSIVE_SCAN -> "PASSIVE_SCAN"
    CaptureResult.CONTROL_AF_STATE_PASSIVE_FOCUSED -> "PASSIVE_FOCUSED"
    CaptureResult.CONTROL_AF_STATE_ACTIVE_SCAN -> "ACTIVE_SCAN"
    CaptureResult.CONTROL_AF_STATE_FOCUSED_LOCKED -> "FOCUSED_LOCKED"
    CaptureResult.CONTROL_AF_STATE_NOT_FOCUSED_LOCKED -> "NOT_FOCUSED_LOCKED"
    CaptureResult.CONTROL_AF_STATE_PASSIVE_UNFOCUSED -> "PASSIVE_UNFOCUSED"
    else -> "UNKNOWN_$state"
}
