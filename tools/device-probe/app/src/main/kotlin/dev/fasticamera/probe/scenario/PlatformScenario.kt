package dev.fasticamera.probe.scenario

import android.hardware.SensorManager
import dev.fasticamera.probe.platform.DeviceCapabilityReports
import dev.fasticamera.probe.platform.GyroProbe
import dev.fasticamera.probe.platform.StorageSpeedProbe
import dev.fasticamera.probe.platform.ThermalSnapshot
import org.json.JSONObject

/**
 * C6 и часть C8: хаптика, экран, тепло, микрофоны, кодеры, гироскоп, скорость записи.
 * Каждый блок — отдельная запись, чтобы сбой одного не скрыл остальные.
 */
class PlatformScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val app = context.appContext
        section(context, "haptics") { DeviceCapabilityReports.haptics(app) }
        section(context, "display") { DeviceCapabilityReports.display(app) }
        section(context, "thermal") { ThermalSnapshot.capture(app) }
        section(context, "microphones") { DeviceCapabilityReports.microphones(app) }
        section(context, "video_encoders") { JSONObject().put("encoders", DeviceCapabilityReports.videoEncoders()) }
        section(context, "gyro") {
            GyroProbe(app.getSystemService(SensorManager::class.java), context.imageHandler).measure(
                requestedPeriodUs = context.args.int("gyro_period_us", DEFAULT_GYRO_PERIOD_US),
                durationMs = context.args.long("gyro_duration_ms", DEFAULT_GYRO_DURATION_MS),
            )
        }
        val storageMib = context.args.int("storage_mib", DEFAULT_STORAGE_MIB)
        if (storageMib > 0) {
            section(context, "storage_write") { StorageSpeedProbe.measure(app.filesDir, storageMib) }
        }
    }

    private fun section(context: ScenarioContext, name: String, block: () -> JSONObject) {
        try {
            context.log.result(name, block())
        } catch (error: RuntimeException) {
            context.log.error(name, error)
        }
    }

    private companion object {
        /** 2500 мкс = 400 Гц — заявленный максимум BMI26x. */
        const val DEFAULT_GYRO_PERIOD_US = 2_500
        const val DEFAULT_GYRO_DURATION_MS = 3_000L
        const val DEFAULT_STORAGE_MIB = 1024
    }
}
