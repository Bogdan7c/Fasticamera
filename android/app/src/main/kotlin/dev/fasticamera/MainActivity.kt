package dev.fasticamera

import android.Manifest
import android.app.Activity
import android.graphics.Insets
import android.os.Bundle
import android.util.Log
import android.view.KeyEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.TextView
import org.json.JSONObject

/**
 * Единственный экран: полноэкранный `SurfaceView` (на нём будет рисовать Rust/Vulkan, сессия 03)
 * и текстовая строка статуса для smoke-сценариев. Логики здесь нет — только маршрутизация
 * событий Android в Rust (`ShellBridge`) и вызовов Rust в Android (`ShellPlatform`).
 * Портрет зафиксирован в манифесте: как в камере iOS, поворачиваются иконки, а не раскладка.
 */
class MainActivity : Activity() {
    private lateinit var platform: ShellPlatform

    /** Последние отправленные в Rust зоны системных жестов: повтор тех же чисел не пересылаем. */
    private var lastSystemGestureZones: Insets? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val surfaceView = SurfaceView(this)
        val statusView = TextView(this)
        statusView.setOnApplyWindowInsetsListener { view, insets ->
            // Панели скрыты, но вырез камеры остаётся: текст статуса не должен под него уезжать.
            val safe = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
            view.setPadding(
                safe.left + STATUS_PADDING_PX,
                safe.top + STATUS_PADDING_PX,
                safe.right + STATUS_PADDING_PX,
                safe.bottom + STATUS_PADDING_PX,
            )
            insets
        }
        // Плашка подсказок владельцу (внизу, GONE в покое) поверх видоискателя и строки статуса.
        val promptView = OperatorPromptView(this)
        setContentView(FrameLayout(this).apply {
            addView(surfaceView)
            addView(statusView)
            addView(promptView)
            // Зоны обязательных системных жестов уходят в Rust: свайп от края, вытягивающий скрытую панель
            // навигации, приходит в окно и обычными касаниями, а экран камеры не должен считать его своим.
            setOnApplyWindowInsetsListener { _, insets ->
                forwardSystemGestureZones(insets)
                insets
            }
        })
        // После setContentView: контроллер панелей живёт у DecorView, а её создаёт setContentView.
        enterFullscreen()

        val initReport = NativeBridge.initialize(NativeConfig.fromIntent(intent).toJson(), packageName)
        logIfInitFailed(initReport)

        platform = ShellPlatform(this, promptView)
        ShellBridge.nativeAttach(platform)
        surfaceView.holder.addCallback(SurfaceCallbacks)
        surfaceView.setOnTouchListener { _, event ->
            ShellBridge.nativeTouch(event)
            true
        }

        val scenario = intent.getStringExtra(EXTRA_SCENARIO)
        statusView.text = if (scenario != null) NativeBridge.runScenario(scenario) else initReport
    }

    /**
     * Пересылает зоны системных жестов в Rust при каждом изменении. `getInsetsIgnoringVisibility` даёт зону
     * и при скрытой панели (обычный `getInsets` вернул бы ноль). Окно с `SurfaceView` на весь экран, поэтому
     * толщины от краёв окна — это толщины от краёв surface. Решений здесь нет: только пересылка.
     */
    private fun forwardSystemGestureZones(insets: WindowInsets) {
        val zones = insets.getInsetsIgnoringVisibility(WindowInsets.Type.mandatorySystemGestures())
        if (zones == lastSystemGestureZones) return
        lastSystemGestureZones = zones
        ShellBridge.nativeSystemGestureZones(zones.left, zones.top, zones.right, zones.bottom)
    }

    /**
     * Вернувшись в фокус (после шторки уведомлений, другого приложения), панели снова скрываем: иначе они
     * остаются висеть (рекомендация документации immersive).
     */
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            enterFullscreen()
        }
    }

    override fun onResume() {
        super.onResume()
        ShellBridge.nativeResumed()
    }

    override fun onPause() {
        ShellBridge.nativePaused()
        super.onPause()
    }

    override fun onDestroy() {
        // onCreate мог упасть раньше, чем создал платформу: тогда отключать нечего.
        if (::platform.isInitialized) {
            ShellBridge.nativeDetach(platform)
            platform.release()
        }
        super.onDestroy()
    }

    /**
     * Все клавиши идут в Rust как есть; решает он. `true` от Rust — клавиша громкости наша, событие
     * потребляется (громкость не меняется, системный ползунок не всплывает). Остальные клавиши
     * и клавиши, которые Rust не принял, обрабатываются системой как обычно (`super`).
     * `dispatchKeyEvent`, а не `onKeyDown`/`onKeyUp`: один вход для нажатия, автоповтора и отпускания.
     */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val consumedByRust = ShellBridge.nativeHardwareKey(event.keyCode, event.action, event.repeatCount, event.eventTime)
        return consumedByRust || super.dispatchKeyEvent(event)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == ShellPlatform.CAMERA_PERMISSION_REQUEST) {
            // Значение rationale — после ответа: по нему Rust отличает первый отказ от постоянного.
            val showRationale = shouldShowRequestPermissionRationale(Manifest.permission.CAMERA)
            ShellBridge.nativeCameraPermissionResult(grantResults, showRationale)
        }
    }

    /** Окно под вырезом камеры, системные панели скрыты и появляются свайпом от края. */
    private fun enterFullscreen() {
        window.attributes = window.attributes.apply {
            layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
        }
        window.insetsController?.let { controller ->
            controller.hide(WindowInsets.Type.systemBars())
            controller.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
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

    /** Колбэки surface → Rust без обработки. */
    private object SurfaceCallbacks : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) = ShellBridge.nativeSurfaceCreated(holder.surface)

        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
            ShellBridge.nativeSurfaceChanged(width, height)

        override fun surfaceDestroyed(holder: SurfaceHolder) = ShellBridge.nativeSurfaceDestroyed()
    }

    companion object {
        /** Intent-параметр с именем smoke-сценария (`scripts/device-smoke.sh`). */
        const val EXTRA_SCENARIO = "scenario"

        /** Тот же тег, что у нативных логов: скрипты фильтруют logcat по нему. */
        private const val LOG_TAG = "Fasticamera"

        private const val STATUS_PADDING_PX = 48
    }
}
