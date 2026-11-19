package dev.fasticamera

import android.view.MotionEvent
import android.view.Surface

/**
 * Колбэки Android → Rust для шва оболочки (crate `android-shell`, модуль `android::natives`).
 *
 * Методы регистрирует `JNI_OnLoad` через `RegisterNatives`; сигнатуры здесь и в Rust сверяются:
 * несовпадение — громкая ошибка при загрузке библиотеки. Kotlin ничего не решает: передаёт
 * события как есть (даже `MotionEvent` целиком — его разбирает Rust, решение владельца 02).
 * Ошибки на стороне Rust не бросают исключений сюда: они пишутся в лог и считаются.
 */
object ShellBridge {
    init {
        // Повторная загрузка той же библиотеки — no-op; гарантирует регистрацию методов,
        // даже если ShellBridge тронули раньше NativeBridge.
        System.loadLibrary("fasticamera")
    }

    /** Activity создана: объект платформы для команд Rust → Kotlin и имя пакета (для датчиков NDK). */
    @JvmStatic external fun nativeAttach(platform: ShellPlatform, packageName: String)

    /** Activity уничтожается: Rust отпускает глобальную ссылку на этот объект платформы. */
    @JvmStatic external fun nativeDetach(platform: ShellPlatform)

    @JvmStatic external fun nativeResumed()

    @JvmStatic external fun nativePaused()

    @JvmStatic external fun nativeSurfaceCreated(surface: Surface)

    @JvmStatic external fun nativeSurfaceChanged(width: Int, height: Int)

    /** Блокирует до подтверждения владельца рендера (с таймаутом): после возврата surface трогать нельзя. */
    @JvmStatic external fun nativeSurfaceDestroyed()

    @JvmStatic external fun nativeTouch(event: MotionEvent)

    /**
     * Аппаратная клавиша: сырые поля `KeyEvent`. Время — `getEventTime()` в мс
     * (`uptimeMillis` = `CLOCK_MONOTONIC`, та же шкала, что у касаний; в наносекунды переводит Rust). Возвращает true, если Rust забрал клавишу
     * (громкость): тогда Activity обязана потребить событие.
     */
    @JvmStatic external fun nativeHardwareKey(keyCode: Int, action: Int, repeatCount: Int, eventTimeMillis: Long): Boolean

    @JvmStatic external fun nativeCameraPermissionResult(grantResults: IntArray, shouldShowRationale: Boolean)

    /** Команда, принятая от Rust, не выполнилась; [command] — один из `ShellPlatform.COMMAND_*`. */
    @JvmStatic external fun nativePlatformCommandFailed(command: Int, reason: String)
}
