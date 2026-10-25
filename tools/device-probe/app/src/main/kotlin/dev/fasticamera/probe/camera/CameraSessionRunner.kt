package dev.fasticamera.probe.camera

import android.annotation.SuppressLint
import android.hardware.camera2.CameraAccessException
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.os.Handler
import org.json.JSONObject
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * Блокирующая обёртка над открытием камеры и созданием сессии для сценариев зонда.
 *
 * Сценарий работает в своём рабочем потоке и ждёт исхода с таймаутом, а колбэки
 * Camera2 приходят в поток [cameraHandler]. Каждый исход — отдельный вариант sealed-типа:
 * «отказ HAL», «отказ по безопасности», «таймаут» и «отключено» для разведки важно различать.
 */
class CameraSessionRunner(
    private val manager: CameraManager,
    private val cameraHandler: Handler,
) {
    private val cameraExecutor = Executor { command -> cameraHandler.post(command) }

    /** Ошибки устройства, пришедшие уже после открытия (например, HAL упал посреди потока). */
    val deviceErrors: MutableList<String> = CopyOnWriteArrayList()

    sealed interface OpenOutcome {
        data class Opened(val device: CameraDevice) : OpenOutcome
        data class DeviceError(val code: Int, val codeName: String) : OpenOutcome
        data class Rejected(val exception: String, val message: String, val reason: Int?) : OpenOutcome
        data object Disconnected : OpenOutcome
        data object TimedOut : OpenOutcome

        fun toJson(): JSONObject = when (this) {
            is Opened -> JSONObject().put("status", "opened")
            is DeviceError -> JSONObject().put("status", "device_error").put("code", code).put("code_name", codeName)
            is Rejected -> JSONObject().put("status", "rejected").put("exception", exception)
                .put("message", message).put("reason", reason ?: JSONObject.NULL)
            Disconnected -> JSONObject().put("status", "disconnected")
            TimedOut -> JSONObject().put("status", "timeout")
        }
    }

    sealed interface SessionOutcome {
        data class Configured(val session: CameraCaptureSession) : SessionOutcome
        data object ConfigureFailed : SessionOutcome
        data class Rejected(val exception: String, val message: String) : SessionOutcome
        data object TimedOut : SessionOutcome

        fun toJson(): JSONObject = when (this) {
            is Configured -> JSONObject().put("status", "configured")
            ConfigureFailed -> JSONObject().put("status", "configure_failed")
            is Rejected -> JSONObject().put("status", "rejected").put("exception", exception).put("message", message)
            TimedOut -> JSONObject().put("status", "timeout")
        }
    }

    // Разрешение CAMERA проверяет ProbeActivity до запуска любого сценария.
    @SuppressLint("MissingPermission")
    fun open(cameraId: String, timeoutMs: Long): OpenOutcome {
        val outcome = CompletableFuture<OpenOutcome>()
        val callback = object : CameraDevice.StateCallback() {
            override fun onOpened(camera: CameraDevice) {
                if (!outcome.complete(OpenOutcome.Opened(camera))) {
                    // Таймаут уже наступил: устройство никому не нужно, закрываем, чтобы не держать камеру.
                    camera.close()
                }
            }

            override fun onDisconnected(camera: CameraDevice) {
                if (!outcome.complete(OpenOutcome.Disconnected)) {
                    deviceErrors += "disconnected camera=${camera.id}"
                }
                camera.close()
            }

            override fun onError(camera: CameraDevice, error: Int) {
                if (!outcome.complete(OpenOutcome.DeviceError(error, deviceErrorName(error)))) {
                    deviceErrors += "error ${deviceErrorName(error)} camera=${camera.id}"
                }
                camera.close()
            }
        }
        try {
            manager.openCamera(cameraId, cameraExecutor, callback)
        } catch (error: CameraAccessException) {
            return OpenOutcome.Rejected(error.javaClass.name, error.message ?: "", error.reason)
        } catch (error: SecurityException) {
            return OpenOutcome.Rejected(error.javaClass.name, error.message ?: "", null)
        } catch (error: IllegalArgumentException) {
            return OpenOutcome.Rejected(error.javaClass.name, error.message ?: "", null)
        }
        return awaitOrTimeout(outcome, timeoutMs) { OpenOutcome.TimedOut }
    }

    /**
     * Создаёт сессию произвольного типа. [sessionType] — не только REGULAR/HIGH_SPEED:
     * вендорские режимы (≥ 0x8000) публичный конструктор SessionConfiguration тоже принимает,
     * а примет ли их фреймворк и HAL — это и есть эксперимент C2.3.
     */
    fun createSession(
        device: CameraDevice,
        outputs: List<OutputConfiguration>,
        sessionType: Int,
        sessionParameters: CaptureRequest?,
        timeoutMs: Long,
    ): SessionOutcome {
        val outcome = CompletableFuture<SessionOutcome>()
        val callback = object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(session: CameraCaptureSession) {
                if (!outcome.complete(SessionOutcome.Configured(session))) {
                    session.close()
                }
            }

            override fun onConfigureFailed(session: CameraCaptureSession) {
                outcome.complete(SessionOutcome.ConfigureFailed)
            }
        }
        val configuration = SessionConfiguration(sessionType, outputs, cameraExecutor, callback)
        if (sessionParameters != null) {
            configuration.sessionParameters = sessionParameters
        }
        try {
            device.createCaptureSession(configuration)
        } catch (error: CameraAccessException) {
            return SessionOutcome.Rejected(error.javaClass.name, error.message ?: "")
        } catch (error: IllegalArgumentException) {
            return SessionOutcome.Rejected(error.javaClass.name, error.message ?: "")
        } catch (error: UnsupportedOperationException) {
            return SessionOutcome.Rejected(error.javaClass.name, error.message ?: "")
        } catch (error: IllegalStateException) {
            return SessionOutcome.Rejected(error.javaClass.name, error.message ?: "")
        }
        return awaitOrTimeout(outcome, timeoutMs) { SessionOutcome.TimedOut }
    }

    private fun <T> awaitOrTimeout(future: CompletableFuture<T>, timeoutMs: Long, onTimeout: () -> T): T = try {
        future.get(timeoutMs, TimeUnit.MILLISECONDS)
    } catch (_: TimeoutException) {
        // complete() после таймаута вернёт false в колбэке, и тот сам закроет поздний объект.
        val timedOut = onTimeout()
        if (future.complete(timedOut)) timedOut else future.get()
    }

    companion object {
        fun deviceErrorName(code: Int): String = when (code) {
            CameraDevice.StateCallback.ERROR_CAMERA_IN_USE -> "ERROR_CAMERA_IN_USE"
            CameraDevice.StateCallback.ERROR_MAX_CAMERAS_IN_USE -> "ERROR_MAX_CAMERAS_IN_USE"
            CameraDevice.StateCallback.ERROR_CAMERA_DISABLED -> "ERROR_CAMERA_DISABLED"
            CameraDevice.StateCallback.ERROR_CAMERA_DEVICE -> "ERROR_CAMERA_DEVICE"
            CameraDevice.StateCallback.ERROR_CAMERA_SERVICE -> "ERROR_CAMERA_SERVICE"
            else -> "UNKNOWN_$code"
        }
    }
}
