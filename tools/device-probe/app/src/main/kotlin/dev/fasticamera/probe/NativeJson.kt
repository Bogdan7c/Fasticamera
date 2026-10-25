package dev.fasticamera.probe

import org.json.JSONException
import org.json.JSONObject

/**
 * Разбор JSON-ответа нативной функции с явными исходами отказа.
 *
 * Нативная библиотека может не загрузиться или вернуть битый JSON — это фиксируется
 * записью `{"ok": false, "stage": ...}`, а не роняет сценарий: отказ тоже результат разведки.
 */
object NativeJson {
    fun call(nativeCall: () -> String): JSONObject = try {
        JSONObject(nativeCall())
    } catch (error: LinkageError) {
        // LinkageError покрывает и UnsatisfiedLinkError при первой загрузке, и NoClassDefFoundError при повторной.
        JSONObject().put("ok", false).put("stage", "load_library").put("error", error.message)
    } catch (error: JSONException) {
        JSONObject().put("ok", false).put("stage", "parse_native_json").put("error", error.message)
    }
}
