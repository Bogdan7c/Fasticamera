package dev.fasticamera

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.view.WindowManager

/**
 * Команды Rust → Android (crate `android-shell`, модули `android::platform_calls` и
 * `android::haptic_query`). Rust вызывает эти методы со своего потока команд.
 *
 * Здесь нет решений: какой эффект играть, когда просить разрешение — решает Rust. Методы только
 * переводят вызов в API платформы. Вибромотор потокобезопасен и вызывается сразу; операции с
 * окном и Activity переносятся на главный поток, а их отказ сообщается в Rust кодом `COMMAND_*`.
 */
class ShellPlatform(private val activity: Activity) {
    private val vibrator: Vibrator =
        activity.getSystemService(VibratorManager::class.java).defaultVibrator

    private val batteryManager: BatteryManager =
        activity.getSystemService(BatteryManager::class.java)

    /** Вибрация как «отклик на касание»: система применяет пользовательскую силу и выключатель. */
    private val touchFeedback: VibrationAttributes =
        VibrationAttributes.createForUsage(VibrationAttributes.USAGE_TOUCH)

    fun hasVibrator(): Boolean = vibrator.hasVibrator()

    fun hasAmplitudeControl(): Boolean = vibrator.hasAmplitudeControl()

    /** [primitive] — `VibrationEffect.Composition.PRIMITIVE_*`. */
    fun isPrimitiveSupported(primitive: Int): Boolean = vibrator.arePrimitivesSupported(primitive)[0]

    /** [effect] — `VibrationEffect.EFFECT_*`; ответ — `Vibrator.VIBRATION_EFFECT_SUPPORT_*`. */
    fun effectSupport(effect: Int): Int = vibrator.areEffectsSupported(effect)[0]

    fun playPrimitive(primitive: Int, scale: Float) {
        val effect = VibrationEffect.startComposition().addPrimitive(primitive, scale).compose()
        vibrator.vibrate(effect, touchFeedback)
    }

    fun playPredefinedEffect(effect: Int) {
        vibrator.vibrate(VibrationEffect.createPredefined(effect), touchFeedback)
    }

    /** Волна: длительности шагов в мс и амплитуды 0–255 (0 — пауза), без повтора. */
    fun playWaveform(timings: LongArray, amplitudes: IntArray) {
        vibrator.vibrate(VibrationEffect.createWaveform(timings, amplitudes, NO_REPEAT), touchFeedback)
    }

    /**
     * Сырой снимок батареи одним вызовом (Rust: `android::power_sampler`, раскладка слотов —
     * `crates/android-shell/src/battery_snapshot.rs`, модуль `slot`). Вызывается с потока
     * `fasticamera-power`, не с главного. Толкований здесь нет: «не поддержано» передаётся как
     * есть (`Long.MIN_VALUE` от `getLongProperty`, `Int.MIN_VALUE` вместо отсутствующего extra).
     */
    fun readBatterySnapshot(): LongArray {
        val currentMicroamps = batteryManager.getLongProperty(BatteryManager.BATTERY_PROPERTY_CURRENT_NOW)
        val sampledAtNanos = System.nanoTime()
        val chargeCounter = batteryManager.getLongProperty(BatteryManager.BATTERY_PROPERTY_CHARGE_COUNTER)
        val isCharging = batteryManager.isCharging
        // Липкое намерение: receiver = null только читает последнее состояние, не подписывается.
        // Флаг экспорта для системной рассылки не обязателен, но явный флаг не даёт lint гадать.
        val sticky: Intent? = activity.registerReceiver(
            null,
            IntentFilter(Intent.ACTION_BATTERY_CHANGED),
            Context.RECEIVER_NOT_EXPORTED,
        )
        fun extra(name: String): Long = (sticky?.getIntExtra(name, Int.MIN_VALUE) ?: Int.MIN_VALUE).toLong()
        return longArrayOf(
            BATTERY_SNAPSHOT_LAYOUT_VERSION,
            sampledAtNanos,
            currentMicroamps,
            chargeCounter,
            extra(BatteryManager.EXTRA_STATUS),
            extra(BatteryManager.EXTRA_PLUGGED),
            extra(BatteryManager.EXTRA_TEMPERATURE),
            extra(BatteryManager.EXTRA_LEVEL),
            extra(BatteryManager.EXTRA_SCALE),
            extra(BatteryManager.EXTRA_VOLTAGE),
            if (isCharging) 1L else 0L,
        )
    }

    /** Ответ придёт в `MainActivity.onRequestPermissionsResult` → `ShellBridge`. */
    fun requestCameraPermission() = onMainThread(COMMAND_REQUEST_CAMERA_PERMISSION) {
        activity.requestPermissions(arrayOf(Manifest.permission.CAMERA), CAMERA_PERMISSION_REQUEST)
    }

    fun openSystemGallery() = onMainThread(COMMAND_OPEN_SYSTEM_GALLERY) {
        val gallery = Intent.makeMainSelectorActivity(Intent.ACTION_MAIN, Intent.CATEGORY_APP_GALLERY)
        activity.startActivity(gallery.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    fun setKeepScreenOn(keep: Boolean) = onMainThread(COMMAND_KEEP_SCREEN_ON) {
        if (keep) {
            activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    /** Выполняет [action] на главном потоке; исключение уходит в Rust как отказ команды [command]. */
    private fun onMainThread(command: Int, action: () -> Unit) {
        activity.runOnUiThread {
            try {
                action()
            } catch (error: RuntimeException) {
                ShellBridge.nativePlatformCommandFailed(command, error.toString())
            }
        }
    }

    companion object {
        /** Коды команд — контракт с `crates/android-shell/src/command_codes.rs`. */
        const val COMMAND_REQUEST_CAMERA_PERMISSION = 1
        const val COMMAND_PLAY_HAPTIC = 2
        const val COMMAND_OPEN_SYSTEM_GALLERY = 3
        const val COMMAND_KEEP_SCREEN_ON = 4

        /** Версия раскладки `readBatterySnapshot` — контракт с `BATTERY_SNAPSHOT_LAYOUT_VERSION` в Rust. */
        const val BATTERY_SNAPSHOT_LAYOUT_VERSION = 1L

        /** requestCode запроса разрешения камеры (MainActivity маршрутизирует ответ по нему). */
        const val CAMERA_PERMISSION_REQUEST = 1001

        /** `createWaveform(..., repeat = -1)`: волна играет один раз. */
        private const val NO_REPEAT = -1
    }
}
