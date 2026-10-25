package dev.fasticamera.probe

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.WindowManager
import android.widget.TextView
import dev.fasticamera.probe.report.FileSink
import dev.fasticamera.probe.report.LogChunker
import dev.fasticamera.probe.report.LogcatSink
import dev.fasticamera.probe.report.ProbeLog
import dev.fasticamera.probe.scenario.CaptureScenario
import dev.fasticamera.probe.scenario.CatalogScenario
import dev.fasticamera.probe.scenario.CodecSurfaceScenario
import dev.fasticamera.probe.scenario.PlatformScenario
import dev.fasticamera.probe.scenario.ProbeScenario
import dev.fasticamera.probe.scenario.ScenarioArgs
import dev.fasticamera.probe.scenario.ScenarioContext
import dev.fasticamera.probe.scenario.StreamScenario
import dev.fasticamera.probe.scenario.VulkanImportScenario
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.Executors

/**
 * Точка входа зонда. Запуск с ПК:
 * `adb shell am start -W -n <pkg>/dev.fasticamera.probe.ProbeActivity -e scenario <name> -e run <id> ...`
 *
 * Activity только маршрутизирует: разбирает интент, поднимает потоки камеры,
 * запускает сценарий в рабочем потоке и всегда завершает его записью `done`.
 * Главный поток не блокируется (инвариант README «UI никогда не ждёт камеру»).
 */
class ProbeActivity : Activity() {

    private val scenarioExecutor = Executors.newSingleThreadExecutor()
    private val cameraThread = HandlerThread("probe-camera").apply { start() }
    private val imageThread = HandlerThread("probe-images").apply { start() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Экран не гаснет во время 5-минутных замеров: иначе камера закроется посреди потока.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        // Запуск с ПК часто приходится на заблокированный экран: без этого Activity остаётся
        // за блокировкой, процесс считается фоновым, и Oplus его замораживает посреди потока
        // (факт сессии 00: isFrozen=true, кадры не приходят). Системная камера делает так же.
        setShowWhenLocked(true)
        setTurnScreenOn(true)
        setContentView(TextView(this).apply { text = "Fasticamera probe: $packageName" })
        startScenario(intent)
    }

    override fun onDestroy() {
        scenarioExecutor.shutdownNow()
        cameraThread.quitSafely()
        imageThread.quitSafely()
        super.onDestroy()
    }

    private fun startScenario(intent: Intent) {
        val args = ScenarioArgs(extractStringExtras(intent))
        val scenarioName = args.string("scenario") ?: "catalog"
        val runId = args.string("run") ?: UUID.randomUUID().toString().take(RUN_ID_LENGTH)
        val log = ProbeLog(
            runId,
            scenarioName,
            sinks = listOf(
                FileSink(File(File(filesDir, ProbeLog.RUNS_DIRECTORY), "$runId.jsonl")),
                LogcatSink(ProbeLog.TAG, LogChunker(ProbeLog.MAX_LOGCAT_CHARS)),
            ),
        )
        log.event(
            "start",
            JSONObject().put("package", packageName).put("args", JSONObject(args.asMap() as Map<*, *>)),
        )
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            // Разрешение выдаёт ПК-скрипт (`pm grant`), диалог здесь бессмысленен для автоматического прогона.
            log.error("permission", "нет android.permission.CAMERA — выдайте: adb shell pm grant $packageName android.permission.CAMERA")
            log.done(JSONObject().put("status", "no_permission"))
            finishWhenDone(args)
            return
        }
        val scenario = scenarioFor(scenarioName)
        if (scenario == null) {
            log.error("scenario", "неизвестный сценарий '$scenarioName'")
            log.done(JSONObject().put("status", "unknown_scenario"))
            finishWhenDone(args)
            return
        }
        val context = ScenarioContext(
            appContext = applicationContext,
            runId = runId,
            args = args,
            log = log,
            cameraHandler = Handler(cameraThread.looper),
            imageHandler = Handler(imageThread.looper),
        )
        scenarioExecutor.execute {
            val status = try {
                scenario.run(context)
                "completed"
            } catch (error: Exception) {
                log.error("scenario_failed", error)
                "failed"
            }
            log.done(JSONObject().put("status", status))
            runOnUiThread { finishWhenDone(args) }
        }
    }

    /** По умолчанию Activity закрывается после сценария, чтобы следующий запуск начинался с чистой камеры. */
    private fun finishWhenDone(args: ScenarioArgs) {
        if (args.string("keep_open") != "1") {
            finish()
        }
    }

    private fun scenarioFor(name: String): ProbeScenario? = when (name) {
        "catalog" -> CatalogScenario()
        "stream" -> StreamScenario()
        "vkimport" -> VulkanImportScenario()
        "platform" -> PlatformScenario()
        "capture" -> CaptureScenario()
        "codecsurface" -> CodecSurfaceScenario()
        else -> null
    }

    /** Берём только строковые extras (`am start -e`); остальные типы сценариям не нужны. */
    private fun extractStringExtras(intent: Intent): Map<String, String> {
        val extras = intent.extras ?: return emptyMap()
        return extras.keySet().mapNotNull { key -> extras.getString(key)?.let { key to it } }.toMap()
    }

    private companion object {
        const val RUN_ID_LENGTH = 8
    }
}
