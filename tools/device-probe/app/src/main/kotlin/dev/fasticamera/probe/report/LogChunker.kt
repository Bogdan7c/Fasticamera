package dev.fasticamera.probe.report

import org.json.JSONObject

/**
 * Режет длинную JSON-строку на куски, которые влезают в одну запись logcat.
 *
 * Зачем: logcat обрезает запись примерно на 4 КБ, а каталог камеры весит десятки КБ.
 * Обрезанный JSON молча потерял бы данные, поэтому длинная запись уходит частями
 * в конверте `{"chunk_of": id, "part": i, "parts": n, "text": "..."}`, а ПК-скрипт
 * склеивает их обратно по `chunk_of` и проверяет, что пришли все части.
 */
class LogChunker(private val maxCharsPerLine: Int) {

    init {
        require(maxCharsPerLine > MIN_CHUNK_TEXT) {
            "maxCharsPerLine=$maxCharsPerLine слишком мал: конверт куска сам занимает место"
        }
    }

    /**
     * Возвращает строки для печати. Короткая запись проходит как есть,
     * длинная — набором конвертов с одинаковым [chunkId].
     */
    fun split(line: String, chunkId: String): List<String> {
        if (line.length <= maxCharsPerLine) {
            return listOf(line)
        }
        // Оставляем запас под сам конверт и экранирование кавычек в тексте куска.
        val textBudget = (maxCharsPerLine / ESCAPE_HEADROOM_DIVISOR).coerceAtLeast(MIN_CHUNK_TEXT)
        val pieces = line.chunked(textBudget)
        return pieces.mapIndexed { index, piece ->
            JSONObject()
                .put("chunk_of", chunkId)
                .put("part", index)
                .put("parts", pieces.size)
                .put("text", piece)
                .toString()
        }
    }

    private companion object {
        /** Минимальный полезный размер текста в куске. */
        const val MIN_CHUNK_TEXT = 256

        /**
         * JSON-экранирование может раздуть текст (каждая `"` превращается в `\"`).
         * Делим бюджет пополам: даже если все символы экранируются, строка влезет.
         */
        const val ESCAPE_HEADROOM_DIVISOR = 2
    }
}
