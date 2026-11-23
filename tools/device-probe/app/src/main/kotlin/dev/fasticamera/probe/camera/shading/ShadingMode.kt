package dev.fasticamera.probe.camera.shading

import dev.fasticamera.probe.camera.VendorKeyValue
import dev.fasticamera.probe.camera.dual.LegSpec

/**
 * Режим камеры, как его настраивает ПРОДУКТ: какая камера, буфер RAW10, fps-диапазон, вендорские ключи.
 * Значения взяты из решений продукта (см. ProductDualExperiment); менять их здесь — значит мерить не то, что в продукте.
 */
enum class ShadingMode(
    val wireName: String,
    val cameraId: String,
    val bufferWidth: Int,
    val bufferHeight: Int,
    /** AE_TARGET_FPS_RANGE [fps, fps] в сессии и запросах; null — не задавать (на фронтальной [60,60] валит провайдер камеры). */
    val aeTargetFps: Int?,
    /** Вендорские ключи CamX в формате VendorKeyValue.parseList; null — без ключей. */
    val vendorKeysSpec: String?,
    /** Длительность кадра режима: потолок выдержки и значение SENSOR_FRAME_DURATION в ручных запросах. */
    val frameDurationNs: Long,
) {
    MAIN_4_3(
        "main43", "2", 4096, 3072, aeTargetFps = null,
        vendorKeysSpec = "org.quic.camera.manualExposure.disableFPSLimits:byte=1;" +
            "org.quic.camera.manualExposure.overrideSensorFrameDuration:long=$FRAME_DURATION_60_FPS_NS",
        frameDurationNs = FRAME_DURATION_60_FPS_NS,
    ),

    /** Буфер 4:3, но HAL по fps [60,60] берёт режим 4096x2304: данные в верхних строках, ниже нули. */
    MAIN_16_9("main169", "2", 4096, 3072, aeTargetFps = 60, vendorKeysSpec = null, frameDurationNs = FRAME_DURATION_60_FPS_NS),

    /** Ключи CamX на ширике ломают камеру (0 кадров) — не задаём. */
    WIDE_4_3("wide43", "3", 4000, 3000, aeTargetFps = 60, vendorKeysSpec = null, frameDurationNs = FRAME_DURATION_60_FPS_NS),

    /** Фронтальная: ни fps-диапазона, ни ключей CamX (AE [60,60] роняет провайдер камеры). */
    FRONT_30("front30", "1", 2304, 1728, aeTargetFps = null, vendorKeysSpec = null, frameDurationNs = FRAME_DURATION_30_FPS_NS),
    ;

    /** Выдержка не может быть равна длительности кадра: оставляем запас на считывание. */
    val exposureCapNs: Long get() = frameDurationNs - EXPOSURE_READOUT_MARGIN_NS

    fun vendorKeys(): List<VendorKeyValue> = VendorKeyValue.parseList(vendorKeysSpec)

    /** Основной RAW10-поток режима. */
    fun rawLeg(): LegSpec = LegSpec.parse("$cameraId:raw10:${bufferWidth}x$bufferHeight")

    /** Пустой YUV-компаньон: объявлен в сессии, но никогда не цель запроса (как в продукте оживляет 3A). */
    fun companionLeg(): LegSpec = LegSpec.parse("$cameraId:yuv:640x480")

    companion object {
        fun fromWire(raw: String): ShadingMode = entries.firstOrNull { it.wireName == raw }
            ?: throw IllegalArgumentException("неизвестный mode '$raw' (допустимы ${entries.joinToString("|") { it.wireName }})")
    }
}

private const val FRAME_DURATION_60_FPS_NS = 16_666_667L
private const val FRAME_DURATION_30_FPS_NS = 33_333_333L

/** Запас между выдержкой и длительностью кадра на считывание строк: 1 мс. */
private const val EXPOSURE_READOUT_MARGIN_NS = 1_000_000L
