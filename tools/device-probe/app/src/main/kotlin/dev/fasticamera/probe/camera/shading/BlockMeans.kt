package dev.fasticamera.probe.camera.shading

import android.hardware.camera2.CameraMetadata

/** Сторона блока усреднения в пикселях буфера (по каждой оси). */
const val BLOCK_SIZE_PIXELS = 32

/** Число CFA-плоскостей: позиции (строка, столбец) = (0,0), (0,1), (1,0), (1,1) в буфере. */
const val CFA_PLANE_COUNT = 4

/** Индекс CFA-плоскости пикселя: чётность строки * 2 + чётность столбца (порядок BlackLevelPattern). */
fun cfaPlaneIndex(x: Int, y: Int): Int = (y and 1) * 2 + (x and 1)

/** Сетка блоков по валидной области буфера; неполные блоки на краях отбрасываются. */
data class BlockGrid(val blocksX: Int, val blocksY: Int) {
    val blocksPerPlane: Int get() = blocksX * blocksY

    companion object {
        fun forValidArea(width: Int, validRows: Int) = BlockGrid(width / BLOCK_SIZE_PIXELS, validRows / BLOCK_SIZE_PIXELS)
    }
}

/**
 * Определение числа строк буфера, в которых есть данные.
 *
 * Нужно для main169: HAL выбирает режим сенсора 4096x2304, а буфер 4096x3072 — данные лежат
 * в строках 0..2303, ниже нули. Реальный RAW не бывает нулевым целиком (есть чёрный уровень),
 * поэтому «валидные» строки = всё до последней строки с ненулевым пикселем.
 */
object ValidRows {
    fun detect(pixels: ShortArray, width: Int, height: Int): Int {
        require(pixels.size >= width * height) { "буфер ${pixels.size} меньше ${width}x$height" }
        for (row in height - 1 downTo 0) {
            val rowStart = row * width
            for (x in 0 until width) {
                if (pixels[rowStart + x].toInt() != 0) return row + 1
            }
        }
        return 0
    }
}

/**
 * Накопитель средних по блокам [BLOCK_SIZE_PIXELS]² отдельно для 4 CFA-плоскостей.
 *
 * Суммы хранятся в Long (точно, без накопления ошибки округления double) — по одной на блок и плоскость;
 * в блоке на каждую плоскость приходится (32/2)² = 256 отсчётов. Память не зависит от разрешения кадра.
 * Чёрный уровень НЕ вычитается: так анализ на ПК сам решает, какой чёрный брать.
 */
class BlockMeanAccumulator(private val width: Int, validRows: Int) {
    val grid: BlockGrid = BlockGrid.forValidArea(width, validRows)
    private val sums = Array(CFA_PLANE_COUNT) { LongArray(grid.blocksPerPlane) }

    var framesAccumulated: Int = 0
        private set

    /**
     * Добавляет кадр: [pixels] — плотный массив width x (>= validRows) значений RAW.
     * [frameValidRows] — сколько строк этого кадра содержат данные; их должно хватать на всю сетку.
     */
    fun addFrame(pixels: ShortArray, frameWidth: Int, frameValidRows: Int) {
        require(frameWidth == width) { "ширина кадра $frameWidth, ожидалась $width" }
        val rowsNeeded = grid.blocksY * BLOCK_SIZE_PIXELS
        require(frameValidRows >= rowsNeeded) { "в кадре $frameValidRows валидных строк, сетке нужно $rowsNeeded" }
        for (y in 0 until rowsNeeded) {
            accumulateRow(pixels, y)
        }
        framesAccumulated++
    }

    private fun accumulateRow(pixels: ShortArray, y: Int) {
        val rowParityBase = (y and 1) * 2
        val blockRowOffset = (y / BLOCK_SIZE_PIXELS) * grid.blocksX
        val evenColumnSums = sums[rowParityBase]
        val oddColumnSums = sums[rowParityBase + 1]
        val usedWidth = grid.blocksX * BLOCK_SIZE_PIXELS
        val rowStart = y * width
        var x = 0
        while (x < usedWidth) {
            val blockIndex = blockRowOffset + x / BLOCK_SIZE_PIXELS
            evenColumnSums[blockIndex] += (pixels[rowStart + x].toInt() and PIXEL_MASK).toLong()
            oddColumnSums[blockIndex] += (pixels[rowStart + x + 1].toInt() and PIXEL_MASK).toLong()
            x += 2
        }
    }

    /** Средние DN за все добавленные кадры, раскладка plane-major [4][blocksY][blocksX]. */
    fun means(): FloatArray {
        check(framesAccumulated > 0) { "ни одного кадра не добавлено" }
        val samplesPerBlock = framesAccumulated.toDouble() * SAMPLES_PER_BLOCK_PER_PLANE
        val result = FloatArray(CFA_PLANE_COUNT * grid.blocksPerPlane)
        for (plane in 0 until CFA_PLANE_COUNT) {
            for (block in 0 until grid.blocksPerPlane) {
                result[plane * grid.blocksPerPlane + block] = (sums[plane][block] / samplesPerBlock).toFloat()
            }
        }
        return result
    }

    private companion object {
        /** Значения RAW10 лежат в 0..1023, но ShortArray знаковый: маска защищает от отрицательных при RAW16. */
        const val PIXEL_MASK = 0xFFFF
        const val SAMPLES_PER_BLOCK_PER_PLANE = (BLOCK_SIZE_PIXELS / 2) * (BLOCK_SIZE_PIXELS / 2)
    }
}

/** Уровни кадра для подбора экспозиции: центр по зелёному и самый яркий блок, оба нормированы в 0..1. */
data class FrameLevels(val centreGreen: Double, val brightestBlock: Double)

/** Статистика поверх средних по блокам: уровни, отношение угол/центр. Всё — чистые функции. */
object BlockStatistics {
    /** Доля стороны кадра, которую занимает «центр» для подбора экспозиции (10% x 10% площади). */
    private const val CENTRE_FRACTION = 0.10

    /** Позиции буфера (индексы CFA-плоскостей), где стоит зелёный, для заданной раскладки CFA камеры. */
    fun greenPlanes(cfaArrangement: Int): IntArray = when (cfaArrangement) {
        CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_RGGB,
        CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_BGGR,
        -> intArrayOf(1, 2)
        CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GRBG,
        CameraMetadata.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT_GBRG,
        -> intArrayOf(0, 3)
        else -> throw IllegalArgumentException("раскладка CFA $cfaArrangement не байеровская")
    }

    /**
     * Уровни кадра. Центр — квадрат из ~10% блоков по каждой оси по центру сетки, среднее зелёных плоскостей;
     * яркий блок — максимум по всем плоскостям (клиппинг любого канала недопустим). Нормировка: (DN - чёрный) / (белый - чёрный).
     */
    fun levels(means: FloatArray, grid: BlockGrid, greenPlanes: IntArray, blackByPlane: DoubleArray, whiteLevel: Int): FrameLevels {
        val centreBlocksX = (grid.blocksX * CENTRE_FRACTION).toInt().coerceAtLeast(1)
        val centreBlocksY = (grid.blocksY * CENTRE_FRACTION).toInt().coerceAtLeast(1)
        val centreX = (grid.blocksX - centreBlocksX) / 2
        val centreY = (grid.blocksY - centreBlocksY) / 2
        val centreMean = regionMean(means, grid, greenPlanes, blackByPlane, centreX until centreX + centreBlocksX, centreY until centreY + centreBlocksY)
        val greenBlack = greenPlanes.map { blackByPlane[it] }.average()
        var brightest = 0.0
        for (plane in 0 until CFA_PLANE_COUNT) {
            val range = whiteLevel - blackByPlane[plane]
            for (block in 0 until grid.blocksPerPlane) {
                brightest = maxOf(brightest, (means[plane * grid.blocksPerPlane + block] - blackByPlane[plane]) / range)
            }
        }
        return FrameLevels((centreMean) / (whiteLevel - greenBlack), brightest)
    }

    /**
     * Отношение «угол / центр» по выбранным плоскостям, с вычтенным чёрным.
     * Угол — среднее 4 угловых блоков, отступив на один блок от края; центр — центральные 3x3 блока.
     * null — сетка слишком мала для такой геометрии.
     */
    fun cornerCentreRatio(means: FloatArray, grid: BlockGrid, planes: IntArray, blackByPlane: DoubleArray): Double? {
        if (grid.blocksX < MIN_GRID_FOR_RATIO || grid.blocksY < MIN_GRID_FOR_RATIO) return null
        val lastX = grid.blocksX - 2
        val lastY = grid.blocksY - 2
        val cornerMean = listOf(1 to 1, lastX to 1, 1 to lastY, lastX to lastY).map { (bx, by) ->
            regionMean(means, grid, planes, blackByPlane, bx..bx, by..by)
        }.average()
        val startX = (grid.blocksX - CENTRE_SIDE_BLOCKS) / 2
        val startY = (grid.blocksY - CENTRE_SIDE_BLOCKS) / 2
        val centreMean = regionMean(
            means, grid, planes, blackByPlane,
            startX until startX + CENTRE_SIDE_BLOCKS, startY until startY + CENTRE_SIDE_BLOCKS,
        )
        return cornerMean / centreMean
    }

    /** Среднее (DN - чёрный) по прямоугольнику блоков и набору плоскостей. */
    fun regionMean(
        means: FloatArray,
        grid: BlockGrid,
        planes: IntArray,
        blackByPlane: DoubleArray,
        blocksX: IntRange,
        blocksY: IntRange,
    ): Double {
        var total = 0.0
        var count = 0
        for (plane in planes) {
            for (by in blocksY) for (bx in blocksX) {
                total += means[plane * grid.blocksPerPlane + by * grid.blocksX + bx] - blackByPlane[plane]
                count++
            }
        }
        return total / count
    }

    /** Угол с отступом на блок + центр 3x3 требуют минимум 5 блоков по каждой оси. */
    private const val MIN_GRID_FOR_RATIO = 5
    private const val CENTRE_SIDE_BLOCKS = 3
}
