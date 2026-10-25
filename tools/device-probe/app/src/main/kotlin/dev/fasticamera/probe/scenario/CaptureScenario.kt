package dev.fasticamera.probe.scenario

import android.graphics.ImageFormat
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.DngCreator
import android.hardware.camera2.TotalCaptureResult
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.Image
import android.media.ImageReader
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.RawPassport
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * C7: эталонный набор одной сцены для сессии 06 —
 * DNG через платформенный DngCreator, вендорский JPEG того же снимка,
 * сырой кадр RAW16 из потока и паспорт метаданных (JSON).
 *
 * Файлы пишутся в filesDir/captures/<run>/; ПК забирает их `pc/probe.py pull-captures`
 * в каталог вне репозитория. В отчёт идут только размеры и SHA-256.
 */
class CaptureScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val cameraId = context.args.requireString("camera")
        val sceneLabel = context.args.string("scene") ?: "scene"
        val characteristics = context.cameraManager.getCameraCharacteristics(cameraId)
        val streamMap = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
            ?: throw IllegalStateException("у камеры $cameraId нет карты потоков")
        // Самые большие публичные размеры: RAW — нативный биннинговый кадр, JPEG — максимальный.
        val rawSize = streamMap.getOutputSizes(ImageFormat.RAW_SENSOR).maxBy { it.width * it.height }
        val jpegSize = streamMap.getOutputSizes(ImageFormat.JPEG).maxBy { it.width * it.height }
        val outputDir = File(context.appContext.filesDir, "captures/${context.runId}").apply { mkdirs() }

        val rawReader = ImageReader.newInstance(rawSize.width, rawSize.height, ImageFormat.RAW_SENSOR, READER_DEPTH)
        val jpegReader = ImageReader.newInstance(jpegSize.width, jpegSize.height, ImageFormat.JPEG, READER_DEPTH)
        val rawImages = LinkedBlockingQueue<Image>()
        val jpegImages = LinkedBlockingQueue<Image>()
        // Поток RAW идёт ~30 к/с, а maxImages ограничен: держим только последние кадры, старые закрываем,
        // иначе ридер исчерпается и acquireNextImage бросит исключение посреди сходимости AE.
        rawReader.setOnImageAvailableListener({ reader ->
            reader.acquireNextImage()?.let { image ->
                rawImages.put(image)
                while (rawImages.size > RAW_IMAGES_KEPT) rawImages.poll()?.close()
            }
        }, context.imageHandler)
        jpegReader.setOnImageAvailableListener({ reader -> reader.acquireNextImage()?.let(jpegImages::put) }, context.imageHandler)

        val runner = context.newSessionRunner()
        val opened = runner.open(cameraId, OPEN_TIMEOUT_MS)
        if (opened !is CameraSessionRunner.OpenOutcome.Opened) {
            context.log.error("open", opened.toJson().toString())
            return
        }
        val device = opened.device
        try {
            val outputs = listOf(OutputConfiguration(rawReader.surface), OutputConfiguration(jpegReader.surface))
            val session = runner.createSession(device, outputs, SessionConfiguration.SESSION_REGULAR, null, OPEN_TIMEOUT_MS)
            if (session !is CameraSessionRunner.SessionOutcome.Configured) {
                context.log.error("session", session.toJson().toString())
                return
            }
            val files = shoot(context, device, session.session, rawReader, jpegReader, rawImages, jpegImages, characteristics)
                ?: return
            val written = writeSet(outputDir, "${cameraId}_$sceneLabel", files, characteristics)
            context.log.result("capture_set", written.put("camera", cameraId).put("scene", sceneLabel)
                .put("raw_size", rawSize.toString()).put("jpeg_size", jpegSize.toString()))
            session.session.close()
        } finally {
            device.close()
            rawReader.close()
            jpegReader.close()
        }
    }

    /** Кадры одного снимка: RAW и JPEG одного запроса + последний кадр RAW из потока + результаты. */
    private class ShotFrames(
        val stillRaw: Image,
        val stillJpeg: Image,
        val stillResult: TotalCaptureResult,
        val streamRaw: Image,
        val streamResult: TotalCaptureResult,
    )

    private fun shoot(
        context: ScenarioContext,
        device: CameraDevice,
        session: CameraCaptureSession,
        rawReader: ImageReader,
        jpegReader: ImageReader,
        rawImages: LinkedBlockingQueue<Image>,
        jpegImages: LinkedBlockingQueue<Image>,
        characteristics: CameraCharacteristics,
    ): ShotFrames? {
        // Поток RAW, пока сходятся AE/AWB/AF — как видоискатель нашего будущего приложения.
        val streamResults = LinkedBlockingQueue<TotalCaptureResult>()
        val stream = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
            addTarget(rawReader.surface)
            RawPassport.applyCleanRawControls(this)
        }.build()
        session.setRepeatingRequest(stream, resultCollector(streamResults), context.imageHandler)
        Thread.sleep(CONVERGENCE_MS)
        session.stopRepeating()
        Thread.sleep(SETTLE_MS)
        // Последний кадр потока с совпадающим результатом — «кадр нашего RAW-потока».
        val streamPair = latestMatching(rawImages, streamResults) ?: run {
            context.log.error("stream_frame", "не нашлось кадра потока с результатом той же метки времени")
            return null
        }
        val still = device.createCaptureRequest(CameraDevice.TEMPLATE_STILL_CAPTURE).apply {
            addTarget(rawReader.surface)
            addTarget(jpegReader.surface)
            RawPassport.applyCleanRawControls(this)
            set(CaptureRequest.JPEG_QUALITY, JPEG_QUALITY)
        }.build()
        val stillResults = LinkedBlockingQueue<TotalCaptureResult>()
        session.capture(still, resultCollector(stillResults), context.imageHandler)
        val stillResult = stillResults.poll(STILL_TIMEOUT_S, TimeUnit.SECONDS)
        val stillRaw = rawImages.poll(STILL_TIMEOUT_S, TimeUnit.SECONDS)
        val stillJpeg = jpegImages.poll(STILL_TIMEOUT_S, TimeUnit.SECONDS)
        if (stillResult == null || stillRaw == null || stillJpeg == null) {
            context.log.error("still", "снимок неполный: result=${stillResult != null} raw=${stillRaw != null} jpeg=${stillJpeg != null}")
            return null
        }
        context.log.event("still_captured", JSONObject().put("characteristics_raw_profiles", characteristics.keys.size))
        return ShotFrames(stillRaw, stillJpeg, stillResult, streamPair.first, streamPair.second)
    }

    private fun resultCollector(target: LinkedBlockingQueue<TotalCaptureResult>) = object : CameraCaptureSession.CaptureCallback() {
        override fun onCaptureCompleted(session: CameraCaptureSession, request: CaptureRequest, result: TotalCaptureResult) {
            target.put(result)
        }
    }

    /** Самая свежая пара «картинка + результат» с одинаковой меткой сенсора; остальные картинки закрываются. */
    private fun latestMatching(
        images: LinkedBlockingQueue<Image>,
        results: LinkedBlockingQueue<TotalCaptureResult>,
    ): Pair<Image, TotalCaptureResult>? {
        val resultByTimestamp = results.toList().associateBy { it.get(android.hardware.camera2.CaptureResult.SENSOR_TIMESTAMP) }
        val drained = generateSequence { images.poll() }.toList()
        val match = drained.lastOrNull { resultByTimestamp.containsKey(it.timestamp) }
        drained.filter { it !== match }.forEach(Image::close)
        return match?.let { it to resultByTimestamp.getValue(it.timestamp) }
    }

    private fun writeSet(dir: File, baseName: String, frames: ShotFrames, characteristics: CameraCharacteristics): JSONObject {
        val report = JSONObject()
        try {
            report.put("dng", writeFile(File(dir, "$baseName.dng")) { stream ->
                DngCreator(characteristics, frames.stillResult).use { it.writeImage(stream, frames.stillRaw) }
            })
            report.put("jpeg", writeFile(File(dir, "$baseName.vendor.jpg")) { stream ->
                val buffer = frames.stillJpeg.planes[0].buffer
                stream.write(ByteArray(buffer.remaining()).also(buffer::get))
            })
            report.put("stream_raw16", writeFile(File(dir, "$baseName.stream.raw16")) { stream ->
                writeRaw16Rows(frames.streamRaw, stream)
            })
            val passport = JSONObject()
                .put("stream_frame", RawPassport.build(characteristics, frames.streamResult)
                    .put("width", frames.streamRaw.width).put("height", frames.streamRaw.height)
                    .put("layout", "RAW16 little-endian, строки без паддинга"))
                .put("still_frame", RawPassport.build(characteristics, frames.stillResult))
            report.put("passport", writeFile(File(dir, "$baseName.passport.json")) { stream ->
                stream.write(passport.toString(2).toByteArray())
            })
        } finally {
            frames.stillRaw.close()
            frames.stillJpeg.close()
            frames.streamRaw.close()
        }
        return report
    }

    /** RAW16 без паддинга строк: файл читается как плотный массив width×height u16. */
    private fun writeRaw16Rows(image: Image, stream: FileOutputStream) {
        val plane = image.planes[0]
        val buffer = plane.buffer.duplicate()
        val rowBytes = image.width * 2
        val row = ByteArray(rowBytes)
        for (y in 0 until image.height) {
            buffer.position(y * plane.rowStride)
            buffer.get(row)
            stream.write(row)
        }
    }

    private fun writeFile(file: File, writer: (FileOutputStream) -> Unit): JSONObject {
        FileOutputStream(file).use(writer)
        val digest = MessageDigest.getInstance("SHA-256").digest(file.readBytes())
        return JSONObject()
            .put("file", file.name)
            .put("bytes", file.length())
            .put("sha256", digest.joinToString("") { "%02x".format(it) })
    }

    private companion object {
        const val READER_DEPTH = 8

        /** Сколько последних RAW-кадров держим открытыми (меньше READER_DEPTH с запасом под снимок). */
        const val RAW_IMAGES_KEPT = 3
        const val OPEN_TIMEOUT_MS = 5_000L
        const val CONVERGENCE_MS = 2_000L
        const val SETTLE_MS = 300L
        const val STILL_TIMEOUT_S = 5L
        const val JPEG_QUALITY: Byte = 95
    }
}
