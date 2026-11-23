package dev.fasticamera.probe.camera.shading

import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * Хранилище результатов кадров по метке сенсора: чтобы каждый кадр получил СВОЙ результат
 * (метка Image.timestamp == SENSOR_TIMESTAMP), даже если картинка пришла раньше результата или кадры пропускались.
 *
 * Держит последние [capacity] результатов. Поколение — номер применённого набора управления (тег запроса):
 * по нему отбрасываются кадры старого запроса, а по номеру кадра внутри поколения считается «успокоение».
 */
class FrameResultStore<R : Any>(private val capacity: Int) {
    private val lock = ReentrantLock()
    private val arrived = lock.newCondition()
    private val byTimestamp = LinkedHashMap<Long, R>()
    private val firstFrameByGeneration = HashMap<Int, Long>()
    private var latest: R? = null

    fun put(timestampNs: Long, generation: Int, frameNumber: Long, result: R) = lock.withLock {
        byTimestamp[timestampNs] = result
        while (byTimestamp.size > capacity) {
            val oldest = byTimestamp.keys.first()
            byTimestamp.remove(oldest)
        }
        firstFrameByGeneration.merge(generation, frameNumber) { known, incoming -> minOf(known, incoming) }
        latest = result
        arrived.signalAll()
    }

    /** Результат кадра с меткой [timestampNs]; ждёт до [timeoutMs], null — не пришёл или уже вытеснен. */
    fun awaitResult(timestampNs: Long, timeoutMs: Long): R? = lock.withLock {
        var remainingNs = TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        var found = byTimestamp[timestampNs]
        while (found == null && remainingNs > 0) {
            remainingNs = arrived.awaitNanos(remainingNs)
            found = byTimestamp[timestampNs]
        }
        found
    }

    /** Номер первого кадра поколения, из которого пришёл результат, или null. */
    fun firstFrameNumber(generation: Int): Long? = lock.withLock { firstFrameByGeneration[generation] }

    fun latestResult(): R? = lock.withLock { latest }
}
