package dev.fasticamera.probe.camera.dual

import dev.fasticamera.probe.camera.ManualExposure
import dev.fasticamera.probe.camera.VendorKeyValue

/**
 * Как именно камера ведётся в эксперименте: fps-диапазон, ручная экспозиция, автофокус,
 * вендорские ключи CamX и «компаньон» (YUV-выход сессии, который не является целью запроса).
 * [plain] — прежнее поведение первых экспериментов (только AE fps).
 */
data class RigProfile(
    val aeTargetFps: Int?,
    val manualExposure: ManualExposure?,
    val afContinuous: Boolean,
    val vendorKeys: List<VendorKeyValue>,
    val companionYuv: LegSpec?,
    /** Возвращать кадры сразу и не считать их («в никуда»). */
    val discardFrames: Boolean,
) {
    val hasSessionParameters: Boolean get() = aeTargetFps != null || vendorKeys.isNotEmpty() || manualExposure != null

    companion object {
        fun plain(aeTargetFps: Int?) = RigProfile(aeTargetFps, null, false, emptyList(), null, false)
    }
}
