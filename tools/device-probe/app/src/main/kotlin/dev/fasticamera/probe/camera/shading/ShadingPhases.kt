package dev.fasticamera.probe.camera.shading

import kotlin.math.roundToLong

/** Фазы замера: каждая меняет ровно одну вещь относительно `base` (см. README, сценарий shading). */
enum class ShadingPhaseId(val wireName: String) {
    BASE("base"),
    MAP_OFF("map_off"),
    ISO_HIGH("iso_high"),
    AE_ON("ae_on"),
    FOCUS_INF("focus_inf"),
    FOCUS_NEAR("focus_near"),
    ;

    val needsManualFocus: Boolean get() = this == FOCUS_INF || this == FOCUS_NEAR

    companion object {
        fun fromWire(raw: String): ShadingPhaseId = entries.firstOrNull { it.wireName == raw }
            ?: throw IllegalArgumentException("неизвестная фаза '$raw' (допустимы ${entries.joinToString("|") { it.wireName }})")
    }
}

/** Как экспозицией управляет запрос: вендорский AE или ручные значения. */
sealed interface ExposureControl {
    data object VendorAuto : ExposureControl
    data class Manual(val setting: ExposureSetting) : ExposureControl
}

/** Режим фокуса запроса. */
enum class FocusControl { CONTINUOUS_PICTURE, INFINITY, NEAREST }

/** Всё, что отличает один запрос от другого в рамках режима камеры. */
data class PhaseControls(
    val exposure: ExposureControl,
    val lensShadingMapRequested: Boolean,
    val focus: FocusControl,
)

/** Что камера умеет по фокусу: от этого зависит, можно ли снимать фазы с ручным фокусом. */
data class FocusCapabilities(val minFocusDistanceDiopters: Float?, val afModes: IntArray?) {
    /** Причина, по которой ручной фокус недоступен, либо null, если доступен. */
    fun manualFocusUnavailableReason(): String? = when {
        minFocusDistanceDiopters == null || minFocusDistanceDiopters <= 0f ->
            "LENS_INFO_MINIMUM_FOCUS_DISTANCE=${minFocusDistanceDiopters ?: "нет"}: фокус фиксированный"
        afModes == null || afModes.all { it == AF_MODE_OFF_VALUE } ->
            "CONTROL_AF_AVAILABLE_MODES содержит только OFF"
        else -> null
    }

    private companion object {
        /** Значение CONTROL_AF_MODE_OFF (0) — константа Camera2, дублируется, чтобы файл оставался чистым JVM-кодом. */
        const val AF_MODE_OFF_VALUE = 0
    }
}

/** Планирование фаз: чистые функции без обращения к камере. */
object PhasePlanner {
    /** Во сколько раз фаза iso_high поднимает ISO (выдержка падает во столько же). */
    const val ISO_HIGH_FACTOR = 4

    fun controlsFor(phase: ShadingPhaseId, base: ExposureSetting, limits: SensorLimits): PhaseControls = when (phase) {
        ShadingPhaseId.BASE -> manual(base, mapRequested = true, FocusControl.CONTINUOUS_PICTURE)
        ShadingPhaseId.MAP_OFF -> manual(base, mapRequested = false, FocusControl.CONTINUOUS_PICTURE)
        ShadingPhaseId.ISO_HIGH -> manual(isoHigh(base, limits), mapRequested = true, FocusControl.CONTINUOUS_PICTURE)
        ShadingPhaseId.AE_ON -> PhaseControls(ExposureControl.VendorAuto, lensShadingMapRequested = true, FocusControl.CONTINUOUS_PICTURE)
        ShadingPhaseId.FOCUS_INF -> manual(base, mapRequested = true, FocusControl.INFINITY)
        ShadingPhaseId.FOCUS_NEAR -> manual(base, mapRequested = true, FocusControl.NEAREST)
    }

    /**
     * ISO x4, выдержка делится на фактически достигнутый множитель: если ISO упёрся в максимум,
     * яркость сохраняется за счёт меньшей выдержки, а реальный множитель виден в записанных значениях.
     */
    fun isoHigh(base: ExposureSetting, limits: SensorLimits): ExposureSetting {
        val iso = (base.iso.toLong() * ISO_HIGH_FACTOR).coerceAtMost(limits.maxIso.toLong()).toInt()
        val appliedFactor = iso.toDouble() / base.iso
        val exposureNs = (base.exposureNs / appliedFactor).roundToLong().coerceAtLeast(limits.minExposureNs)
        return ExposureSetting(exposureNs, iso)
    }

    private fun manual(setting: ExposureSetting, mapRequested: Boolean, focus: FocusControl) =
        PhaseControls(ExposureControl.Manual(setting), mapRequested, focus)

    /** Причина пропуска фазы или null, если фазу снимать можно. */
    fun skipReason(phase: ShadingPhaseId, focus: FocusCapabilities): String? =
        if (phase.needsManualFocus) focus.manualFocusUnavailableReason() else null
}
