//! Отчёт о расписании статистики кадра для сценария `viewfinder` и его вердикт «публикации идут».
//!
//! Статистика кадра на GPU нужна только своим потребителям (сегодня — наш AE на потоке рендера); без них стадия не
//! считается, и публикаций нет. Поэтому ожидание «публикаций ≈ обработанные кадры / период» существует только
//! там, где потребитель есть: тип отчёта ([`FrameStatisticsReport`]) либо есть (потребитель включён), либо `None`
//! (вендорский AE: проверка не применяется и не краснеет).

use render_loop::viewfinder::{StatisticsDiagnostics, ViewfinderDiagnostics};
use serde::Serialize;

use super::viewfinder_metrics::WindowDelta;

/// Расписание статистики кадра при включённом потребителе (наш AE на потоке рендера).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FrameStatisticsReport {
    /// Установившийся период: статистику считает каждый N-й обработанный кадр (конфиг
    /// `viewfinder.auto_exposure.statistics_every_n_frames`).
    pub every_n_frames: u32,
}

impl FrameStatisticsReport {
    /// Отчёт по диагностике рендера; `None` — потребителей статистики на потоке рендера нет.
    pub fn of(viewfinder: &ViewfinderDiagnostics) -> Option<Self> {
        Self::of_schedule(viewfinder.statistics)
    }

    /// Отчёт по диагностике расписания (`None` — расписания нет: потребителей нет).
    pub fn of_schedule(schedule: Option<StatisticsDiagnostics>) -> Option<Self> {
        schedule.map(|schedule| Self {
            every_n_frames: schedule.every_n_frames,
        })
    }

    /// Образец для тестов вердикта: штатный период 4.
    #[cfg(test)]
    pub fn four_for_tests() -> Self {
        Self { every_n_frames: 4 }
    }
}

/// Нарушения «статистика кадра публикуется» (зеркало — в `scripts/smoke_checks_viewfinder.py`): за окно замера
/// опубликовано не меньше доли `min_fraction` от ожидаемого (обработанные кадры / период). Без потребителя
/// (`None`) публикаций не ждём. Верхней границы нет: на холодном старте AE статистика идёт на каждом кадре.
pub fn publication_violations(
    report: Option<&FrameStatisticsReport>,
    window: &WindowDelta,
    min_fraction: f64,
) -> Vec<String> {
    let Some(report) = report else {
        return Vec::new();
    };
    let period = f64::from(report.every_n_frames.max(1));
    let expected = window.isp.frames_processed as f64 / period;
    let published = window.isp.statistics_published as f64;
    if published >= expected * min_fraction {
        return Vec::new();
    }
    vec![format!(
        "статистика кадра опубликована {} раз, ожидалось не меньше {:.0} (обработано {} кадров, каждый {}-й, доля {})",
        window.isp.statistics_published,
        (expected * min_fraction).ceil(),
        window.isp.frames_processed,
        report.every_n_frames,
        min_fraction
    )]
}

#[cfg(test)]
#[path = "frame_statistics_report_tests.rs"]
mod tests;
