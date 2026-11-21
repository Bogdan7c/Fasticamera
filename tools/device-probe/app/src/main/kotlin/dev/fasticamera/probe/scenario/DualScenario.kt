package dev.fasticamera.probe.scenario

import dev.fasticamera.probe.camera.dual.AlignmentPlan
import dev.fasticamera.probe.camera.dual.BringUpOrder
import dev.fasticamera.probe.camera.dual.ConcurrentStreamExperiment
import dev.fasticamera.probe.camera.dual.CurrentSampler
import dev.fasticamera.probe.camera.dual.ExposureStep
import dev.fasticamera.probe.camera.dual.LegSpec
import dev.fasticamera.probe.camera.dual.PreopenSwitchExperiment
import dev.fasticamera.probe.camera.dual.ProductDualExperiment
import dev.fasticamera.probe.camera.dual.SensitivityPlan
import dev.fasticamera.probe.platform.ThermalSnapshot
import java.io.File

/**
 * Сессия 13, пакет F: можно ли стримить две камеры одновременно и сколько стоит переключение.
 *
 * Режимы (`mode=`):
 * - `concurrent` (по умолчанию): `legs=2:yuv:640x480,3:yuv:640x480 [fps=30] [seconds=10]` —
 *   открыть все камеры, создать сессии, стримить; ноги с одним id камеры = одна сессия
 *   (логическая камера: `legs=0:yuv:640x480:2,0:yuv:640x480:3`);
 * - `product*`: см. [runProduct]; у `product` и `product_sensitivity` есть `order=main_first|wide_first`;
 * - `preopen`: `legs=2:raw10:4096x3072,3:raw10:4000x3000 [reps=5]` — первая нога стримит,
 *   вторая — цель переключения; сравнение «предоткрытая камера» против «закрыть и открыть».
 *
 * Как и другие сценарии, одна команда = один запуск Activity: падение HAL повредит только эту строку.
 */
class DualScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val mode = context.args.string("mode") ?: "concurrent"
        if (mode.startsWith("product")) {
            runProduct(context, mode)
            return
        }
        val legs = LegSpec.parseList(context.args.stringList("legs", emptyList()))
        require(legs.isNotEmpty()) { "нет параметра legs (камера:формат:ШxВ[:физическая], через запятую)" }
        val thermalBefore = ThermalSnapshot.capture(context.appContext)
        val progress = { stage: String, details: org.json.JSONObject -> context.log.event(stage, details) }
        val fps = context.args.intOrNull("fps")
        val stepTimeoutMs = context.args.long("step_timeout_ms", DEFAULT_STEP_TIMEOUT_MS)
        val report = when (val mode = context.args.string("mode") ?: "concurrent") {
            "concurrent" -> ConcurrentStreamExperiment(
                context.newSessionRunner(), context.imageHandler,
                currentSampler = CurrentSampler(context.appContext).takeIf { it.isOnBattery },
                progress = progress,
            ).run(
                ConcurrentStreamExperiment.Plan(
                    legs = legs, aeTargetFps = fps, seconds = context.args.int("seconds", DEFAULT_SECONDS),
                    stepTimeoutMs = stepTimeoutMs, settleMs = context.args.long("settle_ms", DEFAULT_SETTLE_MS),
                ),
            )
            "preopen" -> {
                require(legs.size == 2) { "для mode=preopen нужно ровно две ноги: стримящая и целевая" }
                PreopenSwitchExperiment(context.newSessionRunner(), context.imageHandler, progress).run(
                    PreopenSwitchExperiment.Plan(
                        streaming = legs[0], target = legs[1], aeTargetFps = fps,
                        repetitions = context.args.int("reps", DEFAULT_REPETITIONS),
                        stepTimeoutMs = stepTimeoutMs, warmupMs = context.args.long("warmup_ms", DEFAULT_WARMUP_MS),
                    ),
                )
            }
            else -> throw IllegalArgumentException("неизвестный mode '$mode' (concurrent|preopen)")
        }
        report.put("mode", context.args.string("mode") ?: "concurrent")
        report.put("fps_requested", fps ?: org.json.JSONObject.NULL)
        report.put("thermal_before", thermalBefore)
        report.put("thermal_after", ThermalSnapshot.capture(context.appContext))
        context.log.result("dual", report)
    }

    /** Продуктовые режимы (сессия 13, добавка F2): `product`, `product_standby`, `product_abba`, `product_sensitivity`, `product_alignment`. */
    private fun runProduct(context: ScenarioContext, mode: String) {
        val experiment = ProductDualExperiment(
            context.newSessionRunner(), context.imageHandler, context.appContext,
            context.args.long("step_timeout_ms", DEFAULT_STEP_TIMEOUT_MS),
        ) { stage, details -> context.log.event(stage, details) }
        val report = when (mode) {
            "product" -> experiment.streamBoth(
                context.args.int("seconds", PRODUCT_SECONDS), discardWide = context.args.int("discard_wide", 0) == 1,
                order = productOrder(context),
            )
            "product_sensitivity" -> experiment.measureSensitivity(
                SensitivityPlan(
                    steps = ExposureStep.parseList(context.args.stringList("exposures", DEFAULT_EXPOSURES)),
                    settleFrames = context.args.int("settle_frames", DEFAULT_SETTLE_FRAMES),
                    sampleFrames = context.args.int("sample_frames", DEFAULT_SAMPLE_FRAMES),
                    fovFraction = context.args.string("fov_fraction")?.toDouble() ?: DEFAULT_FOV_FRACTION,
                    order = productOrder(context),
                ),
            )
            "product_alignment" -> experiment.measureAlignment(
                AlignmentPlan(
                    pairs = context.args.int("pairs", ALIGN_PAIRS),
                    downsample = context.args.int("downsample", ALIGN_DOWNSAMPLE),
                    settleMs = context.args.long("settle_ms", ALIGN_SETTLE_MS),
                    mainExposureNs = context.args.long("exposure_ns", ALIGN_EXPOSURE_NS),
                    iso = context.args.int("iso", ALIGN_ISO),
                    order = productOrder(context),
                    marginFraction = context.args.string("margin")?.toDouble() ?: ALIGN_MARGIN,
                    outputDirectory = File(context.appContext.filesDir, "align/${context.runId}"),
                ),
            )
            "product_standby" -> experiment.standbyStart(context.args.int("reps", DEFAULT_REPETITIONS))
            "product_abba" -> experiment.currentAbba(
                order = context.args.string("order") ?: "ABBAABBA",
                windowSeconds = context.args.int("seconds", ABBA_WINDOW_SECONDS),
                source = CurrentSampler.Source.entries.first { it.wireName == (context.args.string("current") ?: "now") },
            )
            else -> throw IllegalArgumentException("неизвестный mode '$mode'")
        }
        report.put("mode", mode)
        report.put("thermal_after", ThermalSnapshot.capture(context.appContext))
        context.log.result("dual", report)
    }

    /** `order=main_first|wide_first`: какая камера поднимается первой (по умолчанию основная). */
    private fun productOrder(context: ScenarioContext): BringUpOrder =
        context.args.string("order")?.let(BringUpOrder::fromWire) ?: BringUpOrder.MAIN_FIRST

    private companion object {
        /** Ступени замера чувствительности по умолчанию: выдержка_нс:iso. */
        val DEFAULT_EXPOSURES = listOf("2000000:100", "4000000:100", "1000000:400")
        const val DEFAULT_SETTLE_FRAMES = 30
        const val DEFAULT_SAMPLE_FRAMES = 20
        const val DEFAULT_FOV_FRACTION = 0.4
        /** Совмещение кадров: пар, блок уменьшения в сырых пикселях, пауза на AF, выдержка/ISO основной, запас вырезки ширика. */
        const val ALIGN_PAIRS = 5
        const val ALIGN_DOWNSAMPLE = 4
        const val ALIGN_SETTLE_MS = 2_500L
        const val ALIGN_EXPOSURE_NS = 4_000_000L
        const val ALIGN_ISO = 400
        const val ALIGN_MARGIN = 0.25
        const val PRODUCT_SECONDS = 30
        const val ABBA_WINDOW_SECONDS = 20
        const val DEFAULT_SECONDS = 10
        const val DEFAULT_STEP_TIMEOUT_MS = 8_000L
        const val DEFAULT_SETTLE_MS = 700L
        const val DEFAULT_REPETITIONS = 5
        const val DEFAULT_WARMUP_MS = 800L
    }
}
