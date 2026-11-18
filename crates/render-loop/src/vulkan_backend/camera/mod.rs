//! Показ кадров камеры: видоискатель через ISP на GPU (сессия 07) или отладочная мозаика (сессия 05).
//!
//! # Кто чем владеет
//! * [`CameraSession`] (`session`) — состояние показа одной surface: учёт лизингов, серии сабмитов,
//!   экспортируемый семафор, импортёр, параметры ISP, привязка статистики AWB к кадрам. Единственный,
//!   кто решает, когда кадр можно вернуть камере;
//! * [`CameraPipelines`] (`pipelines`) — объекты GPU, записанные в кадры цели: ISP видоискателя,
//!   компоновщик, отладочная мозаика, метки времени кадра; запись команд кадра видоискателя;
//! * `white_balance` — наш AWB на потоке рендера (сессия 08): белая сцены, холодный старт, расписание статистики;
//! * `isp_params` — кэш `IspParams` (пересчёт только при смене входов); `placement` — где на экране
//!   изображение; `settings` — что меняют команды; `viewfinder_stats` — диагностика видоискателя;
//! * `headroom_bench` — стенд «запас под видео» (V4): нагрузка полного кадра над тем же RAW в том же
//!   сабмите, её ленивое создание и освобождение по завершённым сериям; `headroom_stats` — его счётчики;
//! * [`lease::LeaseLedger`] — правила «что и когда возвращать» (нейтральный, без Vulkan);
//! * [`submissions::SubmissionTracker`] — когда GPU точно закончил (нейтральный);
//! * [`CameraFrameImporter`] — шов импорта буфера камеры в Vulkan: на телефоне `AHardwareBuffer`
//!   без копий (`android_importer`), на ПК — синтетические источники теста;
//! * `passport` — перевод паспорта кадра в параметры предпросмотра.
//!
//! # Инварианты
//! 1. Лизинг возвращается камере только вместе с fence сабмита, который уже не читает буфер
//!    (или после подтверждённого простоя GPU); никогда — «просто так».
//! 2. Кадр мозаики, который читал последний принятый сабмит, не возвращается: он может быть
//!    перерисован. Кадр ISP читается ровно одним сабмитом и возвращается с fence ЭТОГО сабмита —
//!    на экране дальше живёт изображение видоискателя, RAW больше не нужен.
//! 3. Выброшенный лизинг при зависшем GPU — намеренная учтённая утечка, а не возврат.

mod export_recovery;
mod headroom_bench;
mod headroom_stats;
mod isp_params;
mod lease;
mod ownership;
mod passport;
mod pipelines;
mod placement;
mod session;
mod settings;
mod submissions;
mod viewfinder_stats;
mod white_balance;

#[cfg(target_os = "android")]
mod android_importer;

use std::sync::Arc;

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
pub use gpu_vulkan::hardware_buffer_cache::SubmissionSerial;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use raw_frame_stream::{RawFrameLease, ViewfinderFrames};

pub(super) use self::headroom_bench::HeadroomBenchParts;
pub(super) use self::headroom_stats::HeadroomLiveState;
pub(super) use self::pipelines::CameraPipelines;
pub(super) use self::session::{CameraDrawError, CameraSession, CameraSessionFailure, CameraSessionParts};
pub(super) use self::settings::ViewfinderSettings;
pub(super) use self::viewfinder_stats::ViewfinderStats;
pub(super) use self::white_balance::{SceneWhiteMemory, WhiteBalanceParts};

#[cfg(target_os = "android")]
pub use self::android_importer::AndroidCameraImporter;

/// Не удалось подготовить буфер камеры к рисованию.
#[derive(Debug, thiserror::Error)]
pub enum CameraImportError {
    /// Ошибка GPU при импорте.
    #[error("ошибка GPU при импорте кадра камеры: {0}")]
    Gpu(#[from] GpuError),
    /// Раскладка буфера не согласуется с паспортом или размером аллокации — догадываться нельзя.
    #[error("раскладка буфера камеры не определена: {0}")]
    Layout(String),
    /// Прочая ошибка платформенного импорта (описание уже содержит причину).
    #[error("импорт буфера камеры: {0}")]
    Platform(String),
}

/// Шов импорта буфера камеры в GPU. Реализации: `AndroidCameraImporter` (телефон, без копий) и
/// тестовая на ПК. Объекты Vulkan, на которые ссылается [`MosaicSource`], принадлежат импортёру и
/// живут, пока он их не освободит (`retire_completed` / `release_all_imports_gpu_idle`).
pub trait CameraFrameImporter: Send {
    /// Импортирует буфер лизинга (или находит уже импортированный) для сабмита серии `submission`.
    fn import_frame(
        &mut self,
        lease: &RawFrameLease,
        submission: SubmissionSerial,
    ) -> Result<MosaicSource, CameraImportError>;

    /// GPU закончил сабмиты до `completed` включительно: можно уничтожить ставшие ненужными импорты.
    fn retire_completed(&mut self, completed: SubmissionSerial);

    /// GPU подтверждённо простаивает: уничтожить ВСЕ импорты.
    fn release_all_imports_gpu_idle(&mut self);

    /// Сколько импортов сейчас живо.
    fn imports_alive(&self) -> usize;

    /// Счётчики кэша импортов (попадания и промахи).
    fn cache_counters(&self) -> ImportCacheCounters;
}

/// Фабрика импортёра: вызывается при каждом присоединении surface, получает контекст GPU бэкенда.
pub type CameraImporterFactory =
    Box<dyn Fn(&GpuContext) -> Result<Box<dyn CameraFrameImporter>, CameraImportError> + Send>;

/// Откуда бэкенд берёт кадры камеры вместо тестового паттерна.
pub struct CameraFeed {
    /// Видоискатель потока кадров (общий для всех присоединений surface).
    pub(super) frames: Arc<ViewfinderFrames>,
    /// Создаёт импортёр под каждую surface.
    pub(super) make_importer: CameraImporterFactory,
}

impl CameraFeed {
    /// Кадры берутся из `frames`, импорт — реализацией, которую создаёт `make_importer`.
    pub fn new(frames: ViewfinderFrames, make_importer: CameraImporterFactory) -> Self {
        Self {
            frames: Arc::new(frames),
            make_importer,
        }
    }
}
