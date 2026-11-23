package dev.fasticamera.probe

import android.hardware.camera2.CameraMetadata
import dev.fasticamera.probe.camera.shading.BLOCK_SIZE_PIXELS
import dev.fasticamera.probe.camera.shading.BlockGrid
import dev.fasticamera.probe.camera.shading.BlockMeanAccumulator
import dev.fasticamera.probe.camera.shading.BlockStatistics
import dev.fasticamera.probe.camera.shading.ValidRows
import dev.fasticamera.probe.camera.shading.cfaPlaneIndex
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class ShadingBlockMeansTest {
    /** Мозаика, где значение однозначно кодирует (плоскость CFA, блок): ошибка в чётности или в блоке сломает числа. */
    private fun mosaic(width: Int, rows: Int, extra: Int = 0): ShortArray = ShortArray(width * rows) { index ->
        val x = index % width
        val y = index / width
        val block = (y / BLOCK_SIZE_PIXELS) * (width / BLOCK_SIZE_PIXELS) + x / BLOCK_SIZE_PIXELS
        (cfaPlaneIndex(x, y) * 100 + block + extra).toShort()
    }

    @Test
    fun `средние по блокам раскладываются по плоскостям строка-столбец точно`() {
        val width = 3 * BLOCK_SIZE_PIXELS
        val rows = 2 * BLOCK_SIZE_PIXELS
        val accumulator = BlockMeanAccumulator(width, rows)
        accumulator.addFrame(mosaic(width, rows), width, rows)
        val means = accumulator.means()
        val perPlane = accumulator.grid.blocksPerPlane
        assertEquals(6, perPlane)
        for (plane in 0 until 4) for (block in 0 until perPlane) {
            assertEquals("плоскость $plane блок $block", (plane * 100 + block).toFloat(), means[plane * perPlane + block], 0f)
        }
    }

    @Test
    fun `два кадра усредняются покадрово`() {
        val width = 2 * BLOCK_SIZE_PIXELS
        val rows = 2 * BLOCK_SIZE_PIXELS
        val accumulator = BlockMeanAccumulator(width, rows)
        accumulator.addFrame(mosaic(width, rows, extra = 0), width, rows)
        accumulator.addFrame(mosaic(width, rows, extra = 10), width, rows)
        // Плоскость 2 (строка нечётная, столбец чётный), блок 3: 200 + 3, во втором кадре на 10 больше.
        assertEquals(208f, accumulator.means()[2 * 4 + 3], 0f)
        assertEquals(2, accumulator.framesAccumulated)
    }

    @Test
    fun `неполные блоки на краях отбрасываются, а валидных строк меньше сетки - ошибка`() {
        val width = BLOCK_SIZE_PIXELS + 6
        val accumulator = BlockMeanAccumulator(width, BLOCK_SIZE_PIXELS + 5)
        assertEquals(BlockGrid(1, 1), accumulator.grid)
        assertThrows(IllegalArgumentException::class.java) {
            accumulator.addFrame(ShortArray(width * 40), width, frameValidRows = BLOCK_SIZE_PIXELS - 1)
        }
        assertEquals(0, accumulator.framesAccumulated)
    }

    @Test
    fun `валидные строки определяются по последней ненулевой, нули снизу отсекаются`() {
        val width = 8
        val pixels = ShortArray(width * 96)
        for (row in 0 until 72) pixels[row * width + 3] = 64
        assertEquals(72, ValidRows.detect(pixels, width, 96))
    }

    @Test
    fun `полный кадр и полностью нулевой кадр`() {
        val width = 4
        assertEquals(10, ValidRows.detect(ShortArray(width * 10) { 64 }, width, 10))
        assertEquals(0, ValidRows.detect(ShortArray(width * 10), width, 10))
    }

    @Test
    fun `main169-подобный буфер даёт сетку только по строкам с данными`() {
        val width = 2 * BLOCK_SIZE_PIXELS
        val height = 4 * BLOCK_SIZE_PIXELS
        val pixels = ShortArray(width * height) { index -> if (index / width < 3 * BLOCK_SIZE_PIXELS) 64 else 0 }
        val validRows = ValidRows.detect(pixels, width, height)
        assertEquals(3 * BLOCK_SIZE_PIXELS, validRows)
        val accumulator = BlockMeanAccumulator(width, validRows)
        accumulator.addFrame(pixels, width, validRows)
        assertEquals(BlockGrid(2, 3), accumulator.grid)
        accumulator.means().forEach { assertEquals(64f, it, 0f) }
    }

    @Test
    fun `позиции зелёного по раскладке CFA`() {
        assertArrayEquals(intArrayOf(1, 2), BlockStatistics.greenPlanes(CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_RGGB))
        assertArrayEquals(intArrayOf(1, 2), BlockStatistics.greenPlanes(CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_BGGR))
        assertArrayEquals(intArrayOf(0, 3), BlockStatistics.greenPlanes(CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GRBG))
        assertArrayEquals(intArrayOf(0, 3), BlockStatistics.greenPlanes(CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GBRG))
        assertThrows(IllegalArgumentException::class.java) {
            BlockStatistics.greenPlanes(CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_RGB)
        }
    }

    /** Сетка 11x11 (нечётная: центр 3x3 симметричен): зелёные плоскости (RGGB) = 164 DN в углу блока (1,1) и 264 по центру, чёрный 64, белый 1023. */
    private fun vignettedMeans(): FloatArray {
        val grid = BlockGrid(11, 11)
        val means = FloatArray(4 * grid.blocksPerPlane) { 100f }
        for (plane in intArrayOf(1, 2)) for (by in 0 until 11) for (bx in 0 until 11) {
            val isCentre = bx in 4..6 && by in 4..6
            means[plane * 121 + by * 11 + bx] = if (isCentre) 264f else 164f
        }
        return means
    }

    @Test
    fun `отношение угол к центру считает угловые блоки с отступом и центр 3x3 с вычетом чёрного`() {
        val ratio = BlockStatistics.cornerCentreRatio(vignettedMeans(), BlockGrid(11, 11), intArrayOf(1, 2), DoubleArray(4) { 64.0 })
        assertNotNull(ratio)
        assertEquals((164.0 - 64.0) / (264.0 - 64.0), ratio!!, 1e-9)
    }

    @Test
    fun `на слишком маленькой сетке отношение не определено`() {
        assertNull(BlockStatistics.cornerCentreRatio(FloatArray(4 * 16), BlockGrid(4, 4), intArrayOf(1), DoubleArray(4)))
    }

    @Test
    fun `уровни кадра берут центр по зелёному и самый яркий блок среди всех плоскостей`() {
        val means = vignettedMeans()
        means[0 * 121 + 5 * 11 + 5] = 563f // красный блок ярче зелёных
        val levels = BlockStatistics.levels(means, BlockGrid(11, 11), intArrayOf(1, 2), DoubleArray(4) { 64.0 }, 1023)
        assertEquals((264.0 - 64.0) / (1023.0 - 64.0), levels.centreGreen, 1e-9)
        assertEquals((563.0 - 64.0) / (1023.0 - 64.0), levels.brightestBlock, 1e-9)
    }
}
