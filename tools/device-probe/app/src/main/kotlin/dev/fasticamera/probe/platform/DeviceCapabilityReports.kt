package dev.fasticamera.probe.platform

import android.content.Context
import android.hardware.display.DisplayManager
import android.media.AudioManager
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.VibrationEffect
import android.os.VibratorManager
import android.view.Display
import org.json.JSONArray
import org.json.JSONObject

/**
 * Справочные возможности платформы (C6, C8): только чтение API, без побочных эффектов.
 */
object DeviceCapabilityReports {

    /** Примитивы хаптики, нужные для iOS-подобных тиков колеса зума и затвора. */
    private val HAPTIC_PRIMITIVES = mapOf(
        VibrationEffect.Composition.PRIMITIVE_TICK to "TICK",
        VibrationEffect.Composition.PRIMITIVE_LOW_TICK to "LOW_TICK",
        VibrationEffect.Composition.PRIMITIVE_CLICK to "CLICK",
        VibrationEffect.Composition.PRIMITIVE_THUD to "THUD",
    )

    fun haptics(context: Context): JSONObject {
        val vibrator = context.getSystemService(VibratorManager::class.java).defaultVibrator
        val primitives = HAPTIC_PRIMITIVES.keys.toIntArray()
        val supported = vibrator.arePrimitivesSupported(*primitives)
        val durations = vibrator.getPrimitiveDurations(*primitives)
        val perPrimitive = JSONObject()
        HAPTIC_PRIMITIVES.values.forEachIndexed { index, name ->
            perPrimitive.put(name, JSONObject().put("supported", supported[index]).put("duration_ms", durations[index]))
        }
        return JSONObject()
            .put("has_vibrator", vibrator.hasVibrator())
            .put("has_amplitude_control", vibrator.hasAmplitudeControl())
            .put("primitives", perPrimitive)
            .put("all_supported", supported.all { it })
    }

    fun display(context: Context): JSONObject {
        val display = context.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val modes = JSONArray(
            display.supportedModes.map { mode ->
                JSONObject().put("id", mode.modeId).put("width", mode.physicalWidth).put("height", mode.physicalHeight)
                    .put("refresh_hz", mode.refreshRate.toDouble())
            },
        )
        return JSONObject()
            .put("current_refresh_hz", display.refreshRate.toDouble())
            .put("current_mode_id", display.mode.modeId)
            .put("modes", modes)
            .put("hdr_types", JSONArray(display.hdrCapabilities?.supportedHdrTypes?.toList() ?: emptyList<Int>()))
    }

    fun microphones(context: Context): JSONObject {
        val audio = context.getSystemService(AudioManager::class.java)
        val microphones = audio.microphones.map { mic ->
            JSONObject()
                .put("description", mic.description)
                .put("id", mic.id)
                .put("type", mic.type)
                .put("location", mic.location)
                .put("position", mic.position?.let { "${it.x},${it.y},${it.z}" } ?: JSONObject.NULL)
                .put("directionality", mic.directionality)
                .put("channel_mapping_count", mic.channelMapping.size)
        }
        return JSONObject().put("count", microphones.size).put("microphones", JSONArray(microphones))
    }

    /**
     * Кодеры HEVC: поддерживают ли 4K60 и Main10 (C8, задел V5).
     * Факт «принимает ли кодер Vulkan-поверхность» здесь не проверяется — только объявленные возможности.
     */
    fun videoEncoders(): JSONArray {
        val result = JSONArray()
        val codecs = MediaCodecList(MediaCodecList.ALL_CODECS).codecInfos.filter { it.isEncoder }
        for (codec in codecs) {
            for (type in codec.supportedTypes.filter { it == MediaFormat.MIMETYPE_VIDEO_HEVC || it == MediaFormat.MIMETYPE_VIDEO_AVC }) {
                val capabilities = codec.getCapabilitiesForType(type)
                val video = capabilities.videoCapabilities ?: continue
                val profiles = capabilities.profileLevels.map { it.profile }.distinct()
                result.put(
                    JSONObject()
                        .put("name", codec.name)
                        .put("type", type)
                        .put("hardware", codec.isHardwareAccelerated)
                        .put("uhd_60_supported", video.areSizeAndRateSupported(UHD_WIDTH, UHD_HEIGHT, FPS_60))
                        .put("dci4k_60_supported", video.areSizeAndRateSupported(DCI4K_WIDTH, UHD_HEIGHT, FPS_60))
                        .put("uhd_achievable_fps", achievableFps(video, UHD_WIDTH, UHD_HEIGHT))
                        .put("bitrate_range", video.bitrateRange.toString())
                        .put("hevc_main10", profiles.contains(MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10))
                        .put("hevc_main10_hdr10", profiles.contains(MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10))
                        .put("surface_input", capabilities.colorFormats.contains(MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)),
                )
            }
        }
        return result
    }

    /**
     * Измеренные производителем к/с для размера. Кодек, не умеющий этот размер, бросает
     * IllegalArgumentException — это ответ «размер не поддержан», а не сбой всего блока.
     */
    private fun achievableFps(video: MediaCodecInfo.VideoCapabilities, width: Int, height: Int): Any = try {
        video.getAchievableFrameRatesFor(width, height)?.toString() ?: JSONObject.NULL
    } catch (_: IllegalArgumentException) {
        "unsupported_size"
    }

    private const val UHD_WIDTH = 3840
    private const val DCI4K_WIDTH = 4096
    private const val UHD_HEIGHT = 2160
    private const val FPS_60 = 60.0
}
