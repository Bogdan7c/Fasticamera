//! Приращение диагностики снимков рендера (`RenderStatus.backend.still`) за прогон `capture`.
//!
//! Счётчики рендера накопительные за жизнь бэкенда, а сценарию нужно «что случилось ПОКА он снимал»:
//! выбор кадра по видам и причинам замены, потеря GPU снимков, отказы до захвата, итоги. Это независимая от журнала
//! выборов сверка: журнал говорит «что выбрано для каждого снимка», счётчики рендера — «что рендер учёл».

use render_loop::still_diagnostics::{FlashStillCounters, StillDiagnostics, StillStepCpu};
use serde::Serialize;

/// Выбор кадра: приращения по видам.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct PickDelta {
    pub exact: u64,
    pub substituted_timing_unknown: u64,
    /// Показанный кадр вытеснен из кольца: проверка глубины кольца (решение 139), обязана быть 0.
    pub substituted_evicted: u64,
    pub substituted_lost_before_ring: u64,
    pub substituted_without_metadata: u64,
    pub substituted_not_regular: u64,
    /// Все виды «кадра нет» вместе.
    pub no_frame: u64,
}

/// Снимки со вспышкой (14d): что рендер учёл за прогон. Доказательство «кадр освещён» на телефоне —
/// `lit_check_passed`: проверка по пикселям RAW (HAL может сообщить `FIRED` без света).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FlashDelta {
    pub begins: u64,
    pub frames_found: u64,
    pub never_arrived: u64,
    pub not_captured_by_camera: u64,
    pub white_from_torch_frame: u64,
    pub white_fallback_no_torch_estimate: u64,
    pub lit_check_passed: u64,
    pub not_lit: u64,
    /// Проверка не решаема (мала предсказанная доля вспышки, нет уровня сцены, нет отсчётов) — все причины.
    pub lit_check_skipped: u64,
}

impl FlashDelta {
    fn between(before: &FlashStillCounters, after: &FlashStillCounters) -> Self {
        let sub = |after: u64, before: u64| after.saturating_sub(before);
        let skipped = |c: &FlashStillCounters| {
            c.lit_check_skipped_low_predicted_share
                .saturating_add(c.lit_check_skipped_no_ambient_level)
                .saturating_add(c.lit_check_skipped_samples_unavailable)
        };
        Self {
            begins: sub(after.begins, before.begins),
            frames_found: sub(after.frames_found, before.frames_found),
            never_arrived: sub(after.never_arrived, before.never_arrived),
            not_captured_by_camera: sub(after.not_captured_by_camera, before.not_captured_by_camera),
            white_from_torch_frame: sub(after.white_from_torch_frame, before.white_from_torch_frame),
            white_fallback_no_torch_estimate: sub(
                after.white_fallback_no_torch_estimate,
                before.white_fallback_no_torch_estimate,
            ),
            lit_check_passed: sub(after.lit_check_passed, before.lit_check_passed),
            not_lit: sub(after.not_lit, before.not_lit),
            lit_check_skipped: sub(skipped(after), skipped(before)),
        }
    }
}

/// Приращение диагностики снимков рендера.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct StillDiagnosticsDelta {
    pub begins: u64,
    pub picks: PickDelta,
    /// Отказы до захвата, все причины вместе (подробности — в диагностике рендера).
    pub refusals: u64,
    /// GPU снимков объявлен потерянным (обязано быть 0).
    pub gpu_lost: u64,
    pub gpu_lost_by_deadline: u64,
    /// Итоги: срыв до захвата, срывы после, успешные.
    pub not_secured: u64,
    pub failed: u64,
    pub completed: u64,
    /// Точные выборы: сколько снимок стоял на экране до нажатия — число наблюдений и среднее, мс.
    pub exact_press_after_shown_count: u64,
    pub exact_press_after_shown_mean_ms: Option<f64>,
    /// CPU потока рендера по шагам движка снимков — НАКОПЛЕННОЕ за жизнь бэкенда (наибольшее из разности
    /// не вычесть). Сценарий идёт в свежем процессе, поэтому это и есть цена снимков прогона: вход 14c
    /// для вопроса «что держит поток рендера, пока видоискатель теряет кадры».
    pub step_cpu: StillStepCpu,
    /// Снимки со вспышкой (14d).
    pub flash: FlashDelta,
}

impl StillDiagnosticsDelta {
    /// Приращение от `before` до `after` (насыщение — защита от перезапуска бэкенда рендера).
    pub fn between(before: &StillDiagnostics, after: &StillDiagnostics) -> Self {
        let sub = |after: u64, before: u64| after.saturating_sub(before);
        let (b, a) = (&before.picks, &after.picks);
        let refusals_total = |diagnostics: &StillDiagnostics| {
            let r = &diagnostics.refusals;
            [
                r.queue_full,
                r.facts_no_capture_details,
                r.facts_camera_not_in_catalog,
                r.facts_crop,
                r.facts_color,
                r.gpu_unavailable,
                r.capture_start_failed,
                r.capture_failed,
                r.raw_samples_unreadable,
                r.begin_without_surface,
            ]
            .iter()
            .fold(0u64, |sum, count| sum.saturating_add(*count))
        };
        let no_frame = |p: &render_loop::still_diagnostics::PickCounters| {
            [
                p.no_frame_ring_disabled,
                p.no_frame_ring_empty,
                p.no_frame_no_eligible,
                p.no_frame_not_live_on_screen,
                p.no_frame_nothing_shown,
            ]
            .iter()
            .fold(0u64, |sum, count| sum.saturating_add(*count))
        };
        let failed = |o: &render_loop::still_diagnostics::OutcomeCounters| {
            [
                o.failed_params_unavailable,
                o.failed_gpu,
                o.failed_target_unavailable,
                o.failed_target_timed_out,
                o.failed_abandoned,
            ]
            .iter()
            .fold(0u64, |sum, count| sum.saturating_add(*count))
        };
        let (bo, ao) = (&before.outcomes, &after.outcomes);
        let shown_count = sub(
            after.exact_press_after_shown.count,
            before.exact_press_after_shown.count,
        );
        let shown_total_ms =
            (after.exact_press_after_shown.total_ms - before.exact_press_after_shown.total_ms).max(0.0);
        Self {
            begins: sub(after.begins, before.begins),
            picks: PickDelta {
                exact: sub(a.exact, b.exact),
                substituted_timing_unknown: sub(a.substituted_timing_unknown, b.substituted_timing_unknown),
                substituted_evicted: sub(a.substituted_evicted, b.substituted_evicted),
                substituted_lost_before_ring: sub(a.substituted_lost_before_ring, b.substituted_lost_before_ring),
                substituted_without_metadata: sub(a.substituted_without_metadata, b.substituted_without_metadata),
                substituted_not_regular: sub(a.substituted_not_regular, b.substituted_not_regular),
                no_frame: sub(no_frame(a), no_frame(b)),
            },
            refusals: sub(refusals_total(after), refusals_total(before)),
            gpu_lost: sub(after.gpu_lost, before.gpu_lost),
            gpu_lost_by_deadline: sub(after.gpu_lost_by_deadline, before.gpu_lost_by_deadline),
            not_secured: sub(ao.not_secured, bo.not_secured),
            failed: sub(failed(ao), failed(bo)),
            completed: sub(ao.rendered + ao.readback, bo.rendered + bo.readback),
            exact_press_after_shown_count: shown_count,
            exact_press_after_shown_mean_ms: (shown_count > 0).then(|| shown_total_ms / shown_count as f64),
            step_cpu: after.step_cpu,
            flash: FlashDelta::between(&before.flash, &after.flash),
        }
    }
}

#[cfg(test)]
#[path = "capture_render_still_tests.rs"]
mod tests;
