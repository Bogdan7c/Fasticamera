package dev.fasticamera.probe.heif

import android.graphics.Bitmap
import android.graphics.ImageFormat
import android.graphics.Rect
import android.graphics.YuvImage
import android.os.SystemClock
import androidx.heifwriter.HeifWriter
import java.io.File
import java.io.FileOutputStream

/** Результат одного кодирования: файл и время. Время кодирования отдельно от времени конвертации RGB→YUV на CPU. */
data class EncodeOutcome(val file: File, val encodeMillis: Long, val convertMillis: Long)

/**
 * Пути (a–f) записи мира в HEIF/JPEG. Каждый метод пишет один файл и возвращает [EncodeOutcome];
 * исключения не глотаем — сценарий логирует их как ошибку именно этого пути.
 */
class HeifEncodePaths(private val chart: ColorChart, private val outputDirectory: File) {

    private val argbPixels: IntArray by lazy { chart.toArgbPixels() }

    /** (a) HEIF из ARGB_8888 Bitmap: RGB→YUV делает кодер/GL внутри HeifWriter. */
    fun heifBitmap(file: File, gridEnabled: Boolean): EncodeOutcome {
        val bitmap = chart.toBitmap()
        val startedAt = SystemClock.elapsedRealtime()
        val writer = newWriter(file, HeifWriter.INPUT_MODE_BITMAP, gridEnabled)
        try {
            writer.start()
            writer.addBitmap(bitmap)
            writer.stop(STOP_TIMEOUT_MS)
        } finally {
            writer.close()
            bitmap.recycle()
        }
        return EncodeOutcome(file, SystemClock.elapsedRealtime() - startedAt, convertMillis = 0)
    }

    /** (b–d) HEIF из нашего I420 с заданной матрицей и диапазоном. */
    fun heifYuvBuffer(file: File, matrix: YuvMatrix, range: YuvRange, gridEnabled: Boolean): EncodeOutcome {
        val convertStart = SystemClock.elapsedRealtime()
        val i420 = YuvConverter.toI420(argbPixels, chart.width, chart.height, matrix, range)
        val convertMillis = SystemClock.elapsedRealtime() - convertStart
        val startedAt = SystemClock.elapsedRealtime()
        val writer = newWriter(file, HeifWriter.INPUT_MODE_BUFFER, gridEnabled)
        try {
            writer.start()
            writer.addYuvBuffer(ImageFormat.YUV_420_888, i420)
            writer.stop(STOP_TIMEOUT_MS)
        } finally {
            writer.close()
        }
        return EncodeOutcome(file, SystemClock.elapsedRealtime() - startedAt, convertMillis)
    }

    /** (e) JPEG через YuvImage(NV21): BT.601 full — то, что подразумевает JFIF. */
    fun jpegYuvImage(file: File): EncodeOutcome {
        val convertStart = SystemClock.elapsedRealtime()
        val nv21 = YuvConverter.toNv21(argbPixels, chart.width, chart.height, YuvMatrix.BT601, YuvRange.FULL)
        val convertMillis = SystemClock.elapsedRealtime() - convertStart
        val startedAt = SystemClock.elapsedRealtime()
        FileOutputStream(file).use { stream ->
            val image = YuvImage(nv21, ImageFormat.NV21, chart.width, chart.height, null)
            check(image.compressToJpeg(Rect(0, 0, chart.width, chart.height), JPEG_QUALITY, stream)) {
                "YuvImage.compressToJpeg вернул false"
            }
        }
        return EncodeOutcome(file, SystemClock.elapsedRealtime() - startedAt, convertMillis)
    }

    /** (f) JPEG через Bitmap.compress. */
    fun jpegBitmap(file: File): EncodeOutcome {
        val bitmap = chart.toBitmap()
        val startedAt = SystemClock.elapsedRealtime()
        try {
            FileOutputStream(file).use { stream ->
                check(bitmap.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, stream)) { "Bitmap.compress вернул false" }
            }
        } finally {
            bitmap.recycle()
        }
        return EncodeOutcome(file, SystemClock.elapsedRealtime() - startedAt, convertMillis = 0)
    }

    private fun newWriter(file: File, inputMode: Int, gridEnabled: Boolean): HeifWriter =
        HeifWriter.Builder(file.absolutePath, chart.width, chart.height, inputMode)
            .setQuality(HEIF_QUALITY)
            .setRotation(/* градусы */ 0)
            .setGridEnabled(gridEnabled)
            .setMaxImages(1)
            .build()

    companion object {
        const val HEIF_QUALITY = 95
        const val JPEG_QUALITY = 95
        private const val STOP_TIMEOUT_MS = 30_000L
    }
}
