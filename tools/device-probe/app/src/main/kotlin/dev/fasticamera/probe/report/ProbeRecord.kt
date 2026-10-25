package dev.fasticamera.probe.report

import org.json.JSONObject

/**
 * Вид записи зонда. ПК-скрипт (pc/probe.py) группирует записи по виду:
 * - [RESULT] — итог эксперимента (строка таблицы отчёта);
 * - [EVENT] — промежуточный факт (камера открылась, сессия настроена);
 * - [ERROR] — явная ошибка с этапом, на котором она случилась;
 * - [DONE] — сценарий завершён; после неё скрипт перестаёт ждать.
 */
enum class RecordKind(val wireName: String) {
    RESULT("result"),
    EVENT("event"),
    ERROR("error"),
    DONE("done"),
}

/**
 * Одна логическая запись зонда. Конверт одинаковый для всех сценариев,
 * чтобы ПК-скрипт не знал внутренностей конкретного сценария.
 */
data class ProbeRecord(
    val runId: String,
    val scenario: String,
    val kind: RecordKind,
    val name: String,
    val payload: JSONObject,
) {
    /** Сериализация в одну JSON-строку; версия формата нужна, чтобы скрипт отличал старые логи. */
    fun toJsonLine(): String = JSONObject()
        .put("probe", FORMAT_VERSION)
        .put("run", runId)
        .put("scenario", scenario)
        .put("kind", kind.wireName)
        .put("name", name)
        .put("data", payload)
        .toString()

    companion object {
        const val FORMAT_VERSION = 1
    }
}
