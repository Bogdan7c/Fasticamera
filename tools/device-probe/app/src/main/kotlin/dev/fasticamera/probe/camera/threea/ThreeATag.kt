package dev.fasticamera.probe.camera.threea

/**
 * Метка запроса (`CaptureRequest.Builder.setTag`): по ней результат привязывается к фазе и виду запроса,
 * даже если он пришёл уже после смены повторяющегося запроса.
 */
data class ThreeATag(
    /** Имя фазы сценария (`baseline`, `af_trigger_center`, ...). */
    val phase: String,
    val kind: Kind,
    /** Номер круга для пачек и AF-триггеров (0 — не применимо). */
    val round: Int = 0,
    /** Индекс запроса внутри пачки (0 — не применимо). */
    val index: Int = 0,
    /** Выдержка, которую просили в этом запросе (для проверки точности), если задавали. */
    val requestedExposureNs: Long? = null,
) {
    enum class Kind { REPEATING, TRIGGER, CANCEL, BURST }
}
