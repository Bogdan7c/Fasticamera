//! Контракт бэкенда показа: всё, что поток рендера знает о GPU.
//!
//! Поток зависит только от этого трейта (а не от `gpu-vulkan`): настоящий бэкенд живёт
//! в `vulkan_backend` и сам переводит свои типы в нейтральные структуры отсюда; в тестах
//! подставляется fake. Исходы разведены по смыслу (успех / пересоздать / surface потеряна /
//! таймаут / ошибка) и не сливаются в `bool`: поток реагирует на каждый по-своему.

use std::collections::BTreeMap;
use std::time::Duration;

use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::{NativeSurface, SurfaceSize};

/// Что известно потоку о кадре, который нужно нарисовать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTiming {
    /// Порядковый номер кадра, возрастает на каждый вызов `render_and_present` (с нуля).
    pub frame_index: u64,
    /// Номер тика vsync, по которому рисуется кадр.
    pub vsync_sequence: u64,
    /// Когда ожидается показ кадра (`desiredPresentTime` для `VK_GOOGLE_display_timing`).
    pub expected_present: MonotonicNanos,
    /// Сколько тиков vsync пропущено между предыдущим кадром и этим.
    pub skipped_vsyncs: u64,
}

/// Результат попытки нарисовать и показать кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentOutcome {
    /// Кадр поставлен на показ.
    Presented,
    /// Swapchain устарел (`SUBOPTIMAL`/`OUT_OF_DATE`/смена размера): нужно пересоздать
    /// цепочку для текущего размера. Кадр не показан.
    NeedsRecreate,
    /// Surface потеряна (`ERROR_SURFACE_LOST_KHR`): бэкенд её больше использовать не может.
    SurfaceLost,
    /// Картинка swapchain не освободилась за `acquire_timeout`: кадр пропущен, это не ошибка.
    TimedOut,
    /// Другая ошибка кадра (не потеря surface). Кадр не показан; поток считает ошибку.
    Failed(BackendError),
}

/// Ошибка бэкенда с человекочитаемой причиной (для статуса и логов).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {reason}")]
pub struct BackendError {
    /// Что делали («create_swapchain», «queue_present»…).
    pub operation: String,
    /// Что пошло не так.
    pub reason: String,
}

/// Отказ `attach`. К моменту возврата бэкенд ничего не держит: переданная surface уже
/// уничтожена (отпущена), swapchain не создан.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
    /// Surface непригодна для показа (например, окно уже умерло).
    #[error("surface не принята бэкендом: {0}")]
    SurfaceRejected(BackendError),
    /// GPU не смог создать цепочку показа.
    #[error("не удалось создать цепочку показа: {0}")]
    SwapchainFailed(BackendError),
}

/// Момент реального показа кадра (по `VK_GOOGLE_display_timing`, `CLOCK_MONOTONIC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PastPresentation {
    /// Номер показа в цепочке бэкенда: растёт на 1 за каждый показанный кадр (циклический
    /// `u32`), начинается заново после пересоздания/присоединения цепочки. Соседние номера —
    /// соседние кадры; по нему пейсинг отличает честный интервал от дыры в записях драйвера.
    pub present_id: u32,
    /// Номер кадра потока, который запросил показ.
    pub frame_index: u64,
    /// Когда кадр реально появился на экране.
    pub actual_present: MonotonicNanos,
}

/// Что бэкенд знает о текущей цепочке показа. Нейтральные типы: форматы и повороты —
/// именами (для логов и отчётов смоков), чтобы поток и сценарии не зависели от `gpu-vulkan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    /// Имя формата картинок swapchain (например, `B8G8R8A8_UNORM`).
    pub format: String,
    /// Имя цветового пространства (например, `SRGB_NONLINEAR_KHR`).
    pub color_space: String,
    /// Физический размер картинок (с учётом поворота дисплея).
    pub extent: SurfaceSize,
    /// Имя поворота дисплея, который учитывает рисование (`Identity`, `Rotate90`…).
    pub transform: String,
    /// Сколько картинок в цепочке.
    pub image_count: usize,
    /// Сколько раз цепочка пересоздавалась с момента `attach`.
    pub recreation_count: u64,
    /// Реальный период обновления экрана, если платформа его сообщает
    /// (`VK_GOOGLE_display_timing`); `None` — неизвестен (ПК-стенд). Пейсинг судит по нему,
    /// а `preferred_frame_rate_hz` берёт только как запасной вариант.
    pub refresh_cycle: Option<Duration>,
}

/// Снимок диагностики бэкенда: текущие значения счётчиков и параметров цепочки.
/// Времена показов сюда не входят: их вычерпывает [`DisplayBackend::drain_past_presentations`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendDiagnostics {
    /// Сколько Vulkan-объектов (или их аналогов) сейчас живо у бэкенда; после `detach`
    /// должно вернуться к исходному значению.
    pub live_objects: u64,
    /// Те же живые объекты в разрезе вида (`Fence`, `Image`, …); виды с нулём не перечисляются.
    /// Нужны, чтобы сравнивать состояние «в покое» целиком, а не только сумму: утечка одного
    /// вида не должна прятаться за освобождением другого.
    pub live_objects_by_kind: BTreeMap<String, u64>,
    /// Накопленное число ошибок validation.
    pub validation_errors: u64,
    /// Накопленное число предупреждений validation.
    pub validation_warnings: u64,
    /// Сколько раз `detach` не смог корректно уничтожить цепочку (GPU не успел за таймаут
    /// и т. п.). Окно при этом всё равно возвращено, но объекты GPU могли остаться.
    pub detach_failures: u64,
    /// Параметры текущей цепочки показа; `None` — surface не присоединена.
    pub display: Option<DisplayInfo>,
    /// Сколько раз сработал каждый ЗАДОКУМЕНТИРОВАННЫЙ известный отход validation (ключ — VUID,
    /// 0 — не срабатывал). Справочные числа: ошибкой не считаются.
    pub known_deviations: BTreeMap<String, u64>,
}

/// Бэкенд показа. Все методы вызываются из одного потока рендера (но сам объект может быть
/// создан в другом — отсюда `Send`).
pub trait DisplayBackend: Send + 'static {
    /// Принимает surface и создаёт цепочку показа заданного размера. При ошибке — см.
    /// [`AttachError`]: бэкенд ничего не удерживает.
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError>;

    /// Пересоздаёт цепочку под новый размер той же surface.
    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError>;

    /// Рисует и показывает один кадр.
    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome;

    /// Дожидается GPU, уничтожает цепочку и возвращает surface. `None` — бэкенд ничего
    /// не держал (повторный вызов безопасен). После возврата окно можно отпускать.
    fn detach(&mut self) -> Option<NativeSurface>;

    /// Снимок диагностики (счётчики, параметры цепочки, период экрана); вызывается по таймеру.
    fn diagnostics(&mut self) -> BackendDiagnostics;

    /// Вычерпывает НОВЫЕ записи о реальных показах кадров (каждая выдаётся один раз).
    /// Дёшево, вызывается потоком на каждом кадре: драйвер хранит лишь короткую историю
    /// времён показа, и редкий опрос теряет записи. Пустой список — новых записей нет
    /// (или цепочки нет / расширения нет).
    fn drain_past_presentations(&mut self) -> Vec<PastPresentation>;
}
