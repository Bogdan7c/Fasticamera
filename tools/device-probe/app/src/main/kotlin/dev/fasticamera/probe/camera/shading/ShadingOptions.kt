package dev.fasticamera.probe.camera.shading

import dev.fasticamera.probe.scenario.ScenarioArgs

/** Параметры сценария `shading`; разбор строгий: неверное значение — исключение с именем параметра. */
data class ShadingOptions(
    val mode: ShadingMode,
    /** Сколько кадров собирать в каждой фазе. */
    val framesPerPhase: Int,
    /** Сколько кадров нового запроса пропустить перед сбором (HAL применяет значения с задержкой). */
    val settleFrames: Int,
    /** Целевой уровень центра (доля от белого после вычитания чёрного). */
    val targetCentreLevel: Double,
    val phases: List<ShadingPhaseId>,
) {
    companion object {
        const val DEFAULT_FRAMES = 16
        const val DEFAULT_SETTLE_FRAMES = 15
        const val DEFAULT_TARGET_LEVEL = 0.40

        fun fromArgs(args: ScenarioArgs): ShadingOptions {
            val options = ShadingOptions(
                mode = ShadingMode.fromWire(args.requireString("mode")),
                framesPerPhase = args.int("frames", DEFAULT_FRAMES),
                settleFrames = args.int("settle", DEFAULT_SETTLE_FRAMES),
                targetCentreLevel = args.double("target", DEFAULT_TARGET_LEVEL),
                phases = args.stringList("phases", ShadingPhaseId.entries.map { it.wireName }).map(ShadingPhaseId::fromWire),
            )
            require(options.framesPerPhase > 0) { "frames должен быть > 0" }
            require(options.settleFrames >= 0) { "settle должен быть >= 0" }
            require(options.targetCentreLevel > 0.0 && options.targetCentreLevel < ExposureSearch.CLIP_LIMIT) {
                "target должен быть в (0; ${ExposureSearch.CLIP_LIMIT})"
            }
            require(options.phases.isNotEmpty()) { "phases пуст" }
            return options
        }
    }
}
