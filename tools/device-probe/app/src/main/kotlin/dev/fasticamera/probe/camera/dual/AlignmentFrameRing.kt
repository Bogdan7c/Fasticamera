package dev.fasticamera.probe.camera.dual

import android.media.Image
import java.nio.ByteBuffer

/** Копия одного RAW-кадра, снятая внутри колбэка ImageReader: сам Image к моменту анализа уже возвращён. */
class RawFrameCopy(val timestampNs: Long, val rowStrideBytes: Int, val bytes: ByteBuffer)

/**
 * Кольцо последних [capacity] сырых кадров одной камеры (копии в предвыделенных direct-буферах).
 * Нужно, чтобы выбрать из двух потоков пару кадров с ближайшими метками сенсора, а тяжёлую конвертацию
 * в зелёную плоскость сделать уже после остановки записи — не на потоке ImageReader.
 *
 * Пока кольцо заморожено ([freeze]), новые кадры пропускаются; [thaw] очищает его и возобновляет запись.
 */
class AlignmentFrameRing(private val capacity: Int, private val bufferBytes: Int) : RawFrameSink {
    private val lock = Any()
    private val slots = List(capacity) { ByteBuffer.allocateDirect(bufferBytes) }
    private val timestamps = LongArray(capacity)
    private val strides = IntArray(capacity)
    private var stored = 0
    private var nextSlot = 0
    private var frozen = true
    private var failure: String? = null

    /** Сколько кадров сейчас в кольце. */
    val storedCount: Int get() = synchronized(lock) { stored }

    fun failureOrNull(): String? = synchronized(lock) { failure }

    /** Очищает кольцо и начинает запись заново. */
    fun thaw() = synchronized(lock) {
        stored = 0
        nextSlot = 0
        frozen = false
    }

    /** Останавливает запись и отдаёт копии (в порядке прихода); буферы валидны до следующего [thaw]. */
    fun freeze(): List<RawFrameCopy> = synchronized(lock) {
        frozen = true
        val oldest = (nextSlot - stored + capacity) % capacity
        List(stored) { offset ->
            val slot = (oldest + offset) % capacity
            RawFrameCopy(timestamps[slot], strides[slot], slots[slot].duplicate().apply { clear() })
        }
    }

    override fun onFrame(image: Image) {
        synchronized(lock) {
            if (frozen) return
            try {
                val plane = image.planes.first()
                val source = plane.buffer.duplicate()
                check(source.remaining() <= bufferBytes) { "кадр ${source.remaining()} Б не влезает в слот $bufferBytes Б" }
                val slot = slots[nextSlot]
                slot.clear()
                slot.put(source)
                timestamps[nextSlot] = image.timestamp
                strides[nextSlot] = plane.rowStride
                nextSlot = (nextSlot + 1) % capacity
                stored = minOf(stored + 1, capacity)
            } catch (error: RuntimeException) {
                failure = "${error.javaClass.name}: ${error.message}"
            }
        }
    }
}
