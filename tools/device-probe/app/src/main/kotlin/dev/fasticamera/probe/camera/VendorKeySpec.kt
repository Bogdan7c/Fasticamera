package dev.fasticamera.probe.camera

import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.CaptureResult

/**
 * Тип значения вендорского тега. Совпадает с типами из дампа
 * `dumpsys media.camera` («with type 0 (byte)» и т.д.). Если тип не совпадёт
 * с объявленным в HAL, фреймворк бросит исключение — мы его покажем, а не проглотим.
 */
enum class VendorValueType(val wireName: String) {
    BYTE("byte"),
    INT32("int"),
    INT64("long"),
    FLOAT("float"),
    INT32_ARRAY("int[]"),
    ;

    companion object {
        fun fromWire(raw: String): VendorValueType = entries.firstOrNull { it.wireName == raw }
            ?: throw IllegalArgumentException(
                "неизвестный тип '$raw', допустимы: ${entries.joinToString { it.wireName }}",
            )
    }
}

/**
 * Один вендорский ключ со значением: `org.quic.camera.manualExposure.disableFPSLimits:byte=1`.
 *
 * Значение хранится уже в Java-типе, который ждёт marshaller Camera2,
 * поэтому ошибка формата всплывает при разборе команды, а не в середине сессии.
 */
data class VendorKeyValue(
    val name: String,
    val type: VendorValueType,
    val value: Any,
) {
    /** Записывает значение в запрос. Ошибка HAL/фреймворка пробрасывается вызывающему сценарию. */
    fun applyTo(builder: CaptureRequest.Builder) {
        when (type) {
            VendorValueType.BYTE -> builder.set(CaptureRequest.Key(name, Byte::class.javaObjectType), value as Byte)
            VendorValueType.INT32 -> builder.set(CaptureRequest.Key(name, Int::class.javaObjectType), value as Int)
            VendorValueType.INT64 -> builder.set(CaptureRequest.Key(name, Long::class.javaObjectType), value as Long)
            VendorValueType.FLOAT -> builder.set(CaptureRequest.Key(name, Float::class.javaObjectType), value as Float)
            VendorValueType.INT32_ARRAY -> builder.set(CaptureRequest.Key(name, IntArray::class.java), value as IntArray)
        }
    }

    fun describe(): String = "$name:${type.wireName}=" + when (value) {
        is IntArray -> value.joinToString("|")
        else -> value.toString()
    }

    companion object {
        /**
         * Разбирает список через `;`: `name:type=value;name2:type=v1|v2`.
         * Элементы массива разделяются `|`, потому что `,` и `;` уже заняты.
         */
        fun parseList(raw: String?): List<VendorKeyValue> {
            if (raw.isNullOrBlank()) return emptyList()
            return raw.split(';').map { it.trim() }.filter { it.isNotEmpty() }.map(::parseOne)
        }

        fun parseOne(spec: String): VendorKeyValue {
            val typeSeparator = spec.lastIndexOf(':')
            val valueSeparator = spec.indexOf('=', startIndex = typeSeparator.coerceAtLeast(0))
            require(typeSeparator > 0 && valueSeparator > typeSeparator) {
                "ожидалось 'имя:тип=значение', получено '$spec'"
            }
            val name = spec.substring(0, typeSeparator)
            val type = VendorValueType.fromWire(spec.substring(typeSeparator + 1, valueSeparator))
            val rawValue = spec.substring(valueSeparator + 1)
            return VendorKeyValue(name, type, parseValue(type, rawValue, spec))
        }

        private fun parseValue(type: VendorValueType, rawValue: String, spec: String): Any = try {
            when (type) {
                VendorValueType.BYTE -> rawValue.toInt().also {
                    require(it in Byte.MIN_VALUE..UByte.MAX_VALUE.toInt()) { "byte вне диапазона: $it" }
                }.toByte()
                VendorValueType.INT32 -> parseIntAnyRadixLocal(rawValue)
                VendorValueType.INT64 -> rawValue.toLong()
                VendorValueType.FLOAT -> rawValue.toFloat()
                VendorValueType.INT32_ARRAY -> rawValue.split('|').map { parseIntAnyRadixLocal(it) }.toIntArray()
            }
        } catch (error: NumberFormatException) {
            throw IllegalArgumentException("не удалось разобрать значение в '$spec'", error)
        }

        private fun parseIntAnyRadixLocal(raw: String): Int =
            dev.fasticamera.probe.scenario.ScenarioArgs.parseIntAnyRadix(raw)
    }
}

/**
 * Вендорский ключ результата для чтения: `org.codeaurora.qcamera3.sensor_meta_data.current_mode:int`.
 * Значение читаем в объявленном типе; ошибка чтения возвращается как строка в отчёт.
 */
data class VendorResultKey(val name: String, val type: VendorValueType) {

    /** Результат чтения: значение, отсутствие или ошибка — три разных исхода, не сливаем их. */
    sealed interface ReadOutcome {
        data class Value(val value: Any) : ReadOutcome
        data object Absent : ReadOutcome
        data class Failed(val reason: String) : ReadOutcome
    }

    fun read(result: CaptureResult): ReadOutcome = try {
        val value: Any? = when (type) {
            VendorValueType.BYTE -> result.get(CaptureResult.Key(name, Byte::class.javaObjectType))
            VendorValueType.INT32 -> result.get(CaptureResult.Key(name, Int::class.javaObjectType))
            VendorValueType.INT64 -> result.get(CaptureResult.Key(name, Long::class.javaObjectType))
            VendorValueType.FLOAT -> result.get(CaptureResult.Key(name, Float::class.javaObjectType))
            VendorValueType.INT32_ARRAY -> result.get(CaptureResult.Key(name, IntArray::class.java))
        }
        if (value == null) ReadOutcome.Absent else ReadOutcome.Value(value)
    } catch (error: RuntimeException) {
        // IllegalArgumentException от фреймворка: тег неизвестен или тип не совпал.
        ReadOutcome.Failed("${error.javaClass.simpleName}: ${error.message}")
    }

    companion object {
        /** `name:type;name2:type` */
        fun parseList(raw: String?): List<VendorResultKey> {
            if (raw.isNullOrBlank()) return emptyList()
            return raw.split(';').map { it.trim() }.filter { it.isNotEmpty() }.map { spec ->
                val separator = spec.lastIndexOf(':')
                require(separator > 0) { "ожидалось 'имя:тип', получено '$spec'" }
                VendorResultKey(spec.substring(0, separator), VendorValueType.fromWire(spec.substring(separator + 1)))
            }
        }
    }
}
