package dev.fasticamera.probe.scenario

/**
 * Аргументы сценария из интента (`am start ... -e key value`).
 *
 * Все значения приходят строками: так команды на ПК единообразны, а разбор и
 * ошибки разбора сосредоточены здесь. Неверное значение — исключение с именем
 * параметра, а не молчаливое значение по умолчанию.
 */
class ScenarioArgs(private val values: Map<String, String>) {

    fun string(name: String): String? = values[name]?.takeIf { it.isNotBlank() }

    fun requireString(name: String): String =
        string(name) ?: throw IllegalArgumentException("нет обязательного параметра '$name'")

    fun int(name: String, default: Int): Int = parse(name, default) { it.toInt() }

    fun intOrNull(name: String): Int? = string(name)?.let { raw -> parseOrThrow(name, raw) { it.toInt() } }

    fun long(name: String, default: Long): Long = parse(name, default) { it.toLong() }

    fun longOrNull(name: String): Long? = string(name)?.let { raw -> parseOrThrow(name, raw) { it.toLong() } }

    /** Числа допускаются в десятичном и шестнадцатеричном (`0x8001`) виде — режимы сессии QTI пишут в hex. */
    fun intAnyRadix(name: String, default: Int): Int = parse(name, default, ::parseIntAnyRadix)

    fun longAnyRadix(name: String, default: Long): Long = parse(name, default, ::parseLongAnyRadix)

    /** Список через запятую: `-e formats 32,37`. */
    fun intList(name: String, default: List<Int>): List<Int> {
        val raw = string(name) ?: return default
        return raw.split(',').map { item -> parseOrThrow(name, item.trim(), ::parseIntAnyRadix) }
    }

    fun stringList(name: String, default: List<String>): List<String> {
        val raw = string(name) ?: return default
        return raw.split(',').map { it.trim() }.filter { it.isNotEmpty() }
    }

    /** Все пары ключ=значение — для отчёта: какой командой получен результат. */
    fun asMap(): Map<String, String> = values.toSortedMap()

    private fun <T> parse(name: String, default: T, parser: (String) -> T): T {
        val raw = string(name) ?: return default
        return parseOrThrow(name, raw, parser)
    }

    private fun <T> parseOrThrow(name: String, raw: String, parser: (String) -> T): T =
        try {
            parser(raw)
        } catch (error: NumberFormatException) {
            throw IllegalArgumentException("параметр '$name': не число '$raw'", error)
        }

    companion object {
        fun parseIntAnyRadix(raw: String): Int = parseLongAnyRadix(raw).let { value ->
            if (value !in Int.MIN_VALUE.toLong()..UInt.MAX_VALUE.toLong()) {
                throw NumberFormatException("вне диапазона int: $raw")
            }
            // Значения вроде 0xF0000001 законны как битовые маски: переносим в Int без потери битов.
            value.toInt()
        }

        fun parseLongAnyRadix(raw: String): Long {
            val trimmed = raw.trim()
            return if (trimmed.startsWith("0x") || trimmed.startsWith("0X")) {
                java.lang.Long.parseUnsignedLong(trimmed.substring(2), 16)
            } else {
                trimmed.toLong()
            }
        }
    }
}
