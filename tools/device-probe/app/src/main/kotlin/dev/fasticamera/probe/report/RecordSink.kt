package dev.fasticamera.probe.report

import android.util.Log
import java.io.File
import java.io.FileWriter
import java.io.IOException

/**
 * Куда уходят записи зонда. Каналов два, и у каждого своя роль:
 * - [FileSink] — основной: полный JSONL в файле приложения, ПК забирает его через `run-as`.
 *   Нужен потому, что кольцевой буфер logcat на телефоне всего 256 КиБ и HAL камеры
 *   вытесняет из него наши записи за секунды (факт сессии 00).
 * - [LogcatSink] — для человека: видно ход сценария в `adb logcat -s FasticameraProbe`.
 */
fun interface RecordSink {
    fun write(line: String)
}

class LogcatSink(private val tag: String, private val chunker: LogChunker) : RecordSink {
    private var chunkCounter = 0L

    @Synchronized
    override fun write(line: String) {
        chunkCounter += 1
        for (piece in chunker.split(line, "$tag-$chunkCounter")) {
            Log.i(tag, piece)
        }
    }
}

/**
 * Дописывает строки в файл и сразу сбрасывает их на диск: если процесс упадёт посреди
 * сценария, уже записанные результаты останутся, а отсутствие `done` покажет сбой.
 */
class FileSink(private val file: File) : RecordSink {

    @Synchronized
    override fun write(line: String) {
        try {
            file.parentFile?.mkdirs()
            FileWriter(file, /* append = */ true).use { writer ->
                writer.write(line)
                writer.write("\n")
            }
        } catch (error: IOException) {
            // Основной канал сломан — сообщаем во второй канал, а не глотаем.
            Log.e(ProbeLog.TAG, "не удалось записать ${file.absolutePath}: ${error.message}")
        }
    }
}
