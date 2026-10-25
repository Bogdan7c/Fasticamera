package dev.fasticamera.probe.platform

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.PowerManager
import org.json.JSONObject

/**
 * Снимок теплового состояния до и после эксперимента (C2 «тепло», C6 thermal headroom).
 * Температура батареи — из липкого интента ACTION_BATTERY_CHANGED (десятые доли °C).
 */
object ThermalSnapshot {

    /** Прогноз headroom «сейчас»: 0 секунд вперёд. */
    private const val HEADROOM_FORECAST_SECONDS = 0

    private const val TENTHS_PER_DEGREE = 10.0

    fun capture(context: Context): JSONObject {
        val power = context.getSystemService(PowerManager::class.java)
        val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val batteryTenths = battery?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Int.MIN_VALUE)
        val headroom = power.getThermalHeadroom(HEADROOM_FORECAST_SECONDS)
        return JSONObject()
            .put("thermal_status", power.currentThermalStatus)
            // NaN означает «сервис не дал оценку» (например, слишком частые запросы) — пишем явно.
            .put("thermal_headroom", if (headroom.isNaN()) "NaN" else headroom.toDouble())
            .put(
                "battery_temp_c",
                batteryTenths?.takeIf { it != Int.MIN_VALUE }?.let { it / TENTHS_PER_DEGREE } ?: JSONObject.NULL,
            )
            .put("battery_level", battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: JSONObject.NULL)
            .put("sustained_performance_supported", power.isSustainedPerformanceModeSupported)
    }
}
