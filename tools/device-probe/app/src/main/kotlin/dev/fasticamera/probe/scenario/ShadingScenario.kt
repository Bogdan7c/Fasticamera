package dev.fasticamera.probe.scenario

import dev.fasticamera.probe.camera.shading.ShadingExperiment
import dev.fasticamera.probe.camera.shading.ShadingOptions

/**
 * Карта затенения объектива (сессия 14f): RAW из продуктовой конфигурации режима `mode=` на плоском поле
 * и полная карта STATISTICS_LENS_SHADING_CORRECTION_MAP по каждому кадру. Подробности и аргументы — README инструмента.
 */
class ShadingScenario : ProbeScenario {
    override fun run(context: ScenarioContext) {
        ShadingExperiment(context, ShadingOptions.fromArgs(context.args)).run()
    }
}
