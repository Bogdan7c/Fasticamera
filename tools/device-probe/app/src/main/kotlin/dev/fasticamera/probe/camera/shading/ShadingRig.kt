package dev.fasticamera.probe.camera.shading

import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CaptureFailure
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.Image
import android.media.ImageReader
import android.os.Handler
import android.util.Range
import android.view.Surface
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.RawPassport
import dev.fasticamera.probe.camera.dual.LegRecorder
import org.json.JSONObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.atomic.AtomicInteger

/**
 * Камера в продуктовой конфигурации: один RAW10-поток + пустой YUV-компаньон в сессии,
 * повторяющийся запрос с управлением фазы.
 *
 * Владеет устройством, сессией, ImageReader и хранилищем результатов. Картинки отдаёт через [images]
 * (очередь на 1 кадр: пока сборщик занят распаковкой, лишние кадры возвращаются камере сразу,
 * поэтому колбэки камеры никогда не ждут обработку).
 */
class ShadingRig(
    private val mode: ShadingMode,
    private val focus: FocusCapabilities,
    private val runner: CameraSessionRunner,
    private val cameraHandler: Handler,
    private val imageHandler: Handler,
    private val timeoutMs: Long,
) {
    /** Результаты последних кадров, ключ — метка сенсора. */
    val results = FrameResultStore<TotalCaptureResult>(RESULT_CAPACITY)

    /** Кадры RAW10 для сборщика. Закрывает картинку тот, кто её взял. */
    val images = ArrayBlockingQueue<Image>(1)

    private val rawReader: ImageReader = mode.rawLeg().let { leg ->
        ImageReader.newInstance(leg.width, leg.height, leg.format.imageFormat, MAX_IMAGES)
    }

    /** Компаньон: выход сессии, который не является целью запроса; кадры возвращаются сразу. */
    private val companion = LegRecorder(mode.companionLeg(), imageHandler, discardFrames = true)

    private val listenerDroppedBusy = AtomicInteger(0)
    private val acquireErrors = AtomicInteger(0)
    private val resultsWithoutTimestamp = AtomicInteger(0)
    private val captureFailures = AtomicInteger(0)
    private val buffersLost = AtomicInteger(0)

    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var requestFactory: ShadingRequestFactory? = null

    /** Открывает камеру и создаёт сессию; любой отказ — исключение с исходом Camera2 (шаг сценария не имеет смысла дальше). */
    fun start(characteristics: CameraCharacteristics) {
        rawReader.setOnImageAvailableListener({ reader -> offerImage(reader) }, imageHandler)
        val opened = runner.open(mode.cameraId, timeoutMs)
        check(opened is CameraSessionRunner.OpenOutcome.Opened) { "камера ${mode.cameraId} не открылась: ${opened.toJson()}" }
        device = opened.device
        val outputs = listOf(OutputConfiguration(rawReader.surface), OutputConfiguration(companion.surface))
        val factory = ShadingRequestFactory(mode, focus, characteristics)
        requestFactory = factory
        val sessionParameters = factory
            .build(opened.device, emptyList(), WARMUP_CONTROLS, SESSION_PARAMETERS_GENERATION)
        val configured = runner.createSession(opened.device, outputs, SessionConfiguration.SESSION_REGULAR, sessionParameters, timeoutMs)
        check(configured is CameraSessionRunner.SessionOutcome.Configured) { "сессия не создана: ${configured.toJson()}" }
        session = configured.session
    }

    /** Применяет управление фазы повторяющимся запросом; кадры этого запроса несут тег [generation]. */
    fun applyControls(controls: PhaseControls, generation: Int) {
        val activeSession = checkNotNull(session) { "сессия не создана" }
        val factory = checkNotNull(requestFactory)
        val request = factory.build(checkNotNull(device), listOf(rawReader.surface), controls, generation)
        activeSession.setRepeatingRequest(request, resultCallback, cameraHandler)
    }

    private fun offerImage(reader: ImageReader) {
        val image = try {
            reader.acquireNextImage()
        } catch (_: IllegalStateException) {
            // maxImages исчерпан: считаем, а не глотаем.
            acquireErrors.incrementAndGet()
            null
        } ?: return
        if (!images.offer(image)) {
            // Сборщик занят предыдущим кадром: возвращаем буфер камере немедленно.
            image.close()
            listenerDroppedBusy.incrementAndGet()
        }
    }

    private val resultCallback = object : CameraCaptureSession.CaptureCallback() {
        override fun onCaptureCompleted(session: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
            val timestampNs = result.get(CaptureResult.SENSOR_TIMESTAMP)
            if (timestampNs == null) {
                resultsWithoutTimestamp.incrementAndGet()
                return
            }
            val generation = request.tag as? Int ?: UNTAGGED_GENERATION
            results.put(timestampNs, generation, result.frameNumber, result)
        }

        override fun onCaptureFailed(session: CameraCaptureSession, request: CaptureRequest, failure: CaptureFailure) {
            captureFailures.incrementAndGet()
        }

        override fun onCaptureBufferLost(session: CameraCaptureSession, request: CaptureRequest, target: Surface, frameNumber: Long) {
            buffersLost.incrementAndGet()
        }
    }

    /** Счётчики потерь за весь прогон — в отчёт. */
    fun countersJson(): JSONObject = JSONObject()
        .put("listener_dropped_busy", listenerDroppedBusy.get())
        .put("acquire_errors", acquireErrors.get())
        .put("results_without_timestamp", resultsWithoutTimestamp.get())
        .put("capture_failures", captureFailures.get())
        .put("buffers_lost", buffersLost.get())

    /** Сколько кадров вернули камере сразу, потому что сборщик был занят. */
    val droppedWhileBusy: Int get() = listenerDroppedBusy.get()

    /** Полная остановка; безопасно вызывать повторно и после частичного старта. */
    fun close() {
        session?.let { activeSession ->
            try {
                activeSession.stopRepeating()
            } catch (_: Exception) {
                // Сессия могла умереть вместе с устройством; закрыть её всё равно нужно.
            }
            activeSession.close()
        }
        session = null
        device?.close()
        device = null
        generateSequence { images.poll() }.forEach(Image::close)
        rawReader.close()
        companion.close()
    }

    companion object {
        /** Управление первого запроса и параметров сессии: вендорский AE, карта запрошена, непрерывный AF. */
        val WARMUP_CONTROLS = PhaseControls(ExposureControl.VendorAuto, lensShadingMapRequested = true, focus = FocusControl.CONTINUOUS_PICTURE)

        private const val RESULT_CAPACITY = 256
        private const val MAX_IMAGES = 4
        private const val UNTAGGED_GENERATION = -1
        private const val SESSION_PARAMETERS_GENERATION = 0
    }
}

/** Строит запросы фазы: общие продуктовые настройки режима + управление конкретной фазы. */
class ShadingRequestFactory(
    private val mode: ShadingMode,
    private val focus: FocusCapabilities,
    private val characteristics: CameraCharacteristics,
) {
    fun build(device: CameraDevice, targets: List<Surface>, controls: PhaseControls, generation: Int): CaptureRequest {
        val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
        targets.forEach(builder::addTarget)
        mode.aeTargetFps?.let { fps -> builder.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(fps, fps)) }
        applyExposure(builder, controls.exposure)
        // Чистый RAW как в паспорте C7 (NR и горячие пиксели выключены); SHADING_MODE не трогаем — продукт его не трогает.
        RawPassport.applyCleanRawControls(builder)
        builder.set(
            CaptureRequest.STATISTICS_LENS_SHADING_MAP_MODE,
            if (controls.lensShadingMapRequested) CaptureRequest.STATISTICS_LENS_SHADING_MAP_MODE_ON else CaptureRequest.STATISTICS_LENS_SHADING_MAP_MODE_OFF,
        )
        applyFocus(builder, controls.focus)
        // Ключи не проглатываем: ошибка типа/тега прерывает шаг и попадает в отчёт.
        mode.vendorKeys().forEach { key -> key.applyTo(builder) }
        builder.setTag(generation)
        return builder.build()
    }

    private fun applyExposure(builder: CaptureRequest.Builder, exposure: ExposureControl) {
        when (exposure) {
            ExposureControl.VendorAuto -> builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON)
            is ExposureControl.Manual -> {
                builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_OFF)
                builder.set(CaptureRequest.SENSOR_EXPOSURE_TIME, exposure.setting.exposureNs)
                builder.set(CaptureRequest.SENSOR_SENSITIVITY, exposure.setting.iso)
                builder.set(CaptureRequest.SENSOR_FRAME_DURATION, mode.frameDurationNs)
            }
        }
    }

    private fun applyFocus(builder: CaptureRequest.Builder, control: FocusControl) {
        when (control) {
            FocusControl.CONTINUOUS_PICTURE -> {
                // На камерах с фиксированным фокусом режим непрерывного AF недоступен: не задаём его (причина видна в af_available_modes).
                val supported = characteristics.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)
                    ?.contains(CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE) == true
                if (supported) builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
            }
            FocusControl.INFINITY -> manualFocus(builder, INFINITY_DIOPTERS)
            FocusControl.NEAREST -> manualFocus(
                builder,
                checkNotNull(focus.minFocusDistanceDiopters) { "минимальная дистанция фокуса неизвестна" },
            )
        }
    }

    private fun manualFocus(builder: CaptureRequest.Builder, diopters: Float) {
        builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_OFF)
        builder.set(CaptureRequest.LENS_FOCUS_DISTANCE, diopters)
    }

    private companion object {
        /** LENS_FOCUS_DISTANCE в диоптриях: 0 = бесконечность. */
        const val INFINITY_DIOPTERS = 0f
    }
}
