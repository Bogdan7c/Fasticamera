//! Платформенные сервисы снимка (сессия 14): хранилище фото, кодер HEIF с входной поверхностью,
//! сжатие JPEG, звук затвора.
//!
//! Реализация на телефоне — `android-shell` (MediaStore, androidx `HeifWriter`, `Bitmap.compress`,
//! `MediaActionSound`), в тестах — fake-реализации. Логики снимка здесь нет: что писать и в каком
//! порядке, решает `photo-output`.
//!
//! Поток вызова: все методы, кроме [`ShutterSoundService::play_shutter_sound`], синхронные и могут
//! ждать платформу (binder, кодер). Их зовёт только рабочий поток записи снимка — никогда главный
//! поток Android и никогда поток рендера (инвариант 4 README).

use std::time::Duration;

use crate::commands::CommandOutcome;
use crate::surface::NativeSurface;

/// Запрос черновика в хранилище фото.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMediaRequest {
    /// Имя файла с расширением, например `IMG_20261122_101530_123.HEIC`.
    pub display_name: String,
    /// MIME-тип (`image/heic`, `image/jpeg`, `image/x-adobe-dng`).
    pub mime_type: String,
    /// Каталог относительно корня хранилища, например `DCIM/Camera`.
    pub relative_path: String,
    /// Когда снято, миллисекунды Unix-времени (сортировка в галерее).
    pub date_taken_millis: i64,
}

/// Ошибка хранилища.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MediaStoreError {
    /// Платформа ещё не подключена (нет JVM/контекста приложения).
    #[error("хранилище недоступно: платформа не подключена")]
    NotAttached,
    /// Платформа отказала (текст исключения).
    #[error("хранилище отказало: {0}")]
    Platform(String),
    /// Ошибка записи байтов в открытый файл.
    #[error("ошибка записи файла: {0}")]
    Io(String),
}

/// Опубликованный файл: виден в галерее.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedMedia {
    /// Адрес в хранилище (`content://media/...`).
    pub uri: String,
}

/// Черновик в хранилище: файл уже существует, но галерея его не видит (MediaStore `IS_PENDING = 1`),
/// пока не вызван [`PendingMedia::publish`]. Частичный файл в галерее не появляется никогда.
///
/// Владение: черновик либо публикуется, либо удаляется ([`PendingMedia::discard`]). Реализация обязана
/// удалить черновик и при выбрасывании без явного итога — недописанный файл не должен остаться.
pub trait PendingMedia: Send {
    /// Адрес черновика (для логов и для открытия кодером).
    fn uri(&self) -> &str;

    /// Дописать байты в конец файла (DNG, JPEG — их пишет Rust).
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), MediaStoreError>;

    /// Сделать файл видимым в галерее. Записанное кодером к этому моменту должно быть закрыто.
    fn publish(self: Box<Self>) -> Result<PublishedMedia, MediaStoreError>;

    /// Удалить черновик (снимок сорвался).
    fn discard(self: Box<Self>);
}

/// Хранилище фото (MediaStore на телефоне).
pub trait MediaStoreService: Send + Sync {
    /// Создать черновик файла.
    fn create_pending(&self, request: &PendingMediaRequest) -> Result<Box<dyn PendingMedia>, MediaStoreError>;
}

/// Ошибка кодера.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    #[error("кодер недоступен: платформа не подключена")]
    NotAttached,
    /// Платформа отказала (исключение кодера).
    #[error("кодер отказал: {0}")]
    Platform(String),
    /// Кодер не закончил за отведённое время.
    #[error("кодер не закончил за {0:?}")]
    TimedOut(Duration),
    /// Неверные параметры (размер, качество, буфер).
    #[error("неверные параметры кодера: {0}")]
    InvalidInput(String),
}

/// Настройки одного HEIF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeifEncodeSettings {
    pub width: u32,
    pub height: u32,
    /// Поворот к «как держали телефон», градусы по часовой (0/90/180/270): пишется тегом, пиксели не вращаются.
    pub rotation_degrees: u32,
    /// Качество кодера 0–100 (конфиг).
    pub quality: u8,
    /// Блок Exif в формате TIFF (порядок байт и IFD0 — как в APP1 JPEG, без заголовка `Exif\0\0`).
    pub exif_tiff: Option<Vec<u8>>,
}

/// Открытый кодер HEIF одного снимка (решение 142): рендер рисует ОДИН кадр в его входную поверхность.
pub trait HeifSurfaceEncoder: Send {
    /// Входная поверхность кодера. Отдаётся один раз (второй вызов — `None`).
    fn take_input_surface(&mut self) -> Option<NativeSurface>;

    /// Кадр показан в поверхность: дописать файл и закрыть кодер, ожидая не дольше `timeout`.
    fn finish(self: Box<Self>, timeout: Duration) -> Result<(), EncodeError>;

    /// Бросить кодирование (снимок сорвался). Файл после этого не годен — черновик удаляют.
    fn abort(self: Box<Self>);
}

/// Фабрика кодеров HEIF.
pub trait HeifEncoderService: Send + Sync {
    /// Открыть кодер, пишущий в черновик `target`.
    fn open(
        &self,
        target: &dyn PendingMedia,
        settings: &HeifEncodeSettings,
    ) -> Result<Box<dyn HeifSurfaceEncoder>, EncodeError>;
}

/// Сжатие JPEG платформой (решение 142: `Bitmap.compress`).
pub trait JpegCompressor: Send + Sync {
    /// RGBA8 sRGB построчно (`width * height * 4` байт) → байты JPEG без Exif (его вставляет `photo-output`).
    fn compress(&self, rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, EncodeError>;
}

/// Звук затвора (`MediaActionSound`). Не ждёт: команда ставится в очередь платформы.
pub trait ShutterSoundService: Send + Sync {
    fn play_shutter_sound(&self) -> CommandOutcome;
}
