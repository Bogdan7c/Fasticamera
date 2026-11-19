package dev.fasticamera

import android.content.Intent
import org.json.JSONArray
import org.json.JSONObject

/**
 * Конфиг нативной части. Значения по умолчанию — из BuildConfig (то есть из
 * android/gradle.properties), фильтр логов можно переопределить intent-параметром на один запуск.
 * Проверяет значения Rust (`RuntimeConfig::from_json`): здесь только сборка JSON.
 *
 * `stageBudgetRows` — строки таблицы сценария `viewfinder-stage-budget` на один запуск
 * (intent-параметр `stages`, например `levels,tone,whole_viewfinder`). Неизвестное имя Rust
 * отвергает при initialize — опечатка видна сразу, а не после шести минут замера.
 *
 * `abWindowMs` — длина одного окна A/B замера тока на один запуск (intent-параметр `ab_window_ms`,
 * секция `camera.scenarios.power.ab.window_ms`). Короткое окно (например 15000) годится, чтобы
 * быстро снять миллисекунды GPU всех стадий; ток в таком окне Rust честно пометит «не различим».
 * Границы (окно длиннее прогрева и периода выборки) проверяет Rust.
 *
 * `exposureController` — какой автоэкспозицией управлять на один запуск (intent-параметр
 * `exposure_controller`, поле `camera.exposure_controller`: `own` или `vendor`). Kotlin значение не
 * разбирает, а передаёт как есть; неизвестное имя Rust отвергает при initialize.
 */
data class NativeConfig(
    val buildType: String,
    val logFilter: String,
    val stageBudgetRows: List<String>? = null,
    val abWindowMs: Long? = null,
    val exposureController: String? = null,
) {
    fun toJson(): String {
        val json = JSONObject()
            .put("build_type", buildType)
            .put("log_filter", logFilter)
        // Частичные секции: остальные поля camera.* Rust берёт по умолчанию.
        val scenarios = JSONObject()
        stageBudgetRows?.let { rows ->
            val stageBudget = JSONObject().put("rows", JSONArray(rows))
            scenarios.put("viewfinder", JSONObject().put("stage_budget", stageBudget))
        }
        abWindowMs?.let { windowMs ->
            val ab = JSONObject().put("window_ms", windowMs)
            scenarios.put("power", JSONObject().put("ab", ab))
        }
        val camera = JSONObject()
        if (scenarios.length() > 0) {
            camera.put("scenarios", scenarios)
        }
        exposureController?.let { name -> camera.put("exposure_controller", name) }
        if (camera.length() > 0) {
            json.put("camera", camera)
        }
        return json.toString()
    }

    companion object {
        /** Intent-параметр с фильтром логов, например `info,fasticamera::scenario=trace`. */
        const val EXTRA_LOG_FILTER = "log_filter"

        /** Intent-параметр со строками таблицы `viewfinder-stage-budget` через запятую. */
        const val EXTRA_STAGE_BUDGET_ROWS = "stages"

        /** Intent-параметр с длиной окна A/B замера тока, мс (строка с целым числом). */
        const val EXTRA_AB_WINDOW_MS = "ab_window_ms"

        /** Intent-параметр с именем автоэкспозиции: `own` или `vendor` (проверяет Rust). */
        const val EXTRA_EXPOSURE_CONTROLLER = "exposure_controller"

        fun fromIntent(intent: Intent): NativeConfig = NativeConfig(
            buildType = BuildConfig.BUILD_TYPE,
            logFilter = intent.getStringExtra(EXTRA_LOG_FILTER) ?: BuildConfig.LOG_FILTER,
            stageBudgetRows = intent.getStringExtra(EXTRA_STAGE_BUDGET_ROWS)
                ?.split(',')
                ?.map { it.trim() }
                ?.filter { it.isNotEmpty() },
            // Не число — параметр не применяется, а initialize идёт с окном по умолчанию: скрипт
            // device-smoke.sh пропускает сюда только цифры, так что это лишь защита от ручного вызова.
            abWindowMs = intent.getStringExtra(EXTRA_AB_WINDOW_MS)?.toLongOrNull(),
            // Нет параметра — null, поле в JSON не попадает, Rust берёт умолчание (own).
            exposureController = intent.getStringExtra(EXTRA_EXPOSURE_CONTROLLER),
        )
    }
}
