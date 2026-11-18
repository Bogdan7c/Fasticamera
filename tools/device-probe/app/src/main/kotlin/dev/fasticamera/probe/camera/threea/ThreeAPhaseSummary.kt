package dev.fasticamera.probe.camera.threea

import dev.fasticamera.probe.camera.FrameTimingStats
import org.json.JSONArray
import org.json.JSONObject

/** Сводка одной фазы: JSON для отчёта + медиана exposure×ISO для сравнения фаз между собой. */
data class ThreeAPhaseSummary(val phase: String, val json: JSONObject, val medianExposureTimesIso: Double?)

/** Чистые вычисления над рядами кадров фазы: ничего не знает про Camera2 и ввод-вывод. */
object ThreeAPhaseSummarizer {

    private val AF_STATE_NAMES = mapOf(
        0 to "INACTIVE", 1 to "PASSIVE_SCAN", 2 to "PASSIVE_FOCUSED", 3 to "ACTIVE_SCAN",
        4 to "FOCUSED_LOCKED", 5 to "NOT_FOCUSED_LOCKED", 6 to "PASSIVE_UNFOCUSED",
    )
    private val AE_STATE_NAMES = mapOf(
        0 to "INACTIVE", 1 to "SEARCHING", 2 to "CONVERGED", 3 to "LOCKED", 4 to "FLASH_REQUIRED", 5 to "PRECAPTURE",
    )

    /** Интервал длиннее этого ломает 60 к/с (16,67 мс + запас на джиттер метки). */
    private const val BROKEN_60FPS_INTERVAL_NS = 20_000_000L

    /**
     * @param submittedAfterFrameNumber номер последнего кадра, пришедшего до смены запроса фазы
     * @param expectedExposureNs выдержка, которую ждём увидеть в результате (для замера задержки применения)
     */
    fun summarize(
        phase: String,
        rows: List<ThreeAFrameRow>,
        submittedAfterFrameNumber: Long,
        expectedExposureNs: Long? = null,
    ): ThreeAPhaseSummary {
        val ordered = rows.sortedBy { it.frameNumber }
        val json = JSONObject().put("phase", phase).put("results", ordered.size)
            .put("submitted_after_frame", submittedAfterFrameNumber)
        if (ordered.isEmpty()) return ThreeAPhaseSummary(phase, json.put("error", "в фазе нет ни одного результата"), null)

        val timestamps = ordered.mapNotNull { it.sensorTimestampNs }.sorted()
        json.put("timing", FrameTimingStats.compute(timestamps).toJson())
        json.put("intervals_over_20ms", timestamps.zipWithNext { a, b -> b - a }.count { it > BROKEN_60FPS_INTERVAL_NS })
        json.put("frame_number_range", JSONArray(listOf(ordered.first().frameNumber, ordered.last().frameNumber)))
        json.put("first_frame_with_phase_tag", ordered.first().frameNumber)
        json.put("frames_from_submit_to_first_tagged", ordered.first().frameNumber - submittedAfterFrameNumber)

        // Медианы берём по второй половине: первая — переходный процесс AE.
        val settled = ordered.drop(ordered.size / 2)
        val exposureTimesIso = settled.mapNotNull { row -> row.exposureNs?.let { e -> row.iso?.let { i -> e.toDouble() * i } } }
        val medianExposureIso = median(exposureTimesIso)
        json.put("settled_median_exposure_ns", median(settled.mapNotNull { it.exposureNs?.toDouble() }) ?: JSONObject.NULL)
        json.put("settled_median_iso", median(settled.mapNotNull { it.iso?.toDouble() }) ?: JSONObject.NULL)
        json.put("settled_median_exposure_x_iso", medianExposureIso ?: JSONObject.NULL)
        json.put("distinct_exposure_ns", distinctCounts(ordered.map { it.exposureNs }))
        json.put("distinct_iso", distinctCounts(ordered.map { it.iso }))
        json.put("distinct_ae_state", distinctCounts(ordered.map { it.aeState?.let { s -> AE_STATE_NAMES[s] ?: "?$s" } }))
        json.put("distinct_ae_lock", distinctCounts(ordered.map { it.aeLock }))
        json.put("af_transitions", afTransitions(ordered))
        val focus = ordered.mapNotNull { it.focusDistance }
        json.put(
            "focus_distance",
            if (focus.isEmpty()) JSONObject.NULL
            else JSONObject().put("first", focus.first()).put("last", focus.last()).put("min", focus.min()).put("max", focus.max()),
        )
        json.put("trigger_to_locked_frames", framesTriggerToLocked(ordered) ?: JSONObject.NULL)
        if (expectedExposureNs != null) {
            val firstReached = ordered.firstOrNull { it.exposureNs == expectedExposureNs }
            json.put("expected_exposure_ns", expectedExposureNs)
            json.put("first_frame_with_expected_exposure", firstReached?.frameNumber ?: JSONObject.NULL)
            json.put(
                "frames_from_submit_to_expected_exposure",
                firstReached?.let { it.frameNumber - submittedAfterFrameNumber } ?: JSONObject.NULL,
            )
        }
        return ThreeAPhaseSummary(phase, json, medianExposureIso)
    }

    /** Список смен AF_STATE: `[кадр, вид запроса, состояние]` — по нему видно ACTIVE_SCAN → LOCKED. */
    private fun afTransitions(ordered: List<ThreeAFrameRow>): JSONArray {
        val transitions = JSONArray()
        var previous: Int? = -1
        for (row in ordered) {
            val isSpecialRequest = row.tag.kind == ThreeATag.Kind.TRIGGER || row.tag.kind == ThreeATag.Kind.CANCEL
            if (row.afState != previous || isSpecialRequest) {
                transitions.put(JSONArray(listOf(row.frameNumber, row.tag.kind.name, row.afState?.let { AF_STATE_NAMES[it] ?: "?$it" })))
            }
            previous = row.afState
        }
        return transitions
    }

    /** Сколько кадров от результата TRIGGER-запроса до первого *_LOCKED состояния AF; null — триггера/лока нет. */
    private fun framesTriggerToLocked(ordered: List<ThreeAFrameRow>): Long? {
        val trigger = ordered.firstOrNull { it.tag.kind == ThreeATag.Kind.TRIGGER } ?: return null
        val locked = ordered.firstOrNull { it.frameNumber > trigger.frameNumber && (it.afState == 4 || it.afState == 5) } ?: return null
        return locked.frameNumber - trigger.frameNumber
    }

    private fun <T> distinctCounts(values: List<T?>): JSONObject {
        val counts = LinkedHashMap<String, Int>()
        values.forEach { counts.merge(it?.toString() ?: "null", 1, Int::plus) }
        val top = counts.entries.sortedByDescending { it.value }.take(MAX_DISTINCT)
        val json = JSONObject()
        top.forEach { json.put(it.key, it.value) }
        return json.put("_distinct_total", counts.size)
    }

    private fun median(values: List<Double>): Double? =
        values.sorted().let { if (it.isEmpty()) null else it[it.size / 2] }

    private const val MAX_DISTINCT = 12
}
