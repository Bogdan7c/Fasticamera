package dev.fasticamera

import android.content.Intent
import android.os.Build
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
 * `stageBudgetSurface` — место замера `viewfinder-stage-budget` на один запуск (intent-параметр `surface`, поле
 * `camera.scenarios.viewfinder.stage_budget.surface`, сессия 16a): `bare_viewfinder` (умолчание), `photo_screen` или
 * `calibrate_screen`. Kotlin значение не разбирает, а передаёт строкой как есть; неизвестное имя и строку таблицы, которая
 * на этом месте не мерится, Rust отвергает при initialize.
 *
 * `abPairs` — число пар A/B замера тока на строку на один запуск (intent-параметр `ab_pairs`, поле
 * `camera.scenarios.power.ab.pairs`, 16a). Границы проверяет Rust.
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
 * `whiteMemory` — режим памяти белой на один запуск (intent-параметр `white_memory`, поле `camera.white_memory.mode`:
 * `file` или `none`). `scripts/device-smoke.sh` ВСЕГДА шлёт `none`: прогон не читает белую владельца и не портит её.
 * `scripts/device-deploy.sh` параметр не шлёт, и память работает как в продукте (файл в папке приложения). Kotlin значение не
 * разбирает, а передаёт как есть; неизвестный режим Rust отвергает при initialize.
 *
 * `whiteResumeRule` — правило возобновления белого, выбранное человеком на один запуск (intent-параметр
 * `white_resume_rule`, поле `camera.white_resume_choice`: `take_from_source` или `keep_last`). Приложение шлёт его
 * движку ДО первого возобновления как выбор человека, и он запоминается в файле белой. Без параметра действует то, что
 * запомнено, а без памяти — умолчание конфига (`take_from_source`). `scripts/device-smoke.sh` параметр не шлёт.
 * Kotlin значение не разбирает, а передаёт как есть; неизвестное значение Rust отвергает при initialize.
 *
 * `whiteFollowMs` — скорость слежения за белым телефона на один запуск (intent-параметр `white_follow_ms`, поле
 * `camera.white_follow_speed_choice`: целое число миллисекунд, 0…10000). Приложение шлёт её движку ДО первого
 * возобновления как выбор человека, и она запоминается в файле белой. Число уходит в JSON числом, а нечисловая строка —
 * как есть строкой: Rust отвергает её при initialize громко (ошибка называет поле), а не берёт молча умолчание.
 * Режим слежения этим параметром не включается: его включает кнопка «Auto».
 *
 * `toneContrast` / `toneDisplayMiddleGray` — ручки тона видоискателя на один запуск (intent-параметры
 * `tone_contrast` и `tone_display_middle_gray`, секция `render.viewfinder.tone`, сессия 12b): подбор контраста
 * и средних тонов глазами владельца без пересборки. Число уходит в JSON числом, а нечисловая строка — как
 * есть строкой: Rust отвергает её при initialize громко, а не берёт молча умолчание. Пределы (контраст > 0,
 * середина в 0…1, кривая строится при подтяжке AE) проверяет Rust.
 *
 * `toneBase` — модель основы тона на один запуск (intent-параметр `tone_base`, поле `render.viewfinder.tone.base`,
 * сессия 14a, решение 182): `agx`, `aces`, `filmic` или `legacy`. Kotlin значение не разбирает, а передаёт строкой как
 * есть; неизвестное имя и модель, которой ещё нет на GPU, Rust отвергает при initialize громко (ошибка называет поле
 * `viewfinder.tone`), а не берёт молча умолчание.
 *
 * `unchangedFrame` — политика пропуска тиков без изменений на один запуск (intent-параметр `unchanged_frame`, поле
 * `render.unchanged_frame.policy`, решение 214, сессия 16a): `skip` (= `skip_when_unchanged`) или `always` (=
 * `always_redraw`, умолчание продукта до замера на телефоне). Kotlin значение не разбирает, а передаёт строкой как есть;
 * неверное значение Rust отвергает при initialize громко, ошибка называет поле `unchanged_frame.policy`.
 *
 * `saturationWorkloads` / `saturationMeasureMs` / `saturationWarmupMs` — список работ, окно замера и прогрев сценария
 * `gpu-saturation-bench` на один запуск (intent-параметры `saturation_workloads`, `saturation_measure_ms`,
 * `saturation_warmup_ms`, секция `camera.scenarios.saturation`, 16a P4b). Имена работ Kotlin не разбирает, а передаёт
 * как есть: неизвестное имя и неверные числа Rust отвергает при initialize.
 *
 * `untrustedOpticalAxisCameras` — ID камер, чьей оптической оси из калибровки HAL верить нельзя (профиль
 * телефона из `fasticamera.zoom.untrustedOpticalAxisCameras`, поле `camera.engine.zoom.untrusted_optical_axis_cameras`).
 * Пустой список в JSON не попадает: Rust берёт пустое умолчание. ID проверяет Rust.
 */
data class NativeConfig(
    val buildType: String,
    val logFilter: String,
    val stageBudgetRows: List<String>? = null,
    val stageBudgetSurface: String? = null,
    val abWindowMs: Long? = null,
    val abPairs: Long? = null,
    val enduranceMs: Long? = null,
    val saturationWorkloads: List<String>? = null,
    val saturationMeasureMs: Long? = null,
    val saturationWarmupMs: Long? = null,
    val exposureController: String? = null,
    val whiteMemory: String? = null,
    val whiteResumeRule: String? = null,
    val whiteFollowMs: String? = null,
    val toneContrast: String? = null,
    val toneDisplayMiddleGray: String? = null,
    val toneBase: String? = null,
    val unchangedFrame: String? = null,
    val untrustedOpticalAxisCameras: List<String> = emptyList(),
) {
    fun toJson(): String {
        val json = JSONObject()
            .put("build_type", buildType)
            .put("log_filter", logFilter)
            // Паспорт телефона для Exif/DNG (`Make`/`Model`): Rust сам ничего не хардкодит (секция `device`).
            .put("device", JSONObject().put("make", Build.MANUFACTURER).put("model", Build.MODEL))
        // Частичные секции: остальные поля camera.* Rust берёт по умолчанию.
        val scenarios = JSONObject()
        // Строки и место замера живут в одной секции `stage_budget`: собираем её целиком, чтобы одно не затёрло другое.
        val stageBudget = JSONObject()
        stageBudgetRows?.let { rows -> stageBudget.put("rows", JSONArray(rows)) }
        stageBudgetSurface?.let { name -> stageBudget.put("surface", name.trim()) }
        if (stageBudget.length() > 0) {
            scenarios.put("viewfinder", JSONObject().put("stage_budget", stageBudget))
        }
        // Настройки живут в одних секциях `power` и `power.ab`: собираем их целиком, чтобы одна не затёрла другую.
        val power = JSONObject()
        val ab = JSONObject()
        abWindowMs?.let { windowMs -> ab.put("window_ms", windowMs) }
        abPairs?.let { pairs -> ab.put("pairs", pairs) }
        if (ab.length() > 0) {
            power.put("ab", ab)
        }
        enduranceMs?.let { durationMs -> power.put("endurance", JSONObject().put("duration_ms", durationMs)) }
        if (power.length() > 0) {
            scenarios.put("power", power)
        }
        // Секция стенда насыщения: список работ и длины окон собираются в один объект.
        val saturation = JSONObject()
        saturationWorkloads?.let { names -> saturation.put("workloads", JSONArray(names)) }
        saturationMeasureMs?.let { measureMs -> saturation.put("measure_ms", measureMs) }
        saturationWarmupMs?.let { warmupMs -> saturation.put("warmup_ms", warmupMs) }
        if (saturation.length() > 0) {
            scenarios.put("saturation", saturation)
        }
        val camera = JSONObject()
        if (scenarios.length() > 0) {
            camera.put("scenarios", scenarios)
        }
        exposureController?.let { name -> camera.put("exposure_controller", name) }
        // Имя режима — строкой как есть: проверяет Rust (неизвестное имя — отказ initialize).
        whiteMemory?.let { mode -> camera.put("white_memory", JSONObject().put("mode", mode.trim())) }
        // Правило возобновления белого — строкой как есть: проверяет Rust (неизвестное значение — отказ initialize).
        whiteResumeRule?.let { rule -> camera.put("white_resume_choice", rule.trim()) }
        // Скорость слежения — целое число; не число — исходная строка, чтобы Rust отверг её с именем поля.
        whiteFollowMs?.let { millis -> camera.put("white_follow_speed_choice", integerOrRaw(millis)) }
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
        // Имя модели — строкой как есть: проверяет Rust (неизвестное имя — отказ initialize).
        toneBase?.let { value -> tone.put("base", value.trim()) }
        // Частичная секция render: остальные поля Rust берёт по умолчанию; тон и политика пропуска живут в одной секции.
        val render = JSONObject()
        if (tone.length() > 0) {
            render.put("viewfinder", JSONObject().put("tone", tone))
        }
        unchangedFrame?.let { value -> render.put("unchanged_frame", JSONObject().put("policy", value.trim())) }
        if (render.length() > 0) {
            json.put("render", render)
        }
        return json.toString()
    }

    /** Число — числом; иначе исходная строка, чтобы Rust отверг её с понятной ошибкой. */
    private fun numberOrRaw(value: String): Any = value.trim().toDoubleOrNull() ?: value

    /** Целое — целым числом; иначе исходная строка (в том числе дробь и отрицательное), чтобы Rust отверг её громко. */
    private fun integerOrRaw(value: String): Any = value.trim().toLongOrNull() ?: value

    companion object {
        /** Intent-параметр с фильтром логов, например `info,fasticamera::scenario=trace`. */
        const val EXTRA_LOG_FILTER = "log_filter"

        /** Intent-параметр со строками таблицы `viewfinder-stage-budget` через запятую. */
        const val EXTRA_STAGE_BUDGET_ROWS = "stages"

        /** Intent-параметр с местом замера `viewfinder-stage-budget` (`bare_viewfinder`, `photo_screen`, `calibrate_screen`). */
        const val EXTRA_STAGE_BUDGET_SURFACE = "surface"

        /** Intent-параметр с длиной окна A/B замера тока, мс (строка с целым числом). */
        const val EXTRA_AB_WINDOW_MS = "ab_window_ms"

        /** Intent-параметр с числом пар A/B замера тока на строку (строка с целым числом). */
        const val EXTRA_AB_PAIRS = "ab_pairs"

        /** Intent-параметр со списком работ `gpu-saturation-bench` через запятую (имена каталога). */
        const val EXTRA_SATURATION_WORKLOADS = "saturation_workloads"

        /** Intent-параметр с окном замера каждой работы `gpu-saturation-bench`, мс (строка с целым числом). */
        const val EXTRA_SATURATION_MEASURE_MS = "saturation_measure_ms"

        /** Intent-параметр с прогревом каждой работы `gpu-saturation-bench`, мс (строка с целым числом). */
        const val EXTRA_SATURATION_WARMUP_MS = "saturation_warmup_ms"

        /** Intent-параметр с длиной прогона `viewfinder-endurance`, мс (строка с целым числом). */
        const val EXTRA_ENDURANCE_MS = "endurance_ms"

        /** Intent-параметр с именем автоэкспозиции: `own` или `vendor` (проверяет Rust). */
        const val EXTRA_EXPOSURE_CONTROLLER = "exposure_controller"

        /** Intent-параметр с режимом памяти белой: `file` или `none` (проверяет Rust). Smoke-прогоны шлют `none`. */
        const val EXTRA_WHITE_MEMORY = "white_memory"

        /** Intent-параметр с правилом возобновления белого: `take_from_source` или `keep_last` (проверяет Rust). */
        const val EXTRA_WHITE_RESUME_RULE = "white_resume_rule"

        /** Intent-параметр со скоростью слежения за белым, мс (строка с целым числом, проверяет Rust). */
        const val EXTRA_WHITE_FOLLOW_MS = "white_follow_ms"

        /** Intent-параметр с контрастом тона видоискателя (строка с числом, проверяет Rust). */
        const val EXTRA_TONE_CONTRAST = "tone_contrast"

        /** Intent-параметр со средним серым экрана, линейная яркость 0…1 (строка с числом, проверяет Rust). */
        const val EXTRA_TONE_DISPLAY_MIDDLE_GRAY = "tone_display_middle_gray"

        /** Intent-параметр с моделью основы тона: `agx`, `aces`, `filmic` или `legacy` (проверяет Rust). */
        const val EXTRA_TONE_BASE = "tone_base"
        /** Intent-параметр с политикой пропуска тиков без изменений: `skip` или `always` (проверяет Rust). */
        const val EXTRA_UNCHANGED_FRAME = "unchanged_frame"

        fun fromIntent(intent: Intent): NativeConfig = NativeConfig(
            buildType = BuildConfig.BUILD_TYPE,
            logFilter = intent.getStringExtra(EXTRA_LOG_FILTER) ?: BuildConfig.LOG_FILTER,
            stageBudgetRows = intent.getStringExtra(EXTRA_STAGE_BUDGET_ROWS)
                ?.split(',')
                ?.map { it.trim() }
                ?.filter { it.isNotEmpty() },
            // Нет параметра — null, место замера не попадает в JSON, Rust берёт `bare_viewfinder`.
            stageBudgetSurface = intent.getStringExtra(EXTRA_STAGE_BUDGET_SURFACE),
            // Не число — параметр не применяется, а initialize идёт с окном по умолчанию: скрипт
            // device-smoke.sh пропускает сюда только цифры, так что это лишь защита от ручного вызова.
            abWindowMs = intent.getStringExtra(EXTRA_AB_WINDOW_MS)?.toLongOrNull(),
            // Так же, как окно A/B: не число — умолчание конфига (скрипт пропускает только цифры).
            abPairs = intent.getStringExtra(EXTRA_AB_PAIRS)?.toLongOrNull(),
            // Так же, как окно A/B: не число — умолчание конфига (скрипт пропускает только цифры).
            enduranceMs = intent.getStringExtra(EXTRA_ENDURANCE_MS)?.toLongOrNull(),
            // Нет параметра — null, поле в JSON не попадает, Rust берёт умолчание (все работы каталога).
            saturationWorkloads = intent.getStringExtra(EXTRA_SATURATION_WORKLOADS)
                ?.split(',')
                ?.map { it.trim() }
                ?.filter { it.isNotEmpty() },
            // Так же, как окно A/B: не число — умолчание конфига (скрипт пропускает только цифры).
            saturationMeasureMs = intent.getStringExtra(EXTRA_SATURATION_MEASURE_MS)?.toLongOrNull(),
            saturationWarmupMs = intent.getStringExtra(EXTRA_SATURATION_WARMUP_MS)?.toLongOrNull(),
            // Нет параметра — null, поле в JSON не попадает, Rust берёт умолчание (own).
            exposureController = intent.getStringExtra(EXTRA_EXPOSURE_CONTROLLER),
            // Нет параметра — null, поле в JSON не попадает, Rust берёт умолчание (file).
            whiteMemory = intent.getStringExtra(EXTRA_WHITE_MEMORY),
            // Нет параметра — null, поле в JSON не попадает: правило решают память и конфиг движка.
            whiteResumeRule = intent.getStringExtra(EXTRA_WHITE_RESUME_RULE),
            // Нет параметра — null, поле в JSON не попадает: скорость решают память и конфиг движка.
            whiteFollowMs = intent.getStringExtra(EXTRA_WHITE_FOLLOW_MS),
            // Нет параметра — null, секция тона в JSON не попадает, Rust берёт умолчание.
            toneContrast = intent.getStringExtra(EXTRA_TONE_CONTRAST),
            toneDisplayMiddleGray = intent.getStringExtra(EXTRA_TONE_DISPLAY_MIDDLE_GRAY),
            toneBase = intent.getStringExtra(EXTRA_TONE_BASE),
            // Нет параметра — null, секция в JSON не попадает, Rust берёт умолчание (always_redraw).
            unchangedFrame = intent.getStringExtra(EXTRA_UNCHANGED_FRAME),
            // Профиль телефона из gradle.properties: пустая строка — список пуст.
            untrustedOpticalAxisCameras = BuildConfig.UNTRUSTED_OPTICAL_AXIS_CAMERAS
                .split(',')
                .map { it.trim() }
                .filter { it.isNotEmpty() },
        )
    }
}
