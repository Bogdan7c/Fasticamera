//! Диагностика видоискателя: что он рисует, сколько это стоит GPU и чем кончались параметры ISP.
//!
//! Приходит в [`crate::thread::RenderStatus`] через `BackendDiagnostics::viewfinder`. Счётчики
//! накопительные за жизнь бэкенда (переживают пересоздание surface), окна времени GPU —
//! скользящие (последние `viewfinder.gpu_time_window_frames` замеров).

use std::sync::Arc;

use isp_vulkan::viewfinder::IspStageSet;
use look_contract::{LookSettings, ToneBaseModel};

use super::auto_exposure::AutoExposureDiagnostics;
use super::continuity::ContinuityDiagnostics;
use super::control::ViewfinderPainter;
use super::framing::EffectivePlacement;
use super::headroom::HeadroomBenchDiagnostics;
use super::scope_diagnostics::ScopeDiagnostics;
use super::unchanged_frames::UnchangedFrameCounters;
use super::white_balance::WhiteBalanceDiagnostics;
use super::work_suppression::GpuWorkSuppression;
use crate::thread::DurationSummary;
use crate::ui_overlay::UiOverlayDiagnostics;

/// В каком цветовом пространстве экрана кодируется выход ISP (по цепочке показа).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewfinderColorSpace {
    /// sRGB.
    Srgb,
    /// Display P3 (кривая sRGB, основные цвета P3).
    DisplayP3,
}

/// Формат изображения видоискателя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewfinderStorage {
    /// 10 бит UNORM на канал (4 байта на пиксель).
    TenBitUnorm,
    /// fp16 на канал (8 байт на пиксель).
    HalfFloat,
}

/// Размер изображения видоискателя, пиксели (прямая ориентация).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewfinderExtent {
    /// Ширина.
    pub width: u32,
    /// Высота.
    pub height: u32,
}

/// Скользящие окна времени GPU. `None` — замеров ещё нет (или очередь не умеет метки времени).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewfinderGpuTimes {
    /// Сам compute-проход ISP (метки внутри `ViewfinderIsp`, только dispatch).
    pub isp_dispatch: Option<DurationSummary>,
    /// Отрезок ISP в командах кадра: приём кадра у камеры + проход + возврат камере. Со включённым
    /// стендом V4 сюда входит и его работа (он читает кадр до возврата камере); цена самого стенда —
    /// в `ViewfinderDiagnostics::headroom`.
    pub frame_isp_span: Option<DurationSummary>,
    /// Компоновка изображения видоискателя на экран.
    pub compose: Option<DurationSummary>,
    /// Весь кадр видоискателя: от начала наших команд до конца компоновки. Начальная метка ждёт
    /// окончания предыдущих команд очереди, поэтому в кадр может войти ожидание fence камеры.
    pub frame_total: Option<DurationSummary>,
}

/// Счётчики видоискателя (накопительные).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewfinderCounters {
    /// Сколько новых кадров камеры прошли через ISP (записаны в сабмит, принятый очередью).
    pub frames_processed: u64,
    /// Сколько раз кадр экрана собран из прежнего изображения видоискателя без ISP и без
    /// повторного импорта (нового кадра камеры не было).
    pub recomposed: u64,
    /// Сколько раз запись ISP отказала (кадр всё равно возвращён камере этим же сабмитом).
    pub isp_record_failures: u64,
    /// Причина последнего отказа записи ISP.
    pub last_isp_failure: Option<String>,
    /// Сколько раз отказала компоновка (вместо неё — чёрный кадр).
    pub compose_failures: u64,
    /// Сколько раз параметры ISP посчитаны заново (входы изменились).
    pub params_resolved: u64,
    /// Сколько кадров взяли прежние параметры, потому что входы те же (кэш).
    pub params_unchanged: u64,
    /// Сколько кадров без метаданных (`FrameCaptureMeta::Missing`) взяли прежние параметры.
    pub params_reused_without_meta: u64,
    /// Сколько кадров взяли прежние параметры после неудачного пересчёта.
    pub params_reused_after_failure: u64,
    /// Сколько пересчётов параметров не удалось (описание сенсора, нейтраль, уровни, матрицы).
    pub params_failed: u64,
    /// Причина последней неудачи пересчёта.
    pub last_params_failure: Option<String>,
    /// Сколько новых кадров не показано, потому что параметров ещё не было ни разу (цвета не
    /// выдумываем — решение 6): лизинг возвращён нетронутым, на экране чёрный.
    pub frames_dropped_no_params: u64,
    /// Сколько кадров отвергнуто до ISP из-за геометрии (кадрирование не влезло и т. п.).
    pub frames_rejected_geometry: u64,
    /// Сколько статистик AWB опубликовано.
    pub statistics_published: u64,
    /// Сколько кадров камеры пришли из брекетинга (`RequestOrigin::Burst`) и возвращены камере без показа
    /// и без AWB: у кадров серии разные выдержки, на экране они мигали бы. В «ровно один раз» эти кадры
    /// НЕ пропуски — они учтены здесь.
    pub bracket_frames_skipped: u64,
    /// Сколько кадров разового запроса со вспышкой (`RequestOrigin::FlashStill`) возвращены камере без показа:
    /// это кадры снимка, их забирает кольцо снимков, на экране они были бы одиночной белой вспышкой (14d).
    pub flash_frames_skipped: u64,
    /// Сколько кадров, освещённых светодиодом (фонарик предвспышки, частичная или неопознанная вспышка), ПОКАЗАНЫ,
    /// но не отданы AE и AWB видоискателя: иначе экспозиция и белая уехали бы к свету светодиода (14d).
    pub led_lit_frames_withheld_from_smoothing: u64,
    /// Сколько кадров обработано ISP, но скрыто на холодном старте AWB (решение 65): `frames_processed =
    /// показанные новые кадры + эти`.
    pub frames_hidden_cold_start: u64,
    /// Сколько статистик AWB выброшено: кадр слота не дошёл до GPU (сабмит не принят), и чей
    /// это кадр, неизвестно — публиковать чужие суммы нельзя.
    pub statistics_discarded: u64,
    /// Сколько раз не удалось записать или прочитать метки времени GPU.
    pub gpu_timing_failures: u64,
    /// Сколько времён кадра на GPU вытеснено из очереди к потоку рендера (поток не вычерпывал их;
    /// в штатной работе 0 — поток забирает их каждый кадр).
    pub gpu_timings_overflowed: u64,
    /// Копии кадра на CPU. Всегда 0: RAW попадает в GPU без копий (импорт `AHardwareBuffer`).
    pub cpu_copies: u64,
    /// Решения «рисовать ли тик» (решение 214, 16a): пропущенные тики и причины, по которым остальные нарисованы.
    pub unchanged_frames: UnchangedFrameCounters,
}

/// Состояние видоискателя.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewfinderDiagnostics {
    /// Чем рисует.
    pub painter: ViewfinderPainter,
    /// Включённые стадии ISP.
    pub stages: IspStageSet,
    /// Вид картинки, действующий на видоискателе (решение 178): по нему сквозной тест видит, что стартовый вид
    /// дошёл до рендера с первого кадра, а команда `SetLook` применилась.
    pub look: Arc<LookSettings>,
    /// Модель основы тона, под которую считаются параметры кадра (решение 195): по ней сквозной тест видит, что
    /// стартовая основа дошла до рендера, а `SetToneBase` применилась. Берётся из кэша параметров, как `look`.
    pub tone_base: ToneBaseModel,
    /// Действующее место видоискателя на экране: правило конфига или прямоугольник из `SetFraming`
    /// (по нему smoke видит, что команда экрана камеры дошла до рендера).
    pub placement: EffectivePlacement,
    /// Пространство цепочки показа на последнем кадре (`None` — кадров не было).
    pub display_color_space: Option<ViewfinderColorSpace>,
    /// Формат изображения видоискателя (`None` — ISP ещё не работал).
    pub storage: Option<ViewfinderStorage>,
    /// Размер изображения видоискателя (`None` — ISP ещё не работал).
    pub output_extent: Option<ViewfinderExtent>,
    /// Время GPU.
    pub gpu_time: ViewfinderGpuTimes,
    /// Счётчики.
    pub counters: ViewfinderCounters,
    /// Стенд «запас под видео» (V4): включён ли, сколько кадров записал, время GPU по проходам.
    pub headroom: HeadroomBenchDiagnostics,
    /// Наш AWB (сессия 08).
    pub white_balance: WhiteBalanceDiagnostics,
    /// Наш AE (сессия 08a); `None` — ящики AE не переданы (вендорский AE) или сессии показа нет.
    pub auto_exposure: Option<AutoExposureDiagnostics>,
    /// UI и стекло поверх видоискателя (сессия 11): источник, счётчики, время GPU. Пока источник не
    /// установлен, всё пусто (`UiOverlayDiagnostics::default()`).
    pub ui_overlay: UiOverlayDiagnostics,
    /// Непрерывность картинки при смене объектива (сессия 13): удержания, метки кадров, кадрирование кадров.
    pub continuity: ContinuityDiagnostics,
    /// Цветовые приборы (14h): запрос, объекты GPU, счётчики, время GPU, отзывы запроса.
    pub scopes: ScopeDiagnostics,
    /// Какие работы GPU сейчас подавлены замером A/B (16a); `NONE` — продукт. По нему сценарий замера сверяет, что
    /// «состояние окна применилось», а не просто команда доставлена.
    pub gpu_work_suppression: GpuWorkSuppression,
}
