package dev.fasticamera.probe.camera.shading

import android.hardware.camera2.params.LensShadingMap

/** Выравнивание карты затенения в плоский массив в порядке Camera2. */
object LensShadingGains {
    /** Число каналов карты: [R, G_even, G_odd, B]. */
    const val CHANNELS = 4

    /**
     * Плоский массив: для каждой точки сетки по строкам (row-major), внутри точки каналы [R, Geven, Godd, B].
     * Индекс элемента: (row * columns + column) * 4 + channel.
     */
    fun flatten(columns: Int, rows: Int, gainAt: (channel: Int, column: Int, row: Int) -> Float): FloatArray {
        val gains = FloatArray(columns * rows * CHANNELS)
        var index = 0
        for (row in 0 until rows) for (column in 0 until columns) for (channel in 0 until CHANNELS) {
            gains[index++] = gainAt(channel, column, row)
        }
        return gains
    }

    fun flatten(map: LensShadingMap): FloatArray =
        flatten(map.columnCount, map.rowCount) { channel, column, row -> map.getGainFactor(channel, column, row) }
}
