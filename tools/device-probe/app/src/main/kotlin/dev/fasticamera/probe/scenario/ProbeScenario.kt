package dev.fasticamera.probe.scenario

import android.content.Context
import android.hardware.camera2.CameraManager
import android.os.Handler
import dev.fasticamera.probe.camera.CameraSessionRunner
import dev.fasticamera.probe.report.ProbeLog

/**
 * Всё, что нужно сценарию: контекст, аргументы, лог и общие потоки Camera2.
 * Сценарии не создают свои потоки камеры — их жизненным циклом владеет ProbeActivity.
 */
class ScenarioContext(
    val appContext: Context,
    /** Идентификатор прогона: им называются файлы результатов и снимков. */
    val runId: String,
    val args: ScenarioArgs,
    val log: ProbeLog,
    val cameraHandler: Handler,
    val imageHandler: Handler,
) {
    val cameraManager: CameraManager = appContext.getSystemService(CameraManager::class.java)

    fun newSessionRunner(): CameraSessionRunner = CameraSessionRunner(cameraManager, cameraHandler)
}

/** Сценарий зонда. Пишет результаты через [ScenarioContext.log]; исключение = ошибка сценария целиком. */
fun interface ProbeScenario {
    fun run(context: ScenarioContext)
}
