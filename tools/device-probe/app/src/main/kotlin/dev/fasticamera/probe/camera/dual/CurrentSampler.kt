package dev.fasticamera.probe.camera.dual

import android.content.Context
import android.os.BatteryManager
import org.json.JSONObject

/**
 * Замер тока батареи (`CURRENT_NOW`) во время потока. Единицы — как отдаёт прошивка
 * (на OnePlus 11 по факту мА, у AOSP — мкА); поэтому в отчёт пишутся сырые числа и знак.
 *
 * Замер имеет смысл только на батарее: при зарядке ток — это ток зарядки.
 */
class CurrentSampler(context: Context, private val source: Source = Source.NOW) {
    /** Какое свойство батареи читаем: мгновенное или усреднённое прошивкой. */
    enum class Source(val wireName: String, val property: Int) {
        NOW("now", BatteryManager.BATTERY_PROPERTY_CURRENT_NOW),
        AVERAGE("average", BatteryManager.BATTERY_PROPERTY_CURRENT_AVERAGE),
    }

    private val battery = context.getSystemService(BatteryManager::class.java)

    val isOnBattery: Boolean get() = !battery.isCharging

    private val samples = ArrayList<Double>()

    /** Один отсчёт; вызывается из потока ожидания сценария раз в ~0,5 с. */
    fun medianOrNull(): Double? = SampleSummary.of(samples)?.median

    fun sample() {
        samples += battery.getIntProperty(source.property).toDouble()
    }

    fun toJson(): JSONObject = JSONObject()
        .put("on_battery", isOnBattery)
        .put("source", source.wireName)
        .put("current_now_raw", SampleSummary.toJsonOrNull(samples))
        // Сколько РАЗНЫХ значений в окне: 1–2 на 20 отсчётов = прошивка отдаёт устаревшее.
        .put("distinct_values", samples.distinct().size)
        .put("raw_samples", org.json.JSONArray(samples))
}
