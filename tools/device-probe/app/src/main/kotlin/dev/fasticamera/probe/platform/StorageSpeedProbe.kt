package dev.fasticamera.probe.platform

import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/**
 * C6: устойчивая скорость последовательной записи во внутреннюю память (справочно, для RAW-видео).
 * Пишем блоками в файл приложения с fsync в конце и удаляем файл в любом исходе.
 */
object StorageSpeedProbe {

    private const val BLOCK_BYTES = 8 * 1024 * 1024
    private const val BYTES_PER_MEBIBYTE = 1024.0 * 1024.0
    private const val NANOS_PER_SECOND = 1_000_000_000.0

    fun measure(directory: File, totalMebibytes: Int): JSONObject {
        val target = File(directory, "probe-write-test.bin")
        val block = ByteArray(BLOCK_BYTES) { index -> index.toByte() }
        val blocks = (totalMebibytes * BYTES_PER_MEBIBYTE / BLOCK_BYTES).toInt().coerceAtLeast(1)
        val measurement = try {
            val startNs = System.nanoTime()
            FileOutputStream(target).use { stream ->
                repeat(blocks) { stream.write(block) }
                // Без fsync замер показал бы скорость кэша страниц, а не флеш-памяти.
                stream.fd.sync()
            }
            val elapsedNs = System.nanoTime() - startNs
            val writtenBytes = blocks.toLong() * BLOCK_BYTES
            JSONObject()
                .put("status", "measured")
                .put("written_mib", writtenBytes / BYTES_PER_MEBIBYTE)
                .put("elapsed_ms", elapsedNs / 1_000_000.0)
                .put("mib_per_second", writtenBytes / BYTES_PER_MEBIBYTE / (elapsedNs / NANOS_PER_SECOND))
        } catch (error: IOException) {
            JSONObject().put("status", "io_error").put("error", error.message)
        }
        // Уборка — часть результата: неудалённый файл на 1 ГБ виден в отчёте, а не теряется молча.
        val cleanedUp = !target.exists() || target.delete()
        return measurement.put("test_file_removed", cleanedUp)
    }
}
