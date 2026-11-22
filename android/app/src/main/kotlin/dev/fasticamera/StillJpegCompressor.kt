package dev.fasticamera

import android.graphics.Bitmap
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer

/**
 * Сжатие JPEG платформой (Rust: `android-shell::android::still_jpeg`, трейт `JpegCompressor`;
 * решение 142: `Bitmap.compress`). Только вызовы API платформы.
 */
class StillJpegCompressor {

    /**
     * RGBA8 sRGB построчно → байты JPEG без Exif.
     *
     * [pixels] — direct `ByteBuffer` поверх памяти Rust, живёт только на время этого вызова:
     * ссылку на него нельзя сохранять. Пиксели копируются в `Bitmap`, дальше Rust память не нужна.
     */
    fun compress(pixels: ByteBuffer, width: Int, height: Int, quality: Int): ByteArray {
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        try {
            // Порядок байт ARGB_8888 в памяти — R, G, B, A: ровно то, что отдаёт Rust.
            pixels.rewind()
            bitmap.copyPixelsFromBuffer(pixels)
            val output = ByteArrayOutputStream()
            check(bitmap.compress(Bitmap.CompressFormat.JPEG, quality, output)) { "Bitmap.compress вернул false" }
            return output.toByteArray()
        } finally {
            bitmap.recycle()
        }
    }
}
