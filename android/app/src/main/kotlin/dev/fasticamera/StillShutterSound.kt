package dev.fasticamera

import android.media.MediaActionSound

/**
 * Звук затвора (Rust: команда `PLAY_SHUTTER_SOUND` потока `fasticamera-platform`, трейт
 * `ShutterSoundService`). Только вызов API платформы: звук играет система по своим правилам
 * (громкость и региональные ограничения).
 */
class StillShutterSound {
    private val sound = MediaActionSound()

    init {
        // Загрузка асинхронная; `play` до её конца ставит звук в очередь самой платформы.
        sound.load(MediaActionSound.SHUTTER_CLICK)
    }

    fun play() = sound.play(MediaActionSound.SHUTTER_CLICK)

    /** Отпускает звуковые ресурсы (Activity уничтожается). */
    fun release() = sound.release()
}
