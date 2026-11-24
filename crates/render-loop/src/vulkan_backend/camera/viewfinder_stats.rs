//! Накопление диагностики видоискателя: счётчики, окна времени GPU, последний формат и размер.
//!
//! Владелец — бэкенд (`VulkanDisplayBackend`); на время присоединения surface объект передаётся
//! сессии показа и возвращается при её освобождении, поэтому счётчики и окна переживают
//! пересоздание surface. Метки времени собираются со слота только после его fence (никаких
//! ожиданий GPU); не готовые метки (`NotReady`) просто пропускаются.

use std::time::Duration;

use gpu_vulkan::gpu_timing::{TimestampCollect, TimestampSpans};
use gpu_vulkan::storage_image::StorageImageFormat;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_vulkan::viewfinder::{VariantCacheStats, ViewfinderImage};

use super::headroom_stats::{HeadroomBenchStats, HeadroomLiveState};
use super::isp_params::{HintsSource, ParamsDecision, ParamsOutcome, ParamsUnavailable};
use super::settings::ViewfinderSettings;
use crate::thread::RollingDurations;
use crate::ui_overlay::UiOverlayDiagnostics;
use crate::viewfinder::{
    ContinuityStats, ScopeCounters, ScopeDiagnostics, ScopeEvents, ViewfinderColorSpace, ViewfinderCounters,
    ViewfinderDiagnostics, ViewfinderExtent, ViewfinderGpuTimes, ViewfinderStorage, WhiteBalanceDiagnostics,
};

/// Точка «начало команд кадра видоискателя».
pub const POINT_FRAME_START: &str = "frame-start";
/// Точка «ISP записан и кадр возвращён камере».
pub const POINT_ISP_DONE: &str = "isp-done";
/// Точка «компоновка на экран закончена».
pub const POINT_COMPOSE_DONE: &str = "compose-done";
/// Точка «UI записан поверх компоновки» (пишется только в кадрах, где UI рисовался: отрезок до неё —
/// стоимость UI на GPU, пирамида размытия включительно).
pub const POINT_UI_DONE: &str = "ui-done";
/// Точки меток кадра в порядке записи (индексы — `FRAME_POINT_*`).
pub const FRAME_POINTS: [&str; 4] = [POINT_FRAME_START, POINT_ISP_DONE, POINT_COMPOSE_DONE, POINT_UI_DONE];
/// Индекс точки начала.
pub const FRAME_POINT_START: usize = 0;
/// Индекс точки конца ISP.
pub const FRAME_POINT_ISP_DONE: usize = 1;
/// Индекс точки конца компоновки.
pub const FRAME_POINT_COMPOSE_DONE: usize = 2;
/// Индекс точки конца UI (только кадры с UI).
pub const FRAME_POINT_UI_DONE: usize = 3;

/// Диагностика видоискателя, копящаяся за жизнь бэкенда.
#[derive(Debug)]
pub struct ViewfinderStats {
    counters: ViewfinderCounters,
    isp_dispatch: RollingDurations,
    frame_isp_span: RollingDurations,
    compose: RollingDurations,
    frame_total: RollingDurations,
    display_color_space: Option<ViewfinderColorSpace>,
    storage: Option<ViewfinderStorage>,
    output_extent: Option<ViewfinderExtent>,
    /// Стенд V4 (счётчики и окна по проходам).
    headroom: HeadroomBenchStats,
    /// Непрерывность картинки при смене объектива (сессия 13).
    continuity: ContinuityStats,
    /// Цветовые приборы (14h): счётчики и окно времени GPU. Копятся здесь, а не в цепочке показа: цепочка (и её
    /// объекты GPU) пересоздаётся, а диагностика обязана это пережить.
    scope_counters: ScopeCounters,
    scope_gpu: RollingDurations,
    /// Последний снимок кэша вариантов пайплайна ISP. Счётчики накопительные у самого ISP и НАЧИНАЮТСЯ С НУЛЯ при его
    /// пересоздании (пересоздание поверхности): здесь только последняя копия, поэтому окно отчёта сценария, внутри
    /// которого ISP пересоздавался, занижает создания (приращение насыщается до нуля).
    pipeline_variants: VariantCacheStats,
}

impl ViewfinderStats {
    /// Пустая статистика с окнами по `window_frames` замеров.
    pub fn new(window_frames: usize) -> Self {
        Self {
            counters: ViewfinderCounters::default(),
            isp_dispatch: RollingDurations::new(window_frames),
            frame_isp_span: RollingDurations::new(window_frames),
            compose: RollingDurations::new(window_frames),
            frame_total: RollingDurations::new(window_frames),
            display_color_space: None,
            storage: None,
            output_extent: None,
            headroom: HeadroomBenchStats::new(window_frames),
            continuity: ContinuityStats::default(),
            scope_counters: ScopeCounters::default(),
            scope_gpu: RollingDurations::new(window_frames),
            pipeline_variants: VariantCacheStats::default(),
        }
    }

    /// Запоминает снимок кэша вариантов пайплайна ISP (после записи кадра; снимок дешёвый — копия счётчиков).
    pub fn note_pipeline_variants(&mut self, stats: VariantCacheStats) {
        self.pipeline_variants = stats;
    }

    /// Учитывает приращения приборов, которые отдала цепочка показа (запись кадра или сбор слота).
    pub fn note_scope_events(&mut self, events: &ScopeEvents) {
        self.scope_counters.absorb(&events.counters);
        for nanos in &events.gpu_times_ns {
            self.scope_gpu.push(Duration::from_nanos(*nanos));
        }
    }

    /// Накопитель непрерывности для правки владельцем показа (сессия видоискателя).
    pub fn continuity_mut(&mut self) -> &mut ContinuityStats {
        &mut self.continuity
    }

    /// Статистика стенда V4 для правки владельцем показа.
    pub fn headroom_mut(&mut self) -> &mut HeadroomBenchStats {
        &mut self.headroom
    }

    /// Счётчики для правки владельцем показа (сессия видоискателя).
    pub fn counters_mut(&mut self) -> &mut ViewfinderCounters {
        &mut self.counters
    }

    /// Учитывает итог подготовки параметров ISP.
    pub fn note_params(&mut self, decision: &ParamsDecision) {
        let counters = &mut self.counters;
        let got_params = !matches!(decision.outcome, ParamsOutcome::Unavailable(_));
        if got_params && decision.hints == HintsSource::EarlierFrame {
            counters.params_reused_without_meta += 1;
        }
        match &decision.outcome {
            ParamsOutcome::Resolved => counters.params_resolved += 1,
            ParamsOutcome::Unchanged => counters.params_unchanged += 1,
            ParamsOutcome::ReusedAfterFailure(failure) => {
                counters.params_failed += 1;
                counters.params_reused_after_failure += 1;
                counters.last_params_failure = Some(failure.to_string());
            }
            ParamsOutcome::Unavailable(reason) => {
                counters.frames_dropped_no_params += 1;
                if let ParamsUnavailable::Failed(failure) = reason {
                    counters.params_failed += 1;
                    counters.last_params_failure = Some(failure.to_string());
                }
            }
        }
    }

    /// Пространство цепочки показа на этом кадре.
    pub fn note_display_color_space(&mut self, color_space: DisplayColorSpace) {
        self.display_color_space = Some(match color_space {
            DisplayColorSpace::Srgb => ViewfinderColorSpace::Srgb,
            DisplayColorSpace::DisplayP3 => ViewfinderColorSpace::DisplayP3,
        });
    }

    /// Формат и размер изображения видоискателя после записи кадра.
    pub fn note_isp_image(&mut self, image: &ViewfinderImage) {
        self.storage = Some(match image.format() {
            StorageImageFormat::A2B10G10R10Unorm => ViewfinderStorage::TenBitUnorm,
            StorageImageFormat::Rgba16Float => ViewfinderStorage::HalfFloat,
        });
        // Размер кадра, а не картинки: у картинки постоянной ёмкости остальное — остатки прежних кадров.
        let extent = image.content_extent();
        self.output_extent = Some(ViewfinderExtent {
            width: extent.width,
            height: extent.height,
        });
    }

    /// Время самого прохода ISP (метки `ViewfinderIsp`).
    pub fn note_isp_dispatch_time(&mut self, collected: &TimestampCollect) {
        if let TimestampCollect::Ready(spans) = collected {
            self.isp_dispatch.push(Duration::from_nanos(spans.total_ns));
        }
    }

    /// Время кадра видоискателя (свои метки кадра: см. `FRAME_POINTS`).
    pub fn note_frame_time(&mut self, collected: &TimestampCollect) {
        let TimestampCollect::Ready(spans) = collected else {
            return;
        };
        self.frame_total.push(Duration::from_nanos(spans.total_ns));
        if let Some(isp) = span_ending_at(spans, POINT_ISP_DONE) {
            self.frame_isp_span.push(isp);
        }
        if let Some(compose) = span_ending_at(spans, POINT_COMPOSE_DONE) {
            self.compose.push(compose);
        }
    }

    /// Снимок для статуса потока: действующие настройки + живое состояние объектов стенда V4
    /// (его знает только сессия показа; без сессии — «объектов нет»).
    pub fn snapshot(
        &mut self,
        settings: &ViewfinderSettings,
        headroom_live: HeadroomLiveState,
        white_balance: WhiteBalanceDiagnostics,
    ) -> ViewfinderDiagnostics {
        ViewfinderDiagnostics {
            painter: settings.painter,
            stages: settings.stages,
            look: std::sync::Arc::clone(&settings.look),
            tone_base: settings.tone_base,
            placement: settings.effective_placement(),
            display_color_space: self.display_color_space,
            storage: self.storage,
            output_extent: self.output_extent,
            gpu_time: ViewfinderGpuTimes {
                isp_dispatch: self.isp_dispatch.summary(),
                frame_isp_span: self.frame_isp_span.summary(),
                compose: self.compose.summary(),
                frame_total: self.frame_total.summary(),
            },
            counters: self.counters.clone(),
            headroom: self.headroom.snapshot(settings.headroom, headroom_live),
            white_balance,
            // Диагностику AE знает только сессия показа (у неё исполнитель): она дописывает поле сама.
            auto_exposure: None,
            // UI-проход принадлежит бэкенду, а не сессии: поле дописывает бэкенд (`UiOverlayState::snapshot`).
            ui_overlay: UiOverlayDiagnostics::default(),
            // Идёт ли удержание, знает только сессия показа: она дописывает поле `holding` сама.
            continuity: self.continuity.snapshot(false),
            // Объекты GPU приборов знает только цепочка показа, число отзывов — бэкенд: они дописывают свои поля.
            scopes: ScopeDiagnostics {
                requested: settings.scopes,
                counters: self.scope_counters.clone(),
                gpu_time: self.scope_gpu.summary(),
                ..ScopeDiagnostics::default()
            },
            gpu_work_suppression: settings.gpu_work_suppression,
            pipeline_variants: self.pipeline_variants,
        }
    }
}

/// Длительность отрезка, кончающегося точкой `point` (отрезки — между соседними записанными точками:
/// у перекомпоновки без ISP отрезок компоновки начинается от начала кадра).
pub(super) fn span_ending_at(spans: &TimestampSpans, point: &str) -> Option<Duration> {
    spans
        .spans
        .iter()
        .find(|span| span.to == point)
        .map(|span| Duration::from_nanos(span.duration_ns))
}

#[cfg(test)]
mod tests {
    use gpu_vulkan::gpu_timing::TimestampSpan;

    use super::super::isp_params::ParamsFailure;
    use super::*;
    use crate::config::ViewfinderConfig;

    fn settings() -> ViewfinderSettings {
        ViewfinderSettings::from_config(&ViewfinderConfig::default()).expect("конфиг по умолчанию корректен")
    }

    fn spans(points: &[(&'static str, &'static str, u64)]) -> TimestampCollect {
        TimestampCollect::Ready(TimestampSpans {
            spans: points
                .iter()
                .map(|(from, to, duration_ns)| TimestampSpan {
                    from,
                    to,
                    duration_ns: *duration_ns,
                })
                .collect(),
            total_ns: points.iter().map(|(_, _, duration)| duration).sum(),
        })
    }

    #[test]
    fn frame_spans_split_into_isp_compose_and_total_windows() {
        let mut stats = ViewfinderStats::new(8);
        // Новый кадр: ISP 3 мс + компоновка 1 мс.
        stats.note_frame_time(&spans(&[
            (POINT_FRAME_START, POINT_ISP_DONE, 3_000_000),
            (POINT_ISP_DONE, POINT_COMPOSE_DONE, 1_000_000),
        ]));
        // Перекомпоновка: только компоновка 2 мс.
        stats.note_frame_time(&spans(&[(POINT_FRAME_START, POINT_COMPOSE_DONE, 2_000_000)]));
        stats.note_frame_time(&TimestampCollect::NotReady);
        stats.note_isp_dispatch_time(&spans(&[("isp-start", "isp-end", 2_500_000)]));
        let snapshot = stats.snapshot(
            &settings(),
            HeadroomLiveState::default(),
            WhiteBalanceDiagnostics::idle(4),
        );
        let times = snapshot.gpu_time;
        assert_eq!(
            times.frame_total.map(|window| window.samples),
            Some(2),
            "NotReady не замер"
        );
        assert_eq!(
            times.frame_total.map(|window| window.max),
            Some(Duration::from_millis(4))
        );
        assert_eq!(times.frame_isp_span.map(|window| window.samples), Some(1));
        assert_eq!(times.compose.map(|window| window.max), Some(Duration::from_millis(2)));
        assert_eq!(
            times.isp_dispatch.map(|window| window.p50),
            Some(Duration::from_micros(2_500))
        );
    }

    #[test]
    fn pipeline_variant_stats_survive_into_the_snapshot_and_the_latest_one_wins() {
        let mut stats = ViewfinderStats::new(4);
        let snapshot_of = |stats: &mut ViewfinderStats| {
            stats
                .snapshot(
                    &settings(),
                    HeadroomLiveState::default(),
                    WhiteBalanceDiagnostics::idle(4),
                )
                .pipeline_variants
        };
        assert_eq!(
            snapshot_of(&mut stats),
            VariantCacheStats::default(),
            "до кадров — нули"
        );
        stats.note_pipeline_variants(VariantCacheStats {
            in_cache: 12,
            created_in_frame: 1,
            created_by_prewarm: 11,
            ..VariantCacheStats::default()
        });
        stats.note_pipeline_variants(VariantCacheStats {
            in_cache: 12,
            created_in_frame: 2,
            created_by_prewarm: 11,
            evicted: 1,
            ..VariantCacheStats::default()
        });
        let latest = snapshot_of(&mut stats);
        assert_eq!(
            (latest.created_in_frame, latest.created_by_prewarm, latest.evicted),
            (2, 11, 1)
        );
        assert_eq!(latest.in_cache, 12);
    }

    #[test]
    fn params_decisions_land_in_their_own_counters() {
        let mut stats = ViewfinderStats::new(4);
        let decide = |hints, outcome| ParamsDecision { hints, outcome };
        stats.note_params(&decide(HintsSource::ThisFrame, ParamsOutcome::Resolved));
        stats.note_params(&decide(HintsSource::ThisFrame, ParamsOutcome::Unchanged));
        stats.note_params(&decide(HintsSource::EarlierFrame, ParamsOutcome::Unchanged));
        stats.note_params(&decide(
            HintsSource::ThisFrame,
            ParamsOutcome::ReusedAfterFailure(ParamsFailure::InvalidNeutral("нечисловая белая".to_owned())),
        ));
        stats.note_params(&decide(
            HintsSource::EarlierFrame,
            ParamsOutcome::Unavailable(ParamsUnavailable::NoMetadataYet),
        ));
        let counters = stats
            .snapshot(
                &settings(),
                HeadroomLiveState::default(),
                WhiteBalanceDiagnostics::idle(4),
            )
            .counters;
        assert_eq!(counters.params_resolved, 1);
        assert_eq!(counters.params_unchanged, 2);
        assert_eq!(
            counters.params_reused_without_meta, 1,
            "кадр без метаданных без параметров не считается"
        );
        assert_eq!((counters.params_failed, counters.params_reused_after_failure), (1, 1));
        assert_eq!(counters.frames_dropped_no_params, 1);
        assert_eq!(
            counters.last_params_failure,
            Some(ParamsFailure::InvalidNeutral("нечисловая белая".to_owned()).to_string())
        );
        assert_eq!(counters.cpu_copies, 0);
    }
}
