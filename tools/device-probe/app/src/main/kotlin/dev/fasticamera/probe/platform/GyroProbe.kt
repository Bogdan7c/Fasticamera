package dev.fasticamera.probe.platform

import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.SystemClock
import dev.fasticamera.probe.camera.FrameTimingStats
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * C8/V3: фактическая частота гироскопа и шкала его времени.
 *
 * Шкала проверяется сравнением `event.timestamp` с `SystemClock.elapsedRealtimeNanos()`
 * в момент прихода события: если это одна шкала (CLOCK_BOOTTIME, как SENSOR_TIMESTAMP камеры
 * с источником REALTIME), задержка будет положительной и маленькой (единицы миллисекунд).
 */
class GyroProbe(private val sensorManager: SensorManager, private val handler: Handler) {

    fun measure(requestedPeriodUs: Int, durationMs: Long): JSONObject {
        val gyro = sensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE)
            ?: return JSONObject().put("status", "absent")
        val timestamps = ArrayList<Long>()
        val arrivalLagsNs = ArrayList<Long>()
        val finished = CountDownLatch(1)
        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                val arrival = SystemClock.elapsedRealtimeNanos()
                synchronized(timestamps) {
                    timestamps += event.timestamp
                    arrivalLagsNs += arrival - event.timestamp
                }
            }

            override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) = Unit
        }
        val registered = sensorManager.registerListener(listener, gyro, requestedPeriodUs, handler)
        if (!registered) {
            return JSONObject().put("status", "register_failed").put("requested_period_us", requestedPeriodUs)
        }
        // Ждём ровно durationMs: latch никто не отпускает, это осознанная пауза замера.
        finished.await(durationMs, TimeUnit.MILLISECONDS)
        sensorManager.unregisterListener(listener)
        val (timing, lags) = synchronized(timestamps) { FrameTimingStats.compute(timestamps.toList()) to arrivalLagsNs.sorted() }
        return JSONObject()
            .put("status", "measured")
            .put("name", gyro.name)
            .put("vendor", gyro.vendor)
            .put("min_delay_us", gyro.minDelay)
            .put("fifo_max_events", gyro.fifoMaxEventCount)
            .put("highest_direct_report_rate_level", gyro.highestDirectReportRateLevel)
            .put("requested_period_us", requestedPeriodUs)
            .put("timing", timing.toJson())
            .put("arrival_lag_ns_median", lags.getOrNull(lags.size / 2) ?: JSONObject.NULL)
            .put("arrival_lag_ns_min", lags.firstOrNull() ?: JSONObject.NULL)
            .put("arrival_lag_ns_max", lags.lastOrNull() ?: JSONObject.NULL)
    }
}
