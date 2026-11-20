package dev.fasticamera.probe.camera.dual

import android.graphics.ImageFormat

/** Формат выходного потока «ноги» эксперимента: короткое имя из команды и константа ImageFormat. */
enum class LegFormat(val wireName: String, val imageFormat: Int) {
    YUV("yuv", ImageFormat.YUV_420_888),
    RAW10("raw10", ImageFormat.RAW10),
    RAW16("raw16", ImageFormat.RAW_SENSOR),
    ;

    companion object {
        fun fromWire(raw: String): LegFormat = entries.firstOrNull { it.wireName == raw }
            ?: throw IllegalArgumentException("неизвестный формат '$raw' (ожидалось ${entries.joinToString("|") { it.wireName }})")
    }
}

/**
 * Одна «нога» эксперимента: один выходной поток одной камеры.
 *
 * Запись в команде: `камера:формат:ШxВ[:физическая]`, например `2:raw10:4096x3072`
 * или `0:yuv:640x480:3` (логическая камера 0, поток физической камеры 3).
 * Ноги с одним [cameraId] попадают в одну сессию этой камеры.
 */
data class LegSpec(
    val cameraId: String,
    val format: LegFormat,
    val width: Int,
    val height: Int,
    val physicalCameraId: String?,
) {
    /** Имя ноги для отчёта и таблицы: однозначно в рамках одной команды. */
    val label: String
        get() = buildString {
            append("cam").append(cameraId)
            physicalCameraId?.let { append("/phys").append(it) }
            append(':').append(format.wireName).append(':').append(width).append('x').append(height)
        }

    companion object {
        /** Разбор одной ноги; любая неточность — исключение с исходным текстом, а не молчаливая подстановка. */
        fun parse(raw: String): LegSpec {
            val parts = raw.trim().split(':')
            require(parts.size in MIN_PARTS..MAX_PARTS) { "нога '$raw': ожидалось камера:формат:ШxВ[:физическая]" }
            val dimensions = parts[2].lowercase().split('x')
            require(dimensions.size == 2) { "нога '$raw': размер должен быть вида 640x480" }
            val width = dimensions[0].toIntOrNull()
            val height = dimensions[1].toIntOrNull()
            require(width != null && height != null && width > 0 && height > 0) { "нога '$raw': некорректный размер '${parts[2]}'" }
            return LegSpec(
                cameraId = parts[0].also { require(it.isNotEmpty()) { "нога '$raw': пустой id камеры" } },
                format = LegFormat.fromWire(parts[1]),
                width = width,
                height = height,
                physicalCameraId = parts.getOrNull(PHYSICAL_INDEX)?.takeIf { it.isNotEmpty() },
            )
        }

        fun parseList(raw: List<String>): List<LegSpec> = raw.map(::parse)

        private const val MIN_PARTS = 3
        private const val MAX_PARTS = 4
        private const val PHYSICAL_INDEX = 3
    }
}
