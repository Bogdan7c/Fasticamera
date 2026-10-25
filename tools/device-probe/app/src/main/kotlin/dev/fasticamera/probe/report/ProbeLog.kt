package dev.fasticamera.probe.report

import org.json.JSONObject

/**
 * Единственная точка вывода результатов зонда. Сценарии формируют данные,
 * а сериализация и доставка (файл + logcat) живут только здесь
 * (правило AGENTS.md «не смешивать вывод и логику»).
 */
class ProbeLog(
    private val runId: String,
    private val scenario: String,
    private val sinks: List<RecordSink>,
) {

    fun result(name: String, payload: JSONObject) = emit(RecordKind.RESULT, name, payload)

    fun event(name: String, payload: JSONObject = JSONObject()) = emit(RecordKind.EVENT, name, payload)

    /** Ошибка всегда несёт этап ([stage]) и класс исключения, чтобы её нельзя было спутать с «нет данных». */
    fun error(stage: String, error: Throwable) = emit(
        RecordKind.ERROR,
        stage,
        JSONObject()
            .put("exception", error.javaClass.name)
            .put("message", error.message ?: "")
            .put("cause", error.cause?.let { "${it.javaClass.name}: ${it.message}" } ?: JSONObject.NULL),
    )

    fun error(stage: String, message: String) = emit(
        RecordKind.ERROR,
        stage,
        JSONObject().put("message", message),
    )

    fun done(payload: JSONObject = JSONObject()) = emit(RecordKind.DONE, "done", payload)

    private fun emit(kind: RecordKind, name: String, payload: JSONObject) {
        val line = ProbeRecord(runId, scenario, kind, name, payload).toJsonLine()
        sinks.forEach { sink -> sink.write(line) }
    }

    companion object {
        const val TAG = "FasticameraProbe"

        /** Запас ниже предела logcat (max payload 4068 байт на этом телефоне, с учётом многобайтных символов). */
        const val MAX_LOGCAT_CHARS = 3000

        /** Каталог файлов прогонов внутри filesDir приложения; ПК читает его через `run-as`. */
        const val RUNS_DIRECTORY = "runs"
    }
}
