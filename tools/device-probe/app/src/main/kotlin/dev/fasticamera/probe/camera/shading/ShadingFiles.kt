package dev.fasticamera.probe.camera.shading

import java.io.File
import java.io.FileOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Запись бинарных результатов сценария shading; формат файлов описан в README инструмента. */
object ShadingFiles {
    /** float32 little-endian подряд, без заголовка. */
    fun writeFloat32LittleEndian(file: File, values: FloatArray) {
        val buffer = ByteBuffer.allocate(values.size * Float.SIZE_BYTES).order(ByteOrder.LITTLE_ENDIAN)
        buffer.asFloatBuffer().put(values)
        FileOutputStream(file).use { it.write(buffer.array()) }
    }

    /** Плотный RAW16 little-endian (u16, без паддинга строк): первые [rows] строк массива [pixels] ширины [width]. */
    fun writeDenseRaw16(file: File, pixels: ShortArray, width: Int, rows: Int) {
        val rowsPerChunk = ROWS_PER_WRITE_CHUNK
        val chunk = ByteBuffer.allocate(width * rowsPerChunk * Short.SIZE_BYTES).order(ByteOrder.LITTLE_ENDIAN)
        FileOutputStream(file).use { stream ->
            var row = 0
            while (row < rows) {
                val chunkRows = minOf(rowsPerChunk, rows - row)
                chunk.clear()
                chunk.asShortBuffer().put(pixels, row * width, chunkRows * width)
                stream.write(chunk.array(), 0, chunkRows * width * Short.SIZE_BYTES)
                row += chunkRows
            }
        }
    }

    private const val ROWS_PER_WRITE_CHUNK = 64
}
