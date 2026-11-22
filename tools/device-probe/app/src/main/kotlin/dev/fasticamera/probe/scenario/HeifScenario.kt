package dev.fasticamera.probe.scenario

import dev.fasticamera.probe.heif.ChartDecodeMeasure
import dev.fasticamera.probe.heif.ColorChart
import dev.fasticamera.probe.heif.EncodeOutcome
import dev.fasticamera.probe.heif.HeifEncodePaths
import dev.fasticamera.probe.heif.SystemDecoder
import dev.fasticamera.probe.heif.YuvMatrix
import dev.fasticamera.probe.heif.YuvRange
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Сессия 14: проба цветопередачи HEIF/JPEG. Рисуем миру с известным sRGB, кодируем шестью
 * путями, декодируем системными декодерами и меряем отклонение патчей. Файлы остаются в
 * `files/heif/` — ПК раскодирует их libheif/Pillow и сверяет независимо от телефона.
 * Параметры: `sizes=4096x3072,1024x768`.
 */
class HeifScenario : ProbeScenario {

    /** Один путь кодирования: имя (идёт в имя файла) + расширение + как закодировать. */
    private class EncodePath(val name: String, val extension: String, val encode: (HeifEncodePaths, File) -> EncodeOutcome)

    override fun run(context: ScenarioContext) {
        val outputDirectory = File(context.appContext.filesDir, OUTPUT_DIRECTORY).apply { mkdirs() }
        for (size in context.args.stringList("sizes", DEFAULT_SIZES)) {
            val (width, height) = size.split('x').map { it.toInt() }
            val chart = ColorChart(width, height)
            val encoders = HeifEncodePaths(chart, outputDirectory)
            for (path in encodePaths()) {
                val file = File(outputDirectory, "${width}x${height}_${path.name}.${path.extension}")
                runPath(context, chart, encoders, path, file)
            }
        }
        context.log.event("chart_patches", JSONObject().put("patches", patchTable(ColorChart(1024, 768))))
    }

    private fun encodePaths(): List<EncodePath> = listOf(
        EncodePath("a_bitmap_grid", "heic") { enc, file -> enc.heifBitmap(file, gridEnabled = true) },
        EncodePath("a_bitmap_nogrid", "heic") { enc, file -> enc.heifBitmap(file, gridEnabled = false) },
        EncodePath("b_yuv_bt601_full", "heic") { enc, file -> enc.heifYuvBuffer(file, YuvMatrix.BT601, YuvRange.FULL, gridEnabled = true) },
        EncodePath("c_yuv_bt601_limited", "heic") { enc, file -> enc.heifYuvBuffer(file, YuvMatrix.BT601, YuvRange.LIMITED, gridEnabled = true) },
        EncodePath("d_yuv_bt709_limited", "heic") { enc, file -> enc.heifYuvBuffer(file, YuvMatrix.BT709, YuvRange.LIMITED, gridEnabled = true) },
        EncodePath("e_jpeg_yuvimage", "jpg") { enc, file -> enc.jpegYuvImage(file) },
        EncodePath("f_jpeg_bitmap", "jpg") { enc, file -> enc.jpegBitmap(file) },
    )

    /** Сбой одного пути не должен срывать остальные: ошибка логируется с именем пути. */
    private fun runPath(context: ScenarioContext, chart: ColorChart, encoders: HeifEncodePaths, path: EncodePath, file: File) {
        val label = "${chart.width}x${chart.height}_${path.name}"
        val outcome = try {
            path.encode(encoders, file)
        } catch (error: Exception) {
            context.log.error("encode_$label", error)
            file.delete() // файл создали мы сами; битый недописанный не нужен
            return
        }
        val decoded = JSONObject()
        for (decoder in SystemDecoder.entries) {
            decoded.put(decoder.wireName, decodeAndMeasure(context, chart, file, decoder, label))
        }
        context.log.result(
            label,
            JSONObject().put("path", path.name).put("size", "${chart.width}x${chart.height}")
                .put("file", file.name).put("file_bytes", file.length())
                .put("encode_ms", outcome.encodeMillis).put("convert_cpu_ms", outcome.convertMillis)
                .put("decoded", decoded),
        )
    }

    private fun decodeAndMeasure(context: ScenarioContext, chart: ColorChart, file: File, decoder: SystemDecoder, label: String): JSONObject =
        try {
            val bitmap = ChartDecodeMeasure.decode(file, decoder)
            try {
                ChartDecodeMeasure.measure(bitmap, chart)
            } finally {
                bitmap.recycle()
            }
        } catch (error: Exception) {
            context.log.error("decode_${decoder.wireName}_$label", error)
            JSONObject().put("error", "${error.javaClass.simpleName}: ${error.message}")
        }

    /** Таблица «имя → прямоугольник → эталонный sRGB» (для маленькой миры; большая — масштаб ×4). */
    private fun patchTable(chart: ColorChart): JSONArray = JSONArray().also { table ->
        chart.patches.forEach { patch ->
            table.put(
                JSONObject().put("name", patch.name).put("group", patch.group.name)
                    .put("rect", JSONArray(listOf(patch.rect.left, patch.rect.top, patch.rect.width, patch.rect.height)))
                    .put("srgb", JSONArray(listOf(patch.red, patch.green, patch.blue))),
            )
        }
    }

    private companion object {
        const val OUTPUT_DIRECTORY = "heif"
        val DEFAULT_SIZES = listOf("4096x3072", "1024x768")
    }
}
