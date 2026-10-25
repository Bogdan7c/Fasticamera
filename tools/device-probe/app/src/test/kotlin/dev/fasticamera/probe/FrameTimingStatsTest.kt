package dev.fasticamera.probe

import dev.fasticamera.probe.camera.FrameTimingStats
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameTimingStatsTest {

    private val period60Ns = 16_666_667L

    private fun computed(timestamps: List<Long>): FrameTimingStats {
        val outcome = FrameTimingStats.compute(timestamps)
        assertTrue("ожидалась посчитанная статистика, а не $outcome", outcome is FrameTimingStats.Outcome.Computed)
        return (outcome as FrameTimingStats.Outcome.Computed).stats
    }

    @Test
    fun `ровный поток 60 кадров в секунду даёт 60 к_с и ноль пропусков`() {
        val timestamps = (0 until 600).map { it * period60Ns }
        val stats = computed(timestamps)
        assertEquals(60.0, stats.fpsFromMedian, 0.01)
        assertEquals(60.0, stats.fpsFromSpan, 0.01)
        assertEquals(0, stats.dropEvents)
        assertEquals(0, stats.estimatedDroppedFrames)
        assertEquals(600, stats.frameCount)
    }

    @Test
    fun `дыра в три периода считается как два пропущенных кадра`() {
        // Кадры 10 и 11 не пришли: между 9 и 12 интервал = 3 периода.
        val timestamps = (0 until 100).filter { it != 10 && it != 11 }.map { it * period60Ns }
        val stats = computed(timestamps)
        assertEquals(1, stats.dropEvents)
        assertEquals(2, stats.estimatedDroppedFrames)
        assertEquals(60.0, stats.fpsFromMedian, 0.01)
        // По полной длительности темп ниже: пропуски честно видны и здесь.
        assertTrue(stats.fpsFromSpan < 59.0)
    }

    @Test
    fun `поток 30 к_с не выдаётся за 60`() {
        val timestamps = (0 until 300).map { it * 33_333_333L }
        val stats = computed(timestamps)
        assertEquals(30.0, stats.fpsFromMedian, 0.01)
        assertEquals(0, stats.dropEvents)
    }

    @Test
    fun `повтор метки считается немонотонным, а не нулевым интервалом`() {
        val timestamps = listOf(0L, period60Ns, period60Ns, 2 * period60Ns, 3 * period60Ns)
        val stats = computed(timestamps)
        assertEquals(1, stats.nonMonotonicCount)
        assertEquals(60.0, stats.fpsFromMedian, 0.01)
    }

    @Test
    fun `одна метка — недостаточно данных, а не ноль кадров в секунду`() {
        val outcome = FrameTimingStats.compute(listOf(123L))
        assertEquals(FrameTimingStats.Outcome.Insufficient(1), outcome)
        assertEquals("insufficient", outcome.toJson().getString("status"))
    }
}
