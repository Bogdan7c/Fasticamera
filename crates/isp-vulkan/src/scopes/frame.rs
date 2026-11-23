//! Типы кадра приборов: вход, итог записи, счётчики для диагностики.

use gpu_vulkan::gpu_timing::TimestampCollect;
use gpu_vulkan::source_window::SourceWindow;
use gpu_vulkan::storage_image::{SampledImageView, StorageImageGeneration};
use scope_contract::ScopeImageSize;
use scope_model::{GridShape, ScopeModelSettings};

use super::rule::{AccumulatorId, GpuScopeRule};

/// Показанная картинка, которую меряют приборы.
///
/// # Контракт вызывающего (render-loop)
/// * В том же командном буфере РАНЬШЕ вызова `record` картинка уже записана (выход ISP видоискателя) и
///   доступна для чтения стадией COMPUTE в раскладке `SHADER_READ_ONLY_OPTIMAL`: если её читателем был
///   фрагментный шейдер, перед `record` записывается барьер чтения для compute
///   (`BackdropSource::record_read_barrier(cb, ConsumerStage::Compute)` или ISP с `reader = Compute`).
/// * Проход картинку НЕ пишет и раскладку не меняет; следующая запись ISP и так ждёт чтений на всех
///   шейдерных стадиях.
/// * Меряется ТОЛЬКО содержимое окна (`window.content()`, левый верхний прямоугольник картинки ёмкости) и с
///   учётом зеркала (`window.mirror()`): прибор видит то, что видит владелец. Увеличение окна
///   (`magnification`, удержанный кадр при смене объектива) приборы игнорируют: они меряют
///   неувеличенное содержимое.
#[derive(Debug, Clone, Copy)]
pub struct ScopeInput {
    /// Картинка (10-бит UNORM или fp16, значения экрана с кодировкой OETF).
    pub view: SampledImageView,
    /// Окно чтения: содержимое и зеркало.
    pub window: SourceWindow,
}

/// Всё, что нужно для записи одного кадра приборов.
#[derive(Clone, Copy)]
pub struct ScopeFrame<'a> {
    /// Что мерить.
    pub input: ScopeInput,
    /// Прибор (для приборов `scope-model` — `&ResolvedScope`).
    pub rule: &'a dyn GpuScopeRule,
    /// Настройки модели: прореживание и правило показа. Размеры сеток в них обязаны укладываться в буфер
    /// счётчиков, рассчитанный при создании (иначе `ScopeError::GridExceedsCounterCapacity`).
    pub settings: &'a ScopeModelSettings,
    /// Размер картинки прибора; смена размера пересоздаёт ТОЛЬКО картинку.
    pub picture_size: ScopeImageSize,
    /// Слот кадра: его fence вызывающий дождался (или слот ещё не использовался).
    pub frame_slot: usize,
}

impl std::fmt::Debug for ScopeFrame<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScopeFrame")
            .field("input", &self.input)
            .field("accumulator", &self.rule.accumulator())
            .field("picture_size", &self.picture_size)
            .field("frame_slot", &self.frame_slot)
            .finish()
    }
}

/// Итог записи кадра приборов.
///
/// После `record` картинка прибора записана и готова к чтению ФРАГМЕНТНЫМ шейдером (раскладка
/// `SHADER_READ_ONLY_OPTIMAL`, барьер записан): её можно отдать рисованию UI как живую картинку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedScope {
    /// Картинка прибора: прямая альфа, цвета экрана, fp16.
    pub output: SampledImageView,
    /// Размер картинки прибора.
    pub size: ScopeImageSize,
    /// Поколение картинки: меняется, когда картинка пересоздана (смена размера) — рисованию UI пора
    /// обновить привязку.
    pub picture_generation: StorageImageGeneration,
    /// Форма сетки, посчитанной в этом кадре.
    pub grid: GridShape,
    /// Какой шейдер накопления её считал.
    pub accumulator: AccumulatorId,
}

/// Счётчики диагностики (растут только после успешной записи).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopePassCounters {
    /// Записанных кадров приборов.
    pub recorded: u64,
    /// Записанных dispatch накопления (кадр без отсчётов dispatch не пишет).
    pub accumulate_dispatches: u64,
    /// Записанных dispatch показа.
    pub render_dispatches: u64,
    /// Сколько раз картинка заменена другой (смена размера); первое создание не в счёт.
    pub picture_recreations: u64,
}

/// Что собрано со слота после его fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSlotReport {
    /// GPU-время проходов (отрезки «накопление», «показ»); `NotReady` — GPU ещё не дошёл, никогда не ждём.
    pub gpu_time: TimestampCollect,
}
