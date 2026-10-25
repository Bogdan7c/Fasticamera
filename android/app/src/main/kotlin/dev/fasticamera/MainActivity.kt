package dev.fasticamera

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.view.WindowInsets
import android.widget.TextView
import org.json.JSONObject

/**
 * Каркас экрана (сессия 01): инициализирует нативную часть и, если в intent есть `scenario`,
 * запускает smoke-сценарий. Итог показывается на экране; строку `FASTICAMERA_RESULT` в logcat
 * пишет Rust. Логики здесь нет — только маршрутизация (README плана, «Владельцы состояния»).
 */
class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val statusView = TextView(this)
        // targetSdk 36 рисует окно под системными панелями (edge-to-edge обязателен),
        // поэтому отступ текста = панели + поле, иначе текст уезжает под статус-бар.
        statusView.setOnApplyWindowInsetsListener { view, insets ->
            val bars = insets.getInsets(WindowInsets.Type.systemBars())
            view.setPadding(
                bars.left + STATUS_PADDING_PX,
                bars.top + STATUS_PADDING_PX,
                bars.right + STATUS_PADDING_PX,
                bars.bottom + STATUS_PADDING_PX,
            )
            insets
        }
        setContentView(statusView)

        val initReport = NativeBridge.initialize(NativeConfig.fromIntent(intent).toJson())
        logIfInitFailed(initReport)

        val scenario = intent.getStringExtra(EXTRA_SCENARIO)
        statusView.text = if (scenario != null) NativeBridge.runScenario(scenario) else initReport
    }

    /**
     * Если initialize не прошёл, нативное логирование может быть не установлено, и отказ
     * не дошёл бы до logcat. Поэтому его дублирует Kotlin.
     */
    private fun logIfInitFailed(initReport: String) {
        val status = JSONObject(initReport).optString("status")
        if (status == "failed") {
            Log.e(LOG_TAG, "native initialize failed: $initReport")
        } else {
            Log.i(LOG_TAG, "native initialize: $initReport")
        }
    }

    companion object {
        /** Intent-параметр с именем smoke-сценария (`scripts/device-smoke.sh`). */
        const val EXTRA_SCENARIO = "scenario"

        /** Тот же тег, что у нативных логов: скрипты фильтруют logcat по нему. */
        private const val LOG_TAG = "Fasticamera"

        private const val STATUS_PADDING_PX = 48
    }
}
