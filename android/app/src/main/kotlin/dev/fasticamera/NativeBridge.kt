package dev.fasticamera

/**
 * JNI-шов к Rust-библиотеке `libfasticamera.so` (crates/fasticamera-app, модуль jni_exports).
 * Методы регистрирует `JNI_OnLoad` через `RegisterNatives` (вместе с методами [ShellBridge]).
 *
 * Контракт: обе функции принимают и возвращают JSON-строки и не бросают исключений на
 * ожидаемых отказах — отказ описан в самом JSON (`status: failed`). Исключение
 * (`RuntimeException`) возможно только при сбое самого JNI.
 */
object NativeBridge {
    init {
        System.loadLibrary("fasticamera")
    }

    /**
     * Передать конфиг и имя пакета процесса нативной части (один раз на процесс). Имя пакета нужно Rust для
     * датчиков с первого кадра (решение 106), поэтому приходит здесь, а не при подключении Activity.
     * Ответ: `{"status": "initialized" | "already_initialized" | "failed", ...}`.
     */
    @JvmStatic external fun initialize(configJson: String, packageName: String): String

    /**
     * Выполнить smoke-сценарий по имени. Ответ: `{"scenario": ..., "status": "passed" | "failed" | "panicked", ...}`
     * для мгновенного сценария или `"status": "started"` для наблюдающего (итог — позже в logcat).
     */
    @JvmStatic external fun runScenario(name: String): String
}
