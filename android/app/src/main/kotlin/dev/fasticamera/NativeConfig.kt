package dev.fasticamera

import android.content.Intent
import org.json.JSONObject

/**
 * Конфиг нативной части. Значения по умолчанию — из BuildConfig (то есть из
 * android/gradle.properties), фильтр логов можно переопределить intent-параметром на один запуск.
 * Проверяет значения Rust (`RuntimeConfig::from_json`): здесь только сборка JSON.
 */
data class NativeConfig(val buildType: String, val logFilter: String) {
    fun toJson(): String = JSONObject()
        .put("build_type", buildType)
        .put("log_filter", logFilter)
        .toString()

    companion object {
        /** Intent-параметр с фильтром логов, например `info,fasticamera::scenario=trace`. */
        const val EXTRA_LOG_FILTER = "log_filter"

        fun fromIntent(intent: Intent): NativeConfig = NativeConfig(
            buildType = BuildConfig.BUILD_TYPE,
            logFilter = intent.getStringExtra(EXTRA_LOG_FILTER) ?: BuildConfig.LOG_FILTER,
        )
    }
}
