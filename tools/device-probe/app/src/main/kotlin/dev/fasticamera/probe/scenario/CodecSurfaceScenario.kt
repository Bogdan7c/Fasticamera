package dev.fasticamera.probe.scenario

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import dev.fasticamera.probe.NativeBridge
import dev.fasticamera.probe.NativeJson
import dev.fasticamera.probe.camera.FrameTimingStats
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * C8/V5: рисует ли Vulkan прямо во входную surface HEVC-кодера (как в swapchain), без копий.
 *
 * Kotlin создаёт кодер и вычитывает его выход в отдельном потоке, нативная сторона
 * создаёт swapchain на `createInputSurface()` и рисует кадры. Итог — сколько кадров
 * реально закодировано и ровны ли их метки времени (PTS).
 */
class CodecSurfaceScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val args = context.args
        val codecName = args.string("codec") ?: DEFAULT_CODEC
        val width = args.int("width", DEFAULT_WIDTH)
        val height = args.int("height", DEFAULT_HEIGHT)
        val fps = args.int("fps", DEFAULT_FPS)
        val frames = args.int("frames", DEFAULT_FRAMES)
        val tenBit = args.string("ten_bit") != "0"
        val format = encoderFormat(width, height, fps, tenBit, args.int("bitrate", DEFAULT_BITRATE))
        context.log.event("encoder_format", JSONObject().put("codec", codecName).put("format", format.toString()))

        val codec = MediaCodec.createByCodecName(codecName)
        val drainExecutor = Executors.newSingleThreadExecutor()
        try {
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            val surface = codec.createInputSurface()
            codec.start()
            val drain = drainExecutor.submit<JSONObject> { drainEncoder(codec) }
            val renderJson = NativeJson.call {
                NativeBridge.codecSurfaceRenderTest(surface, width, height, frames, NANOS_PER_SECOND / fps, tenBit)
            }
            context.log.result("vulkan_render", renderJson)
            // Конец потока: кодер выдаст EOS, поток вычитки завершится.
            codec.signalEndOfInputStream()
            val encoded = drain.get(DRAIN_TIMEOUT_S, TimeUnit.SECONDS)
            context.log.result("encoder_output", encoded.put("output_format", codec.outputFormat.toString()))
            surface.release()
        } finally {
            drainExecutor.shutdownNow()
            codec.stop()
            codec.release()
        }
    }

    private fun encoderFormat(width: Int, height: Int, fps: Int, tenBit: Boolean, bitrate: Int): MediaFormat =
        MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_HEVC, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            if (tenBit) {
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10)
                setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT2020)
                setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_HLG)
                setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED)
            }
        }

    /** Вычитывает выход кодера до EOS: число кадров, размеры и метки времени. */
    private fun drainEncoder(codec: MediaCodec): JSONObject {
        val info = MediaCodec.BufferInfo()
        val presentationTimesNs = ArrayList<Long>()
        var totalBytes = 0L
        var configFrames = 0
        var keyFrames = 0
        val startedNs = System.nanoTime()
        while (System.nanoTime() - startedNs < TimeUnit.SECONDS.toNanos(DRAIN_TIMEOUT_S)) {
            val index = codec.dequeueOutputBuffer(info, DEQUEUE_TIMEOUT_US)
            if (index < 0) continue
            val isConfig = info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0
            if (isConfig) {
                configFrames += 1
            } else if (info.size > 0) {
                presentationTimesNs += info.presentationTimeUs * NANOS_PER_MICRO
                totalBytes += info.size
                if (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0) keyFrames += 1
            }
            codec.releaseOutputBuffer(index, false)
            if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                return encodedSummary(presentationTimesNs, totalBytes, configFrames, keyFrames, reachedEos = true)
            }
        }
        return encodedSummary(presentationTimesNs, totalBytes, configFrames, keyFrames, reachedEos = false)
    }

    private fun encodedSummary(pts: List<Long>, bytes: Long, configFrames: Int, keyFrames: Int, reachedEos: Boolean) =
        JSONObject()
            .put("encoded_frames", pts.size)
            .put("encoded_bytes", bytes)
            .put("config_frames", configFrames)
            .put("key_frames", keyFrames)
            .put("reached_eos", reachedEos)
            .put("pts_timing", FrameTimingStats.compute(pts).toJson())

    private companion object {
        const val DEFAULT_CODEC = "c2.qti.hevc.encoder.hdr"
        const val DEFAULT_WIDTH = 3840
        const val DEFAULT_HEIGHT = 2160
        const val DEFAULT_FPS = 60
        const val DEFAULT_FRAMES = 300
        const val DEFAULT_BITRATE = 100_000_000
        const val NANOS_PER_SECOND = 1_000_000_000L
        const val NANOS_PER_MICRO = 1_000L
        const val DEQUEUE_TIMEOUT_US = 10_000L
        const val DRAIN_TIMEOUT_S = 30L
    }
}
