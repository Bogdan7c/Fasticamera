package dev.fasticamera.probe.heif

import android.graphics.Bitmap

/** Прямоугольник патча в пикселях картинки. */
data class PatchRect(val left: Int, val top: Int, val width: Int, val height: Int) {
    /** Центральные 50% по каждой оси: далеко от границ, куда «протекает» субдискретизация цвета. */
    fun central(): PatchRect = PatchRect(left + width / 4, top + height / 4, width / 2, height / 2)
}

/** Один однотонный патч миры с известным эталонным sRGB. */
data class ChartPatch(val name: String, val group: PatchGroup, val rect: PatchRect, val red: Int, val green: Int, val blue: Int)

/** Группа нужна отчёту: отдельно считаем худшее отклонение на серых и на основных цветах. */
enum class PatchGroup { GRAY, PRIMARY, SKIN_SKY }

/**
 * Синтетическая мира: сетка 6×4 однотонных патчей с известными 8-бит sRGB.
 * Масштаб задаётся размером: каждый патч занимает целую ячейку сетки
 * (на 1024×768 это 170×192 пикселя, на 4096×3072 — 682×768).
 */
class ColorChart(val width: Int, val height: Int) {

    val patches: List<ChartPatch> = buildPatches()

    /** Пиксели ARGB (sRGB, непрозрачные) всей миры. */
    fun toArgbPixels(): IntArray {
        val pixels = IntArray(width * height)
        for (patch in patches) {
            val argb = (0xFF shl 24) or (patch.red shl 16) or (patch.green shl 8) or patch.blue
            val rect = patch.rect
            for (row in rect.top until rect.top + rect.height) {
                val rowStart = row * width
                java.util.Arrays.fill(pixels, rowStart + rect.left, rowStart + rect.left + rect.width, argb)
            }
        }
        return pixels
    }

    fun toBitmap(): Bitmap {
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        bitmap.setPixels(toArgbPixels(), 0, width, 0, 0, width, height)
        return bitmap
    }

    private fun buildPatches(): List<ChartPatch> {
        val cellWidth = width / COLUMNS
        val cellHeight = height / ROWS
        return SPECS.mapIndexed { index, spec ->
            val rect = PatchRect((index % COLUMNS) * cellWidth, (index / COLUMNS) * cellHeight, cellWidth, cellHeight)
            ChartPatch(spec.name, spec.group, rect, spec.red, spec.green, spec.blue)
        }
    }

    private class Spec(val name: String, val group: PatchGroup, val red: Int, val green: Int, val blue: Int)

    companion object {
        const val COLUMNS = 6
        const val ROWS = 4

        private val GRAY_LEVELS = listOf(0, 16, 32, 64, 96, 128, 160, 192, 224, 255)

        /** Основные цвета R,G,B,C,M,Y при значении [level] на включённых каналах. */
        private fun primaries(level: Int): List<Spec> = listOf(
            Spec("R$level", PatchGroup.PRIMARY, level, 0, 0),
            Spec("G$level", PatchGroup.PRIMARY, 0, level, 0),
            Spec("B$level", PatchGroup.PRIMARY, 0, 0, level),
            Spec("C$level", PatchGroup.PRIMARY, 0, level, level),
            Spec("M$level", PatchGroup.PRIMARY, level, 0, level),
            Spec("Y$level", PatchGroup.PRIMARY, level, level, 0),
        )

        private val SPECS: List<Spec> =
            GRAY_LEVELS.map { Spec("gray$it", PatchGroup.GRAY, it, it, it) } +
                primaries(255) + primaries(128) +
                listOf(
                    Spec("skin", PatchGroup.SKIN_SKY, 194, 150, 130),
                    Spec("sky", PatchGroup.SKIN_SKY, 100, 150, 220),
                )

        init {
            check(SPECS.size == COLUMNS * ROWS) { "патчей ${SPECS.size}, а ячеек ${COLUMNS * ROWS}" }
        }
    }
}
