package dev.fasticamera.probe.scenario

import dev.fasticamera.probe.NativeBridge
import dev.fasticamera.probe.NativeJson
import dev.fasticamera.probe.camera.RawOutput
import dev.fasticamera.probe.camera.StreamExperiment
import dev.fasticamera.probe.camera.StreamExperimentConfig
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * C4: RAW из нативного AImageReader → импорт AHardwareBuffer в Vulkan → контрольная сумма.
 *
 * Kotlin управляет камерой (как и в будущем гибриде, если NDK не даст нужный режим сессии),
 * а кадры читает только нативная сторона. Сначала перебираем комбинации usage: какую примет
 * AImageReader для RAW — отдельный факт, даже если до импорта дело не дойдёт.
 */
class VulkanImportScenario : ProbeScenario {

    override fun run(context: ScenarioContext) {
        context.log.result("vulkan_info", NativeJson.call { NativeBridge.vulkanInfo() })
        val config = StreamExperimentConfig.fromArgs(context.args)
        val usages = context.args.stringList("usages", DEFAULT_USAGE_CANDIDATES).map { candidate ->
            dev.fasticamera.probe.scenario.ScenarioArgs.parseLongAnyRadix(candidate)
        }
        val importFrames = context.args.int("import_frames", DEFAULT_IMPORT_FRAMES)

        var acceptedHandle: Long? = null
        var acceptedUsage: Long? = null
        for (usage in usages) {
            val created = NativeJson.call {
                NativeBridge.rawReaderCreate(config.size.width, config.size.height, config.format, usage, config.maxImages)
            }
            context.log.result("reader_create", created.put("usage", "0x" + usage.toString(16)))
            if (created.optBoolean("ok") && acceptedHandle == null) {
                acceptedHandle = created.getLong("handle")
                acceptedUsage = usage
            } else if (created.optBoolean("ok")) {
                // Нужен только первый принятый вариант; остальные сразу освобождаем.
                NativeBridge.rawReaderDestroy(created.getLong("handle"))
            }
        }
        val handle = acceptedHandle ?: run {
            context.log.error("reader_create", "AImageReader не принял ни один usage для формата 0x${config.format.toString(16)}")
            return
        }
        try {
            runImport(context, config, handle, acceptedUsage, importFrames)
        } finally {
            context.log.result("reader_destroy", NativeJson.call { NativeBridge.rawReaderDestroy(handle) })
        }
    }

    private fun runImport(context: ScenarioContext, config: StreamExperimentConfig, handle: Long, usage: Long?, importFrames: Int) {
        val surface = NativeBridge.rawReaderSurface(handle) ?: run {
            context.log.error("reader_surface", "нет Surface для handle=$handle")
            return
        }
        // Нативный тест блокирует свой поток до прихода кадров, поэтому стартует раньше камеры.
        val importExecutor = Executors.newSingleThreadExecutor()
        try {
            val importResult = importExecutor.submit<String> {
                NativeBridge.rawReaderRunImportTest(handle, importFrames, config.timeoutMs.toInt())
            }
            val stream = StreamExperiment(context.newSessionRunner(), context.imageHandler) { stage, details -> context.log.event(stage, details) }
                .run(config, RawOutput.ExternalSurface(surface, importResult))
            context.log.result("import_stream", stream.put("usage", usage?.let { "0x" + it.toString(16) }))
            val importJson = if (importResult.isDone) {
                NativeJson.call { importResult.get() }
            } else {
                JSONObject().put("ok", false).put("stage", "timeout").put("error", "нативный тест не завершился за timeout_ms")
            }
            context.log.result("import_test", importJson)
        } finally {
            importExecutor.shutdownNow()
            surface.release()
        }
    }

    private companion object {
        const val DEFAULT_IMPORT_FRAMES = 5

        /**
         * AHARDWAREBUFFER_USAGE_*: GPU_SAMPLED_IMAGE = 0x100, GPU_DATA_BUFFER = 0x1000000,
         * CPU_READ_OFTEN = 0x3 (нужен только для сверки контрольной суммы, не для продукта).
         *
         * GPU_DATA_BUFFER намеренно не в списке по умолчанию: AImageReader создаётся с ним без ошибки,
         * но gralloc этого телефона отказывает в выделении не-BLOB буфера («not supported prior to
         * gralloc 4.1», факт сессии 00) — камера тогда молча не отдаёт ни одного кадра.
         */
        val DEFAULT_USAGE_CANDIDATES = listOf("0x100", "0x103", "0x3")
    }
}
