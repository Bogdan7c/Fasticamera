package dev.fasticamera.probe.camera.threea

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import dev.fasticamera.probe.report.ProbeLog
import org.json.JSONObject

/**
 * Раз в [periodMs] читает ток батареи (BatteryManager, мкА), счётчик заряда и температуру,
 * пишет события `power`. Нужен потому, что `dumpsys battery` на этом телефоне отдаёт ток 0,
 * а sysfs закрыт для shell. Знак тока смотрим по данным: на Android «минус» обычно разряд.
 */
class ThreeAPowerSampler(
    private val context: Context,
    private val log: ProbeLog,
    private val periodMs: Long,
) {
    @Volatile
    private var running = true
    private val worker = Thread({ loop() }, "probe-power")

    fun start() = worker.start()

    fun stop() {
        running = false
        worker.join()
    }

    private fun loop() {
        val manager = context.getSystemService(BatteryManager::class.java)
        while (running) {
            val sticky = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            log.event(
                "power",
                JSONObject()
                    .put("t_ms", android.os.SystemClock.elapsedRealtime())
                    .put("current_now_ua", manager.getLongProperty(BatteryManager.BATTERY_PROPERTY_CURRENT_NOW))
                    .put("charge_counter_uah", manager.getLongProperty(BatteryManager.BATTERY_PROPERTY_CHARGE_COUNTER))
                    .put("temp_decic", sticky?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Int.MIN_VALUE) ?: JSONObject.NULL)
                    .put("plugged", sticky?.getIntExtra(BatteryManager.EXTRA_PLUGGED, -1) ?: JSONObject.NULL),
            )
            try {
                Thread.sleep(periodMs)
            } catch (_: InterruptedException) {
                return
            }
        }
    }
}
