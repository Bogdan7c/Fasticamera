package dev.fasticamera.probe.heif

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Какой системный декодер использовать для измерения. */
enum class SystemDecoder(val wireName: String) { IMAGE_DECODER("ImageDecoder"), BITMAP_FACTORY("BitmapFactory") }

/**
 * Декодирует файл на телефоне и меряет среднее sRGB центральной части каждого патча.
 * Берём цвета через Bitmap.getPixels: он отдаёт sRGB-ints, при необходимости конвертируя из цветового пространства битмапа,
 * то есть ровно то, что увидит приложение, рисующее битмап.
 */
object ChartDecodeMeasure {

    fun decode(file: File, decoder: SystemDecoder): Bitmap = when (decoder) {
        SystemDecoder.IMAGE_DECODER -> ImageDecoder.decodeBitmap(ImageDecoder.createSource(file)) { imageDecoder, _, _ ->
            imageDecoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        SystemDecoder.BITMAP_FACTORY -> {
            val options = BitmapFactory.Options().apply { inPreferredConfig = Bitmap.Config.ARGB_8888 }
            checkNotNull(BitmapFactory.decodeFile(file.absolutePath, options)) { "BitmapFactory вернул null" }
        }
    }

    /** JSON: размер, конфиг, цветовое пространство, патчи (среднее rgb и разница с эталоном), макс. |Δ| по группам. */
    fun measure(bitmap: Bitmap, chart: ColorChart): JSONObject {
        val patchesJson = JSONArray()
        val worstByGroup = PatchGroup.entries.associateWith { IntArray(3) }
        for (patch in chart.patches) {
            val region = patch.rect.central()
            val pixels = IntArray(region.width * region.height)
            bitmap.getPixels(pixels, 0, region.width, region.left, region.top, region.width, region.height)
            val sums = DoubleArray(3)
            for (pixel in pixels) {
                sums[0] += (pixel shr 16) and 0xFF
                sums[1] += (pixel shr 8) and 0xFF
                sums[2] += pixel and 0xFF
            }
            val means = DoubleArray(3) { sums[it] / pixels.size }
            val reference = intArrayOf(patch.red, patch.green, patch.blue)
            val deltas = DoubleArray(3) { means[it] - reference[it] }
            for (channel in 0..2) {
                val worst = worstByGroup.getValue(patch.group)
                worst[channel] = maxOf(worst[channel], Math.round(Math.abs(deltas[channel])).toInt())
            }
            patchesJson.put(
                JSONObject().put("name", patch.name)
                    .put("mean", roundedArray(means)).put("delta", roundedArray(deltas)),
            )
        }
        val worstJson = JSONObject()
        worstByGroup.forEach { (group, worst) -> worstJson.put(group.name.lowercase(), JSONArray(worst.toList())) }
        return JSONObject()
            .put("width", bitmap.width).put("height", bitmap.height)
            .put("config", bitmap.config?.name ?: "null")
            .put("color_space", bitmap.colorSpace?.name ?: "null")
            .put("max_abs_delta_rgb", worstJson)
            .put("patches", patchesJson)
    }

    private fun roundedArray(values: DoubleArray): JSONArray =
        JSONArray(values.map { Math.round(it * 100) / 100.0 })
}
