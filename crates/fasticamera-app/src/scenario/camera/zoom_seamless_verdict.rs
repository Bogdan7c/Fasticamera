//! Вердикт бесшовных шагов `zoom` (13b): чистая функция «запись шага / итоги → нарушения».
//!
//! Что обязан показать бесшовный шаг со сменой объектива (`out`, `back`):
//! * смена исполнена повышением тёплой (`promoted_standby`), событие `LensSwitched` — ровно одно;
//! * ни одного удержанного кадра, ни одного чёрного кадра любого вида, ни одного кадра нового потока, скрытого
//!   непрерывностью (`frames_hidden_by_continuity`);
//! * разрыв между последним кадром старой камеры и первым живым новой — не больше `max_switch_gap_periods` периодов;
//! * обе задние камеры, пока стримят обе, держат не меньше `min_back_fps`;
//! * тёплая стала готова (время «нажатие → готова» измерено).
//!
//! Шаг `release` обязан закончиться: колесо ушло и ни одна ячейка не держит камеру (время измерено).
//! Итоги прогона: ни одного утёкшего лизинга, ни одного отказа тёплой, ни одного отвергнутого обмена.

use super::zoom_record::ZoomStepRecord;
use super::zoom_report::ZoomObservationReport;
use crate::zoom_scenario_config::SeamlessPlan;
use camera_core::lens_status::LensSwitchMethod;

/// Нарушения одного бесшовного шага (пусто у обычных шагов).
pub fn seamless_step_violations(plan: &SeamlessPlan, step: &ZoomStepRecord) -> Vec<String> {
    let Some(seamless) = &step.seamless else {
        return Vec::new();
    };
    let name = &step.name;
    let mut violations = Vec::new();
    if seamless.leg == "release" {
        if seamless.closed_ms.is_none() {
            violations.push(format!("{name}: время «пальцы подняты → тёплая закрыта» не измерено"));
        }
        return violations;
    }
    match step.switch.map(|switch| switch.method) {
        Some(LensSwitchMethod::PromotedStandby) => {}
        Some(LensSwitchMethod::Reopened) => violations.push(format!(
            "{name}: смена исполнена переоткрытием камеры (Reopened), а не повышением тёплой — шов виден"
        )),
        None => violations.push(format!("{name}: события смены объектива нет")),
    }
    if seamless.switch_events != 1 {
        violations.push(format!(
            "{name}: событий LensSwitched {} (бесшовная смена — ровно одно)",
            seamless.switch_events
        ));
    }
    let hidden = step.continuity.frames_hidden_by_continuity;
    let black = step.black_frames_while_picture_expected + step.continuity.black_frames_before_first_picture;
    for (count, what) in [
        (step.held_frames, "удержанных кадров"),
        (black, "чёрных кадров"),
        (hidden, "кадров нового потока, скрытых непрерывностью"),
    ] {
        if count > 0 {
            violations.push(format!("{name}: {what} {count} (бесшовная смена — 0)"));
        }
    }
    match seamless.gap {
        Some(gap) if gap.periods() > plan.max_switch_gap_periods => violations.push(format!(
            "{name}: разрыв показа при смене {:.1} мс = {:.1} периода кадра ({:.1} мс), порог {} периода",
            gap.gap_ms,
            gap.periods(),
            gap.period_ms,
            plan.max_switch_gap_periods
        )),
        Some(_) => {}
        None if step.ring_overran => {}
        None => violations.push(format!(
            "{name}: разрыв показа при смене не измерен (нет кадров старой или новой камеры)"
        )),
    }
    match seamless.fps {
        Some(fps) if fps.minimum() < plan.min_back_fps => violations.push(format!(
            "{name}: пока стримят обе камеры, частота {:.1} (показываемая) и {:.1} (тёплая) к/с, порог {}",
            fps.primary, fps.standby, plan.min_back_fps
        )),
        Some(_) => {}
        None => violations.push(format!("{name}: частота обеих камер не измерена")),
    }
    if seamless.ready_ms.is_none() {
        violations.push(format!("{name}: время «нажатие → тёплая готова» не измерено"));
    }
    violations
}

/// Нарушения по итогам прогона: утечки, отказы тёплой, отвергнутые обмены.
pub fn seamless_totals_violations(report: &ZoomObservationReport) -> Vec<String> {
    let mut violations = Vec::new();
    let totals = &report.totals;
    if totals.leaked_leases > 0 {
        violations.push(format!(
            "утекло лизингов кадров: {} (должно быть 0)",
            totals.leaked_leases
        ));
    }
    if totals.standby_failures > 0 {
        violations.push(format!(
            "тёплая камера отказывала {} раз (должно быть 0)",
            totals.standby_failures
        ));
    }
    if totals.promotion_fallbacks > 0 {
        violations.push(format!(
            "бэкенд отверг обмен ролей {} раз: смена пошла переоткрытием",
            totals.promotion_fallbacks
        ));
    }
    let seamless_switches = report
        .steps
        .iter()
        .filter(|step| step.seamless.as_ref().is_some_and(|seamless| seamless.leg != "release"))
        .count() as u64;
    if totals.standby_promotions < seamless_switches {
        violations.push(format!(
            "движок повысил тёплую {} раз при {seamless_switches} бесшовных сменах",
            totals.standby_promotions
        ));
    }
    violations
}

#[cfg(test)]
#[path = "zoom_seamless_verdict_tests.rs"]
mod tests;
