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
 * `enduranceMs` — длина прогона `viewfinder-endurance` на один запуск (intent-параметр `endurance_ms`,
 * секция `camera.scenarios.power.endurance.duration_ms`). Короткий прогон (например 180000) годится для
 * A/B замера тока «наш AE / вендорский» перезапусками; нижнюю границу (шаг снимков) проверяет Rust.
 *
 * `exposureController` — какой автоэкспозицией управлять на один запуск (intent-параметр
 * `exposure_controller`, поле `camera.exposure_controller`: `own` или `vendor`). Kotlin значение не
 * разбирает, а передаёт как есть; неизвестное имя Rust отвергает при initialize.
 *
 * `toneContrast` / `toneDisplayMiddleGray` — ручки тона видоискателя на один запуск (intent-параметры
 * `tone_contrast` и `tone_display_middle_gray`, секция `render.viewfinder.tone`, сессия 12b): подбор контраста
 * и средних тонов глазами владельца без пересборки. Число уходит в JSON числом, а нечисловая строка — как
 * есть строкой: Rust отвергает её при initialize громко, а не берёт молча умолчание. Пределы (контраст > 0,
 * середина в 0…1, кривая строится при подтяжке AE) проверяет Rust.
 *
 * `untrustedOpticalAxisCameras` — ID камер, чьей оптической оси из калибровки HAL верить нельзя (профиль
 * телефона из `fasticamera.zoom.untrustedOpticalAxisCameras`, поле `camera.engine.zoom.untrusted_optical_axis_cameras`).
 * Пустой список в JSON не попадает: Rust берёт пустое умолчание. ID проверяет Rust.
 */
data class NativeConfig(
    val buildType: String,
    val logFilter: String,
    val stageBudgetRows: List<String>? = null,
    val abWindowMs: Long? = null,
    val enduranceMs: Long? = null,
    val exposureController: String? = null,
    val toneContrast: String? = null,
    val toneDisplayMiddleGray: String? = null,
    val untrustedOpticalAxisCameras: List<String> = emptyList(),
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
        // Обе настройки живут в одной секции `power`: собираем её целиком, чтобы одна не затёрла другую.
        val power = JSONObject()
        abWindowMs?.let { windowMs -> power.put("ab", JSONObject().put("window_ms", windowMs)) }
        enduranceMs?.let { durationMs -> power.put("endurance", JSONObject().put("duration_ms", durationMs)) }
        if (power.length() > 0) {
            scenarios.put("power", power)
        }
        val camera = JSONObject()
        if (scenarios.length() > 0) {
            camera.put("scenarios", scenarios)
        }
        exposureController?.let { name -> camera.put("exposure_controller", name) }
        if (untrustedOpticalAxisCameras.isNotEmpty()) {
            val zoom = JSONObject().put("untrusted_optical_axis_cameras", JSONArray(untrustedOpticalAxisCameras))
            camera.put("engine", JSONObject().put("zoom", zoom))
        }
        if (camera.length() > 0) {
            json.put("camera", camera)
        }
        val tone = JSONObject()
        toneContrast?.let { value -> tone.put("contrast", numberOrRaw(value)) }
        toneDisplayMiddleGray?.let { value -> tone.put("display_middle_gray", numberOrRaw(value)) }
        if (tone.length() > 0) {
            // Частичная секция: остальные поля render.* Rust берёт по умолчанию.
            json.put("render", JSONObject().put("viewfinder", JSONObject().put("tone", tone)))
        }
        return json.toString()
    }

    /** Число — числом; иначе исходная строка, чтобы Rust отверг её с понятной ошибкой. */
    private fun numberOrRaw(value: String): Any = value.trim().toDoubleOrNull() ?: value

    companion object {
        /** Intent-параметр с фильтром логов, например `info,fasticamera::scenario=trace`. */
        const val EXTRA_LOG_FILTER = "log_filter"

        /** Intent-параметр со строками таблицы `viewfinder-stage-budget` через запятую. */
        const val EXTRA_STAGE_BUDGET_ROWS = "stages"

        /** Intent-параметр с длиной окна A/B замера тока, мс (строка с целым числом). */
        const val EXTRA_AB_WINDOW_MS = "ab_window_ms"

        /** Intent-параметр с длиной прогона `viewfinder-endurance`, мс (строка с целым числом). */
        const val EXTRA_ENDURANCE_MS = "endurance_ms"

        /** Intent-параметр с именем автоэкспозиции: `own` или `vendor` (проверяет Rust). */
        const val EXTRA_EXPOSURE_CONTROLLER = "exposure_controller"

        /** Intent-параметр с контрастом тона видоискателя (строка с числом, проверяет Rust). */
        const val EXTRA_TONE_CONTRAST = "tone_contrast"

        /** Intent-параметр со средним серым экрана, линейная яркость 0…1 (строка с числом, проверяет Rust). */
        const val EXTRA_TONE_DISPLAY_MIDDLE_GRAY = "tone_display_middle_gray"

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
            // Так же, как окно A/B: не число — умолчание конфига (скрипт пропускает только цифры).
            enduranceMs = intent.getStringExtra(EXTRA_ENDURANCE_MS)?.toLongOrNull(),
            // Нет параметра — null, поле в JSON не попадает, Rust берёт умолчание (own).
            exposureController = intent.getStringExtra(EXTRA_EXPOSURE_CONTROLLER),
            // Нет параметра — null, секция тона в JSON не попадает, Rust берёт умолчание.
            toneContrast = intent.getStringExtra(EXTRA_TONE_CONTRAST),
            toneDisplayMiddleGray = intent.getStringExtra(EXTRA_TONE_DISPLAY_MIDDLE_GRAY),
            // Профиль телефона из gradle.properties: пустая строка — список пуст.
            untrustedOpticalAxisCameras = BuildConfig.UNTRUSTED_OPTICAL_AXIS_CAMERAS
                .split(',')
                .map { it.trim() }
                .filter { it.isNotEmpty() },
        )
    }
}
