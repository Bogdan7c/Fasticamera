package dev.fasticamera

import android.content.ContentValues
import android.content.Context
import android.content.ContentResolver
import android.content.ContentUris
import android.net.Uri
import android.os.Bundle
import android.os.Process
import android.os.SystemClock
import android.provider.MediaStore
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Хранилище фото для снимка (Rust: `android-shell::android::still_media_store`, трейт
 * `shell_contract::still_media::MediaStoreService`).
 *
 * Здесь только вызовы API платформы, без логики снимка: что писать и когда публиковать, решает Rust.
 * Файл создаётся скрытым (`IS_PENDING = 1`), пока Rust не вызовет [publish]; недописанное галерея не видит.
 *
 * Все методы вызываются с рабочего потока записи снимка (не главного); любое исключение уходит в Rust
 * и становится `MediaStoreError::Platform` — здесь оно не глотается.
 *
 * Коллекция — `MediaStore.Images`: и JPEG, и HEIC, и DNG (`image/x-adobe-dng`) — «image/…» (любой тип изображения). Каталог
 * `DCIM/Camera` разрешён для коллекции изображений.
 */
class StillMediaStore(private val context: Context) {

    /**
     * Создаёт скрытый файл и возвращает его адрес. Исключение — если хранилище отказало
     * (`insert` вернул `null` тоже считается отказом: файл не создан).
     */
    fun createPending(displayName: String, mimeType: String, relativePath: String, dateTakenMillis: Long): String {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeType)
            put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath)
            put(MediaStore.MediaColumns.DATE_TAKEN, dateTakenMillis)
            put(MediaStore.MediaColumns.IS_PENDING, PENDING)
        }
        val uri = context.contentResolver.insert(imagesCollection(), values)
            ?: throw IllegalStateException("MediaStore.insert вернул null для $displayName")
        return uri.toString()
    }

    /**
     * Открывает файл черновика на запись («w» — с усечением) и отдаёт Rust сырой дескриптор:
     * `detachFd()` снимает владение с `ParcelFileDescriptor`, закрывает дескриптор уже Rust.
     */
    fun openWriteFd(uri: String): Int {
        val descriptor = context.contentResolver.openFileDescriptor(Uri.parse(uri), "w")
            ?: throw IllegalStateException("openFileDescriptor(w) вернул null для $uri")
        return descriptor.detachFd()
    }

    /** Делает файл видимым в галерее. Ни одной обновлённой строки — ошибка (файл исчез). */
    fun publish(uri: String) {
        val values = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, PUBLISHED) }
        val updatedRows = context.contentResolver.update(Uri.parse(uri), values, null, null)
        check(updatedRows == 1) { "publish: обновлено $updatedRows строк вместо 1 для $uri" }
    }

    /** Удаляет черновик. Ни одной удалённой строки — ошибка (черновика уже нет — Rust хочет знать). */
    fun discard(uri: String) {
        val deletedRows = context.contentResolver.delete(Uri.parse(uri), null, null)
        check(deletedRows == 1) { "discard: удалено $deletedRows строк вместо 1 для $uri" }
    }

    /**
     * Открывает файл черновика для кодера HEIF. `HeifWriter` пишет контейнер с перемоткой, поэтому
     * режим «rw» (без усечения: пустой файл уже создан). Закрывает дескриптор вызывающий.
     */
    internal fun openEncoderDescriptor(uri: String) =
        context.contentResolver.openFileDescriptor(Uri.parse(uri), "rw")
            ?: throw IllegalStateException("openFileDescriptor(rw) вернул null для $uri")

    /**
     * Уборка осиротевших черновиков: удаляет НАШИ скрытые записи (`IS_PENDING = 1`), оставшиеся от прошлого
     * процесса (снимок, брошенный при убийстве процесса). Запускается РОВНО ОДИН РАЗ на процесс: `ShellPlatform`
     * пересоздаётся с каждой Activity, а повторная уборка удалила бы черновик снимка, который пишется сейчас.
     * Работает на своём потоке (запросы к MediaStore — binder, главный поток не ждёт); итог читает Rust через
     * [orphanSweepReport] и пишется в logcat.
     */
    fun startOrphanSweepOnce() {
        if (!sweepStarted.compareAndSet(false, true)) return
        sweepReport = REPORT_RUNNING
        Thread({
            sweepReport = try {
                val removed = sweepOrphanedDrafts()
                Log.i(LOG_TAG, "orphan sweep: removed $removed stale pending drafts")
                "$REPORT_REMOVED_PREFIX$removed"
            } catch (error: Exception) {
                Log.e(LOG_TAG, "orphan sweep failed", error)
                "$REPORT_FAILED_PREFIX${error.javaClass.simpleName}: ${error.message}"
            }
        }, "fasticamera-orphan-sweep").start()
    }

    /** Состояние уборки текстом для Rust: `not_started` | `running` | `removed:N` | `failed:текст`. */
    fun orphanSweepReport(): String = sweepReport

    /**
     * Находит и удаляет черновики: владелец — наш пакет, `IS_PENDING = 1` и создан ДО старта процесса
     * (`DATE_ADDED` — секунды; строго меньше секунды старта, так что свежий черновик этого процесса не затронут).
     * Возвращает число удалённых записей. Не удалось удалить запись — исключение (итог `failed`, не тихий пропуск).
     */
    private fun sweepOrphanedDrafts(): Int {
        val resolver = context.contentResolver
        val processStartSeconds = processStartEpochSeconds()
        val queryArgs = Bundle().apply {
            // Скрытые записи по умолчанию запросом не видны: просим ТОЛЬКО их.
            putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_ONLY)
            putString(
                ContentResolver.QUERY_ARG_SQL_SELECTION,
                "${MediaStore.MediaColumns.OWNER_PACKAGE_NAME} = ? AND ${MediaStore.MediaColumns.IS_PENDING} = 1 " +
                    "AND ${MediaStore.MediaColumns.DATE_ADDED} < ?",
            )
            putStringArray(
                ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS,
                arrayOf(context.packageName, processStartSeconds.toString()),
            )
        }
        val collection = imagesCollection()
        val orphanIds = mutableListOf<Long>()
        resolver.query(collection, arrayOf(MediaStore.MediaColumns._ID), queryArgs, null)?.use { cursor ->
            while (cursor.moveToNext()) orphanIds += cursor.getLong(0)
        } ?: throw IllegalStateException("MediaStore.query вернул null")
        var removed = 0
        for (id in orphanIds) {
            val deletedRows = resolver.delete(ContentUris.withAppendedId(collection, id), null, null)
            check(deletedRows == 1) { "sweep: удалено $deletedRows строк вместо 1 для id=$id" }
            removed += 1
        }
        return removed
    }

    private fun imagesCollection(): Uri = MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

    private companion object {
        const val PENDING = 1
        const val PUBLISHED = 0
        const val LOG_TAG = "Fasticamera"
        const val REPORT_NOT_STARTED = "not_started"
        const val REPORT_RUNNING = "running"
        const val REPORT_REMOVED_PREFIX = "removed:"
        const val REPORT_FAILED_PREFIX = "failed:"

        /** Уборка запускалась в этом процессе (общая для всех экземпляров: Activity пересоздаётся). */
        val sweepStarted = AtomicBoolean(false)

        /** Итог уборки, виден Rust; пишет поток уборки. */
        @Volatile
        var sweepReport: String = REPORT_NOT_STARTED

        /** Момент старта процесса в секундах Unix-времени. */
        fun processStartEpochSeconds(): Long {
            val sinceStartMillis = SystemClock.elapsedRealtime() - Process.getStartElapsedRealtime()
            return (System.currentTimeMillis() - sinceStartMillis) / 1000
        }
    }
}
