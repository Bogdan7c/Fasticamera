package dev.fasticamera.probe

import android.view.Surface

/**
 * JNI-шов к Rust-библиотеке `probe_native` (tools/device-probe/native).
 *
 * Контракт: каждая функция, кроме [rawReaderSurface], возвращает JSON-строку
 * `{"ok": true|false, ...}` и не бросает исключений на ожидаемых отказах —
 * отказ API (например, AImageReader не принял usage) и есть результат разведки.
 * Имена символов зависят от пакета `dev.fasticamera.probe` (namespace сборки), а не от applicationId.
 */
object NativeBridge {
    init {
        System.loadLibrary("probe_native")
    }

    /** C1 через NDK: список камер, характеристики и попытка открыть ID 0–7. */
    @JvmStatic external fun ndkCameraCatalog(): String

    /** C4: возможности Vulkan, нужные для импорта AHardwareBuffer. */
    @JvmStatic external fun vulkanInfo(): String

    /** C4: создать AImageReader с заданным usage. Возвращает JSON с `handle` или причиной отказа. */
    @JvmStatic external fun rawReaderCreate(width: Int, height: Int, format: Int, usage: Long, maxImages: Int): String

    /** Java-поверхность нативного ридера (ANativeWindow_toSurface); null — неизвестный handle. */
    @JvmStatic external fun rawReaderSurface(handle: Long): Surface?

    /** C4: принять до [frames] кадров, импортировать в Vulkan, сверить контрольные суммы GPU и CPU. Блокирует поток. */
    @JvmStatic external fun rawReaderRunImportTest(handle: Long, frames: Int, timeoutMs: Int): String

    @JvmStatic external fun rawReaderDestroy(handle: Long): String

    /**
     * C8/V5: swapchain Vulkan на входной surface кодера, [frames] кадров с шагом [frameIntervalNs].
     * [preferTenBit] — выбрать 10-битный формат surface, если драйвер его предлагает.
     */
    @JvmStatic external fun codecSurfaceRenderTest(
        surface: Surface,
        width: Int,
        height: Int,
        frames: Int,
        frameIntervalNs: Long,
        preferTenBit: Boolean,
    ): String
}
