package dev.fasticamera

/**
 * JNI-шов к Rust-библиотеке `libfasticamera.so` (crates/fasticamera-app, модуль jni_exports).
 *
 * Контракт: обе функции принимают и возвращают JSON-строки и не бросают исключений на
 * ожидаемых отказах — отказ описан в самом JSON (`status: failed`). Исключение
 * (`RuntimeException`) возможно только при сбое самого JNI.
 */
object NativeBridge {
    init {
        System.loadLibrary("fasticamera")
    }

    /** Передать конфиг нативной части (один раз на процесс). Ответ: `{"status": "initialized" | "already_initialized" | "failed", ...}`. */
    @JvmStatic external fun initialize(configJson: String): String

    /** Выполнить smoke-сценарий по имени. Ответ: `{"scenario": ..., "status": "passed" | "failed" | "panicked", ...}`. */
    @JvmStatic external fun runScenario(name: String): String
}
