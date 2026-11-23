package dev.fasticamera.probe

import dev.fasticamera.probe.camera.shading.FrameResultStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FrameResultStoreTest {
    @Test
    fun `результат находится по своей метке сенсора, а не по порядку прихода`() {
        val store = FrameResultStore<String>(capacity = 8)
        store.put(300, generation = 1, frameNumber = 3, result = "c")
        store.put(100, generation = 1, frameNumber = 1, result = "a")
        store.put(200, generation = 1, frameNumber = 2, result = "b")
        assertEquals("a", store.awaitResult(100, 0))
        assertEquals("b", store.awaitResult(200, 0))
        assertEquals("c", store.awaitResult(300, 0))
        assertNull(store.awaitResult(999, 5))
    }

    @Test
    fun `вытесняются самые старые результаты сверх ёмкости`() {
        val store = FrameResultStore<Int>(capacity = 2)
        store.put(1, 1, 1, 10)
        store.put(2, 1, 2, 20)
        store.put(3, 1, 3, 30)
        assertNull(store.awaitResult(1, 0))
        assertEquals(20, store.awaitResult(2, 0))
        assertEquals(30, store.awaitResult(3, 0))
        assertEquals(30, store.latestResult())
    }

    @Test
    fun `первый кадр поколения - минимальный номер даже при перепутанном порядке`() {
        val store = FrameResultStore<String>(capacity = 8)
        store.put(10, generation = 2, frameNumber = 52, result = "x")
        store.put(11, generation = 2, frameNumber = 50, result = "y")
        store.put(12, generation = 3, frameNumber = 60, result = "z")
        assertEquals(50L, store.firstFrameNumber(2))
        assertEquals(60L, store.firstFrameNumber(3))
        assertNull(store.firstFrameNumber(9))
    }

    @Test
    fun `ожидание получает результат, пришедший позже картинки`() {
        val store = FrameResultStore<String>(capacity = 8)
        val producer = Thread {
            Thread.sleep(50)
            store.put(777, 1, 1, "late")
        }
        producer.start()
        assertEquals("late", store.awaitResult(777, 2_000))
        producer.join()
    }
}
