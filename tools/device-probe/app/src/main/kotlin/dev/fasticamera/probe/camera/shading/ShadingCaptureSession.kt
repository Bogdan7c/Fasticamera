package dev.fasticamera.probe.camera.shading

/**
 * Применение управления + сбор кадров одной фазы. Ведёт счётчик поколений запросов:
 * кадры предыдущего запроса, всё ещё летящие в конвейере, по поколению отличаются от кадров нового.
 */
class ShadingCaptureSession(
    private val rig: ShadingRig,
    private val collector: ShadingFrameCollector,
    private val settleFrames: Int,
) {
    private var generation = 0

    /** Применяет управление без сбора (прогрев); возвращает поколение запроса. */
    fun apply(controls: PhaseControls): Int {
        generation++
        rig.applyControls(controls, generation)
        return generation
    }

    /** Применяет [controls], пропускает кадры успокоения и собирает [frames] кадров. */
    fun capture(controls: PhaseControls, frames: Int, denseConsumer: DenseFrameConsumer? = null): PhaseCollection {
        val applied = apply(controls)
        val request = CollectionRequest(applied, settleFrames, frames, captureTimeoutMs(frames), denseConsumer)
        return collector.collect(request)
    }

    /** Результат последнего кадра поколения [generation] либо null (нет результатов этого запроса). */
    fun latestResultOf(generation: Int) = rig.results.latestResult()?.takeIf { it.request.tag == generation }

    val bufferGeometry: BufferGeometry? get() = collector.geometry

    /** Запас по времени: на каждый кадр (включая успокоение) до 1,5 с — распаковка 12,6 Мпикс на телефоне медленная. */
    private fun captureTimeoutMs(frames: Int): Long = BASE_TIMEOUT_MS + (frames + settleFrames) * PER_FRAME_TIMEOUT_MS

    private companion object {
        const val BASE_TIMEOUT_MS = 10_000L
        const val PER_FRAME_TIMEOUT_MS = 1_500L
    }
}
