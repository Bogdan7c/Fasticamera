package dev.fasticamera

import android.Manifest
import android.app.Activity
import android.os.Bundle
import android.util.Log
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
        setContentView(FrameLayout(this).apply {
            addView(surfaceView)
            addView(statusView)
        })
        // После setContentView: контроллер панелей живёт у DecorView, а её создаёт setContentView.
        enterFullscreen()

        val initReport = NativeBridge.initialize(NativeConfig.fromIntent(intent).toJson())
        logIfInitFailed(initReport)

        platform = ShellPlatform(this)
        ShellBridge.nativeAttach(platform, packageName)
        surfaceView.holder.addCallback(SurfaceCallbacks)
        surfaceView.setOnTouchListener { _, event ->
            ShellBridge.nativeTouch(event)
            true
        }

        val scenario = intent.getStringExtra(EXTRA_SCENARIO)
        statusView.text = if (scenario != null) NativeBridge.runScenario(scenario) else initReport
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
        }
        super.onDestroy()
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
