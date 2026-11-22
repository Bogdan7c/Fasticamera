package dev.fasticamera

import android.os.ParcelFileDescriptor
import android.view.Surface
import androidx.heifwriter.HeifWriter
import java.util.concurrent.TimeoutException

/**
 * Фабрика кодеров HEIF одного снимка (Rust: `android-shell::android::still_heif_encoder`, трейт
 * `HeifEncoderService`). Решение 142: кодер — androidx `HeifWriter` в режиме входной поверхности;
 * Vulkan рисует ОДИН кадр прямо в её `Surface`.
 *
 * Только вызовы API платформы. Вызывается с рабочего потока записи снимка.
 */
class StillHeifEncoder(private val store: StillMediaStore) {

    /**
     * Создаёт кодер, пишущий в черновик [pendingUri]. Порядок обязателен (исходники `WriterBase`):
     * `getInputSurface()` — ДО `start()` (после него бросает), Exif — ПОСЛЕ `start()`.
     * Любое исключение закрывает всё уже открытое и уходит в Rust.
     */
    fun open(
        pendingUri: String,
        width: Int,
        height: Int,
        rotationDegrees: Int,
        quality: Int,
        exifBlock: ByteArray?,
    ): StillHeifSession {
        val descriptor = store.openEncoderDescriptor(pendingUri)
        var writer: HeifWriter? = null
        try {
            val openedWriter = HeifWriter.Builder(descriptor.fileDescriptor, width, height, HeifWriter.INPUT_MODE_SURFACE)
                .setQuality(quality)
                .setRotation(rotationDegrees)
                .setMaxImages(1)
                .build()
            writer = openedWriter
            val surface = openedWriter.inputSurface
            openedWriter.start()
            if (exifBlock != null) {
                openedWriter.addExifData(0, exifBlock, 0, exifBlock.size)
            }
            return StillHeifSession(openedWriter, surface, descriptor)
        } catch (error: Throwable) {
            // Не оставляем ни кодер, ни дескриптор: исключение всё равно идёт наверх. Исходная ошибка
            // главная; сбой закрытия не должен её спрятать и не должен помешать закрыть остальное,
            // поэтому он уходит в `addSuppressed`.
            closeSuppressingInto(error) { writer?.close() }
            closeSuppressingInto(error) { descriptor.close() }
            throw error
        }
    }
}

/**
 * Открытый кодер одного снимка. Rust держит глобальную ссылку на него до [finish] или [abort].
 */
class StillHeifSession internal constructor(
    private val writer: HeifWriter,
    private val surface: Surface,
    private val descriptor: ParcelFileDescriptor,
) {
    /** Входная поверхность кодера (Rust берёт из неё `ANativeWindow`). */
    fun inputSurface(): Surface = surface

    /**
     * Кадр уже показан в поверхность: дописать файл и закрыть кодер, ожидая не дольше [timeoutMillis].
     *
     * Метка конца потока — «сейчас» в часах `System.nanoTime()` (CLOCK_MONOTONIC): буфер Vulkan
     * ставится в очередь с монотонной меткой момента `queueBuffer`, то есть раньше «сейчас» — наш
     * единственный кадр проходит под отсечку, кадры позже (случайные повторные показы) — нет.
     * С `setMaxImages(1)` запись заканчивается и сама, когда вышел единственный кадр; метка лишь
     * закрывает вход.
     *
     * Возврат: [FINISH_DONE] или [FINISH_TIMED_OUT]. Остальные исключения идут в Rust как есть.
     * Поверхность, кодер и дескриптор закрываются в любом исходе.
     */
    fun finish(timeoutMillis: Long): Int {
        try {
            writer.setInputEndOfStreamTimestamp(System.nanoTime())
            writer.stop(timeoutMillis)
            return FINISH_DONE
        } catch (timeout: TimeoutException) {
            return FINISH_TIMED_OUT
        } finally {
            release()
        }
    }

    /** Бросить кодирование: без ожидания, файл после этого негоден (черновик удалит Rust). */
    fun abort() = release()

    private fun release() {
        try {
            writer.close()
        } finally {
            descriptor.close()
        }
    }

    companion object {
        /** Коды результата [finish] — контракт с `crates/android-shell/src/still_encode_input.rs`. */
        const val FINISH_DONE = 0
        const val FINISH_TIMED_OUT = 1
    }
}

/** Выполняет закрытие; его сбой дописывается к [primary] как подавленный, а не заменяет исходную ошибку. */
internal inline fun closeSuppressingInto(primary: Throwable, close: () -> Unit) {
    try {
        close()
    } catch (secondary: Throwable) {
        primary.addSuppressed(secondary)
    }
}
