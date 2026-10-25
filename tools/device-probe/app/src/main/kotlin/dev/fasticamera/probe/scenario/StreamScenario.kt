package dev.fasticamera.probe.scenario

import dev.fasticamera.probe.camera.RawOutput
import dev.fasticamera.probe.camera.StreamExperiment
import dev.fasticamera.probe.camera.StreamExperimentConfig
import dev.fasticamera.probe.platform.ThermalSnapshot

/**
 * C2/C3/C8: одна попытка получить поток с заданными камерой, форматом, размером и рычагами.
 *
 * Одна попытка = один запуск Activity: если HAL уронит процесс камеры, пострадает
 * только эта строка таблицы, а ПК-скрипт увидит отсутствие `done` и запишет сбой.
 */
class StreamScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val config = StreamExperimentConfig.fromArgs(context.args)
        context.log.event("config", config.toJson())
        val thermalBefore = ThermalSnapshot.capture(context.appContext)
        val experiment = StreamExperiment(context.newSessionRunner(), context.imageHandler) { stage, details -> context.log.event(stage, details) }
        val report = experiment.run(config, RawOutput.JavaReader)
        report.put("thermal_before", thermalBefore)
        report.put("thermal_after", ThermalSnapshot.capture(context.appContext))
        context.log.result("stream", report)
    }
}
