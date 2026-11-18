package dev.fasticamera.probe.camera.threea

import android.graphics.Rect
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.MeteringRectangle
import dev.fasticamera.probe.camera.VendorKeyValue
import dev.fasticamera.probe.report.ProbeLog
import org.json.JSONArray
import org.json.JSONObject

/** Настройки сценариев фаз: все числа вынесены сюда, в коде фаз нет «магических» значений. */
data class ThreeAPhaseSettings(
    /** Длительность обычной фазы. */
    val holdMs: Long,
    /** Сколько ждать первого кадра в фазе, прежде чем объявить ошибку «нет кадров». */
    val firstFrameGraceMs: Long,
    val activeArray: Rect,
    val frameDurationNs: Long,
    /** Выдержка фазы age_test (диагностика возраста кадра). */
    val ageTestExposureNs: Long = 8_000_000L,
)

/**
 * Сценарий 7 фаз 3A-разведки. Каждая фаза: сменить повторяющийся запрос → подержать → свести результаты.
 * Исключение внутри фазы пишется в JSONL (ошибка с именем фазы), остальные фазы продолжаются.
 */
class ThreeAPhases(
    private val session: ThreeASession,
    private val recorder: ThreeARecorder,
    private val log: ProbeLog,
    private val settings: ThreeAPhaseSettings,
    private val rawProbe: ThreeARaw10Probe? = null,
) {
    val summaries = mutableListOf<ThreeAPhaseSummary>()

    /**
     * Порядок важен: вендорские теги iso_exp_priority «липкие» (HAL держит последнее значение,
     * пока его не перезапишут), поэтому вендорская фаза идёт ПОСЛЕДНЕЙ и не загрязняет остальные.
     * [only] — необязательный фильтр имён фаз (для перепрогона части).
     */
    fun runAll(only: Set<String>? = null) {
        val plan = listOf<Pair<String, () -> Unit>>(
            "baseline" to ::baseline,
            "af_trigger_center" to { afTrigger("af_trigger_center", centerRegion()) },
            "af_trigger_corner" to { afTrigger("af_trigger_corner", topLeftRegion()) },
            "ev_comp" to ::evComp,
            "ae_lock" to ::aeLock,
            "manual_1_1000" to ::manual,
            "burst" to ::burst,
            "vendor_iso_priority" to ::vendorIsoPriority,
            "mode_ae_on" to { runRepeatingPhase("mode_ae_on") { } },
            "mode_ae_off" to { runRepeatingPhase("mode_ae_off") { applyManualExposure(it, AE_OFF_EXPOSURE_NS, AE_OFF_ISO) } },
            "raw_sameness" to { runRepeatingPhase("raw_sameness") { applyManualExposure(it, AE_OFF_EXPOSURE_NS, SAMENESS_ISO) } },
            "raw_sameness_fixed_focus" to { runRepeatingPhase("raw_sameness_fixed_focus") {
                applyManualExposure(it, AE_OFF_EXPOSURE_NS, SAMENESS_ISO)
                it.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_OFF)
                it.set(CaptureRequest.LENS_FOCUS_DISTANCE, FIXED_FOCUS_DIOPTERS)
            } },
            "focus_sweep" to ::focusSweep,
            "age_test" to { runRepeatingPhase("age_test") {
                applyManualExposure(it, settings.ageTestExposureNs, AE_OFF_ISO)
                it.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_OFF)
                it.set(CaptureRequest.LENS_FOCUS_DISTANCE, FIXED_FOCUS_DIOPTERS)
            } },
            "soak" to { runRepeatingPhase("soak") { applyManualExposure(it, AE_OFF_EXPOSURE_NS, AE_OFF_ISO) } },
            "ae_off_af_continuous" to { runRepeatingPhase("ae_off_af_continuous") { applyManualExposure(it, AE_OFF_EXPOSURE_NS, AE_OFF_ISO) } },
            "ae_off_af_trigger" to { afTrigger("ae_off_af_trigger", centerRegion()) { applyManualExposure(it, AE_OFF_EXPOSURE_NS, AE_OFF_ISO) } },
            "ae_off_exposure_steps" to ::aeOffExposureSteps,
            "ae_off_control_mode_auto" to { runRepeatingPhase("ae_off_control_mode_auto") {
                applyManualExposure(it, AE_OFF_EXPOSURE_NS, AE_OFF_ISO)
                it.set(CaptureRequest.CONTROL_MODE, CaptureRequest.CONTROL_MODE_AUTO)
            } },
        )
        for ((name, body) in plan) {
            if (only == null || name in only) guarded(name, body)
        }
    }

    private fun guarded(name: String, body: () -> Unit) {
        log.event("phase_begin", JSONObject().put("phase", name))
        try {
            body()
        } catch (error: Exception) {
            log.error("phase_failed:$name", error)
        }
    }

    // --- Фазы -------------------------------------------------------------------------------

    private fun baseline() {
        runRepeatingPhase("baseline") { }
    }

    private fun afTrigger(phase: String, region: MeteringRectangle, baseConfig: (CaptureRequest.Builder) -> Unit = {}) {
        val regions = arrayOf(region)
        val afConfig: (CaptureRequest.Builder) -> Unit = { builder ->
            baseConfig(builder)
            builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_AUTO)
            builder.set(CaptureRequest.CONTROL_AF_REGIONS, regions)
            builder.set(CaptureRequest.CONTROL_AE_REGIONS, regions)
        }
        val submittedAfter = recorder.lastFrameNumber
        session.setRepeating(session.newRequest(ThreeATag(phase, ThreeATag.Kind.REPEATING), afConfig))
        // Сначала AF_MODE_AUTO без триггера: видно, куда встал фокус и какое у него состояние до START.
        hold(phase, settings.holdMs / 2)
        val triggerRequest = session.newRequest(ThreeATag(phase, ThreeATag.Kind.TRIGGER)) { builder ->
            afConfig(builder)
            builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CaptureRequest.CONTROL_AF_TRIGGER_START)
        }
        session.captureOnce(triggerRequest)
        hold(phase, settings.holdMs)
        val cancelRequest = session.newRequest(ThreeATag(phase, ThreeATag.Kind.CANCEL)) { builder ->
            afConfig(builder)
            builder.set(CaptureRequest.CONTROL_AF_TRIGGER, CaptureRequest.CONTROL_AF_TRIGGER_CANCEL)
        }
        session.captureOnce(cancelRequest)
        hold(phase, settings.holdMs / 2)
        finishPhase(phase, submittedAfter)
    }

    private fun evComp() {
        evPhase("ev_plus", compensationSteps = EV_PLUS_STEPS)
        evPhase("ev_minus", compensationSteps = EV_MINUS_STEPS)
    }

    private fun evPhase(phase: String, compensationSteps: Int) {
        runRepeatingPhase(phase) { it.set(CaptureRequest.CONTROL_AE_EXPOSURE_COMPENSATION, compensationSteps) }
    }

    private fun aeLock() {
        // Сначала даём AE сойтись при компенсации 0, иначе зафиксируем промежуточное значение.
        runRepeatingPhase("ae_lock_warmup") { }
        runRepeatingPhase("ae_lock") { it.set(CaptureRequest.CONTROL_AE_LOCK, true) }
    }

    private fun manual() {
        runRepeatingPhase("manual_1_1000", expectedExposureNs = MANUAL_EXPOSURE_NS) { builder ->
            applyManualExposure(builder, MANUAL_EXPOSURE_NS, MANUAL_ISO)
        }
    }

    private fun vendorIsoPriority() {
        runRepeatingPhase("vendor_control") { }
        runRepeatingPhase("vendor_a_abs_iso_800") {
            applyVendor(it, "select_priority:int=0", "use_iso_exp_priority:long=8", "use_iso_value:int=800")
        }
        runRepeatingPhase("vendor_b_mode5_iso800") {
            applyVendor(it, "select_priority:int=0", "use_iso_exp_priority:long=5")
        }
        runRepeatingPhase("vendor_c_exposure_1ms") {
            applyVendor(it, "select_priority:int=1", "use_iso_exp_priority:long=$VENDOR_EXPOSURE_PRIORITY_NS")
        }
        // Липкость: HAL держит вендорские теги, пока их не перезапишут. Ручной режим без вендорских ключей
        // сразу после приоритета выдержки показывает, переопределяет ли приоритет ручные значения.
        runRepeatingPhase("vendor_d_manual_after_priority_sticky") {
            applyManualExposure(it, STICKY_CHECK_EXPOSURE_NS, MANUAL_ISO)
        }
        runRepeatingPhase("vendor_e_manual_with_priority_reset") {
            applyManualExposure(it, STICKY_CHECK_EXPOSURE_NS, MANUAL_ISO)
            applyVendor(it, "select_priority:int=0", "use_iso_exp_priority:long=0")
        }
    }

    /** Ступени выдержки при ручной экспозиции и непрерывном AF: AF не должен сбрасываться, считаем задержку применения. */
    private fun aeOffExposureSteps() {
        val phase = "ae_off_exposure_steps"
        val steps = JSONArray()
        STEP_EXPOSURES_NS.forEachIndexed { index, exposureNs ->
            val submittedAfter = recorder.lastFrameNumber
            val tag = ThreeATag(phase, ThreeATag.Kind.REPEATING, round = index, requestedExposureNs = exposureNs)
            session.setRepeating(session.newRequest(tag) { applyManualExposure(it, exposureNs, STEP_ISO) })
            hold(phase, STEP_HOLD_MS)
            val stepRows = recorder.rowsOfPhase(phase).filter { it.tag.round == index }.sortedBy { it.frameNumber }
            val first = stepRows.firstOrNull()
            val settled = stepRows.drop(stepRows.size / 2)
            steps.put(
                JSONObject().put("step", index).put("requested_exposure_ns", exposureNs).put("results", stepRows.size)
                    .put("submitted_after_frame", submittedAfter)
                    .put("first_tagged_frame", first?.frameNumber ?: JSONObject.NULL)
                    .put("latency_frames_to_first_tagged", first?.let { it.frameNumber - submittedAfter } ?: JSONObject.NULL)
                    .put("first_tagged_exposure_ns", first?.exposureNs ?: JSONObject.NULL)
                    .put("settled_exposure_ns", settled.firstOrNull()?.exposureNs ?: JSONObject.NULL)
                    .put("af_states_seen", JSONArray(stepRows.map { it.afState }.distinct()))
                    .put("af_inactive_results", stepRows.count { it.afState == AF_STATE_INACTIVE })
                    .put("focus_distance_first_last", JSONArray(listOf(first?.focusDistance, stepRows.lastOrNull()?.focusDistance))),
            )
        }
        finishPhase(phase, -1)
        log.result("exposure_steps", JSONObject().put("steps", steps))
    }

    /** Свип фокуса при AF_MODE_OFF: на каждом шаге отдельная порция статистики RAW, чтобы отличить реальное движение линзы от эха запроса. */
    private fun focusSweep() {
        rawProbe?.snapshotAndReset() // сброс накопленного до свипа
        for (diopters in SWEEP_FOCUS_DIOPTERS) {
            val phase = "sweep_$diopters"
            val submittedAfter = recorder.lastFrameNumber
            session.setRepeating(session.newRequest(ThreeATag(phase, ThreeATag.Kind.REPEATING)) { builder ->
                applyManualExposure(builder, AE_OFF_EXPOSURE_NS, SAMENESS_ISO)
                builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_OFF)
                builder.set(CaptureRequest.LENS_FOCUS_DISTANCE, diopters)
            })
            // Кадры сразу после смены запроса пропускает сам зонд (content_skip), здесь только ждём.
            rawProbe?.snapshotAndReset()
            hold(phase, SWEEP_STEP_MS)
            val rows = recorder.rowsOfPhase(phase)
            log.result(
                "sweep_step",
                JSONObject().put("requested_diopters", diopters)
                    .put("reported_focus", JSONArray(rows.mapNotNull { it.focusDistance }.distinct().take(6)))
                    .put("lens_state", JSONArray(rows.map { it.json.opt("lens_state") }.distinct()))
                    .put("content", rawProbe?.snapshotAndReset() ?: JSONObject.NULL),
            )
            summaries += ThreeAPhaseSummarizer.summarize(phase, rows, submittedAfter)
        }
    }

    /** Пачка из 4 выдержек при работающем повторяющемся запросе (кейс продукта), 3 круга. */
    private fun burst() {
        val phase = "burst"
        val submittedAfter = recorder.lastFrameNumber
        session.setRepeating(session.newRequest(ThreeATag(phase, ThreeATag.Kind.REPEATING)) {
            applyManualExposure(it, BURST_REPEATING_EXPOSURE_NS, MANUAL_ISO)
        })
        hold(phase, settings.holdMs / 2)
        for (round in 1..BURST_ROUNDS) {
            val requests = BURST_EXPOSURES_NS.mapIndexed { index, exposureNs ->
                val tag = ThreeATag(phase, ThreeATag.Kind.BURST, round = round, index = index, requestedExposureNs = exposureNs)
                session.newRequest(tag) { applyManualExposure(it, exposureNs, MANUAL_ISO) }
            }
            session.captureBurst(requests)
            hold(phase, BURST_SETTLE_MS)
        }
        hold(phase, BURST_SETTLE_MS)
        finishPhase(phase, submittedAfter)
        log.result("burst_mapping", burstMapping())
    }

    // --- Общие кирпичики ---------------------------------------------------------------------

    private fun runRepeatingPhase(
        phase: String,
        expectedExposureNs: Long? = null,
        configure: (CaptureRequest.Builder) -> Unit,
    ) {
        val submittedAfter = recorder.lastFrameNumber
        session.setRepeating(session.newRequest(ThreeATag(phase, ThreeATag.Kind.REPEATING), configure))
        hold(phase, settings.holdMs)
        finishPhase(phase, submittedAfter, expectedExposureNs)
    }

    private fun finishPhase(phase: String, submittedAfter: Long, expectedExposureNs: Long? = null) {
        val summary = ThreeAPhaseSummarizer.summarize(phase, recorder.rowsOfPhase(phase), submittedAfter, expectedExposureNs)
        summaries += summary
        log.result("phase_summary", summary.json)
    }

    /** Держит фазу [durationMs]; если за это время и за запас не пришло ни одного кадра — пишет ошибку. */
    private fun hold(phase: String, durationMs: Long) {
        Thread.sleep(durationMs)
        if (recorder.rowsOfPhase(phase).isEmpty()) {
            Thread.sleep(settings.firstFrameGraceMs)
            if (recorder.rowsOfPhase(phase).isEmpty()) log.error("phase_no_frames:$phase", "за ${durationMs + settings.firstFrameGraceMs} мс нет результатов")
        }
    }

    private fun applyManualExposure(builder: CaptureRequest.Builder, exposureNs: Long, iso: Int) {
        builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_OFF)
        builder.set(CaptureRequest.SENSOR_EXPOSURE_TIME, exposureNs)
        builder.set(CaptureRequest.SENSOR_SENSITIVITY, iso)
        builder.set(CaptureRequest.SENSOR_FRAME_DURATION, settings.frameDurationNs)
    }

    private fun applyVendor(builder: CaptureRequest.Builder, vararg shortSpecs: String) {
        for (spec in shortSpecs) VendorKeyValue.parseOne("$VENDOR_ISO_SECTION.$spec").applyTo(builder)
    }

    private fun centerRegion(): MeteringRectangle {
        val width = settings.activeArray.width() / REGION_DIVISOR
        val height = settings.activeArray.height() / REGION_DIVISOR
        return MeteringRectangle(
            settings.activeArray.left + (settings.activeArray.width() - width) / 2,
            settings.activeArray.top + (settings.activeArray.height() - height) / 2,
            width, height, MeteringRectangle.METERING_WEIGHT_MAX,
        )
    }

    private fun topLeftRegion(): MeteringRectangle = MeteringRectangle(
        settings.activeArray.left, settings.activeArray.top,
        settings.activeArray.width() / REGION_DIVISOR, settings.activeArray.height() / REGION_DIVISOR,
        MeteringRectangle.METERING_WEIGHT_MAX,
    )

    /** Для каждого запроса пачки: какой кадр нёс метку, равна ли выдержка заказанной, пришёл ли RAW-буфер. */
    private fun burstMapping(): JSONObject {
        val entries = JSONArray()
        val burstRows = recorder.rowsOfPhase("burst").filter { it.tag.kind == ThreeATag.Kind.BURST }
        for (row in burstRows.sortedBy { it.frameNumber }) {
            entries.put(
                JSONObject().put("round", row.tag.round).put("index", row.tag.index)
                    .put("requested_exposure_ns", row.tag.requestedExposureNs)
                    .put("frame_number", row.frameNumber).put("sensor_timestamp", row.sensorTimestampNs ?: JSONObject.NULL)
                    .put("actual_exposure_ns", row.exposureNs ?: JSONObject.NULL)
                    .put("exposure_equal", row.exposureNs == row.tag.requestedExposureNs)
                    .put("raw_image_arrived", row.sensorTimestampNs?.let(recorder::hasImageWithTimestamp) ?: false),
            )
        }
        return JSONObject().put("expected_results", BURST_ROUNDS * BURST_EXPOSURES_NS.size)
            .put("tagged_results", burstRows.size).put("entries", entries)
    }

    private companion object {
        /** Шаг AE-компенсации 1/6 EV (из характеристик), поэтому ±2 EV = ±12 шагов. */
        const val EV_PLUS_STEPS = 12
        const val EV_MINUS_STEPS = -12
        const val MANUAL_EXPOSURE_NS = 1_000_000L
        const val MANUAL_ISO = 400
        const val VENDOR_EXPOSURE_PRIORITY_NS = 1_000_000L
        const val STICKY_CHECK_EXPOSURE_NS = 8_000_000L
        const val VENDOR_ISO_SECTION = "org.codeaurora.qcamera3.iso_exp_priority"
        const val REGION_DIVISOR = 8
        const val AE_OFF_EXPOSURE_NS = 8_000_000L
        const val AE_OFF_ISO = 800
        const val SAMENESS_ISO = 400
        val SWEEP_FOCUS_DIOPTERS = listOf(0.0f, 2.2f, 5.0f, 10.0f)
        const val SWEEP_STEP_MS = 1300L
        const val FIXED_FOCUS_DIOPTERS = 2.2f
        val STEP_EXPOSURES_NS = listOf(2_000_000L, 4_000_000L, 8_000_000L, 16_000_000L)
        const val STEP_ISO = 400
        const val STEP_HOLD_MS = 500L
        const val AF_STATE_INACTIVE = 0
        val BURST_EXPOSURES_NS = listOf(2_000_000L, 4_000_000L, 8_000_000L, 16_000_000L)
        const val BURST_REPEATING_EXPOSURE_NS = 8_000_000L
        const val BURST_ROUNDS = 3
        const val BURST_SETTLE_MS = 700L
    }
}
