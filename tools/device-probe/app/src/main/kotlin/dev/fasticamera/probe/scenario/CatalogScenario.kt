package dev.fasticamera.probe.scenario

import android.hardware.camera2.CameraAccessException
import dev.fasticamera.probe.NativeBridge
import dev.fasticamera.probe.NativeJson
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.camera.CharacteristicsReport
import org.json.JSONArray
import org.json.JSONObject

/**
 * C1: что видит наш пакет. Java-список камер, характеристики и открытие каждого ID
 * (в том числе не перечисленных: 4 и 5 помечены SYSTEM_CAMERA), плюс тот же опрос через NDK.
 */
class CatalogScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        val manager = context.cameraManager
        val listedIds = manager.cameraIdList.toList()
        context.log.result(
            "java_id_list",
            JSONObject()
                .put("ids", JSONArray(listedIds))
                .put("concurrent_sets", JSONArray(manager.concurrentCameraIds.map { JSONArray(it.toList()) })),
        )

        val probeIds = (listedIds + (0 until context.args.int("max_probe_id", DEFAULT_MAX_PROBE_ID) + 1).map(Int::toString)).distinct()
        val runner = context.newSessionRunner()
        for (cameraId in probeIds) {
            context.log.result("java_camera", describeCamera(context, runner, cameraId, cameraId in listedIds))
        }

        context.log.result("ndk_catalog", NativeJson.call { NativeBridge.ndkCameraCatalog() })
    }

    private fun describeCamera(context: ScenarioContext, runner: CameraSessionRunner, cameraId: String, listed: Boolean): JSONObject {
        val entry = JSONObject().put("id", cameraId).put("listed", listed)
        try {
            entry.put("characteristics", CharacteristicsReport.build(context.cameraManager.getCameraCharacteristics(cameraId)))
        } catch (error: IllegalArgumentException) {
            entry.put("characteristics_error", "${error.javaClass.simpleName}: ${error.message}")
        } catch (error: CameraAccessException) {
            entry.put("characteristics_error", "${error.javaClass.simpleName}(${error.reason}): ${error.message}")
        }
        // Открытие — главный факт для скрытых камер: «перечислена» и «открывается» — разные вещи.
        val outcome = runner.open(cameraId, context.args.long("open_timeout_ms", DEFAULT_OPEN_TIMEOUT_MS))
        entry.put("open", outcome.toJson())
        if (outcome is CameraSessionRunner.OpenOutcome.Opened) {
            outcome.device.close()
        }
        return entry
    }

    companion object {
        private const val DEFAULT_MAX_PROBE_ID = 7
        private const val DEFAULT_OPEN_TIMEOUT_MS = 4_000L
    }
}
