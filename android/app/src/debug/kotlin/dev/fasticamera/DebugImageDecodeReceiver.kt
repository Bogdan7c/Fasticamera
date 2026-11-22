package dev.fasticamera

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.ImageDecoder
import android.net.Uri
import org.json.JSONObject

/**
 * Только debug: декодирует файл из MediaStore СИСТЕМНЫМ `ImageDecoder` и возвращает результат скрипту ПК.
 *
 * Нужен smoke-сценарию `capture` (проверка «HEIF, записанный нами, открывается системой»): `adb shell am broadcast
 * -n <пакет>/dev.fasticamera.DebugImageDecodeReceiver --es uri content://media/...` печатает
 * `Broadcast completed: result=0, data="<JSON>"`. JSON: `status` = `decoded` | `failed`, у `decoded` — `width`,
 * `height`, `color_space`, `bitmap_config`; у `failed` — `error`.
 *
 * Расшифровка 12-мегапиксельного снимка занимает сотни миллисекунд и не должна идти в главном потоке, поэтому
 * приёмник берёт `goAsync()` и работает в своём потоке; ответ уходит через `PendingResult.setResultData`.
 * Адрес проверяется: только `content://media/...` — приёмник открыт для adb и не должен читать чужое.
 */
class DebugImageDecodeReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        val uriText = intent.getStringExtra(EXTRA_URI)
        val appContext = context.applicationContext
        Thread({
            val answer = try {
                decode(appContext, uriText)
            } catch (error: Throwable) {
                // Любой сбой — явный ответ `failed`, а не тишина: скрипт различит «не открылось» и «приёмника нет».
                failure("${error.javaClass.simpleName}: ${error.message}")
            }
            pending.setResultData(answer.toString())
            pending.finish()
        }, "fasticamera-debug-decode").start()
    }

    private fun decode(context: Context, uriText: String?): JSONObject {
        if (uriText.isNullOrEmpty()) return failure("нет параметра uri")
        val uri = Uri.parse(uriText)
        if (uri.scheme != "content" || uri.authority != MEDIA_AUTHORITY) {
            return failure("адрес не из MediaStore: $uriText")
        }
        val source = ImageDecoder.createSource(context.contentResolver, uri)
        val bitmap = ImageDecoder.decodeBitmap(source)
        try {
            return JSONObject()
                .put("status", "decoded")
                .put("width", bitmap.width)
                .put("height", bitmap.height)
                .put("color_space", bitmap.colorSpace?.name ?: "unknown")
                .put("bitmap_config", bitmap.config?.name ?: "unknown")
        } finally {
            bitmap.recycle()
        }
    }

    private fun failure(reason: String): JSONObject =
        JSONObject().put("status", "failed").put("error", reason)

    companion object {
        /** Параметр адреса файла (контракт со скриптом `scripts/smoke_checks_capture.py`). */
        const val EXTRA_URI = "uri"

        /** Единственный разрешённый источник адресов. */
        private const val MEDIA_AUTHORITY = "media"
    }
}
