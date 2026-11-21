//! Чистые правила сценария `standby` (13b): какие коэффициенты публиковать и как считать частоту.
//!
//! Никаких «основная/ширик» в коде: «дом» — объектив, чей собственный коэффициент ближе всего к 1× (опорная камера
//! шкалы), «сосед» — та камера, которую движок реально прогрел (берётся из его статуса), а коэффициенты считаются по
//! каталогу зума.

use std::time::Duration;

use camera_contract::ids::LensFacing;
use camera_contract::zoom::{LensGroup, LensZoomPoint, ZoomCatalog, ZoomRatio};
use raw_frame_stream::RawStreamCounters;

/// Объектив «дома»: собственный коэффициент ближе всего к 1× на логарифмической шкале (0,5× и 2× одинаково далеки).
pub fn home_lens(group: &LensGroup) -> Option<&LensZoomPoint> {
    group
        .lenses
        .iter()
        .min_by(|left, right| distance_from_one(left.native_ratio).total_cmp(&distance_from_one(right.native_ratio)))
}

fn distance_from_one(ratio: ZoomRatio) -> f32 {
    ratio.value().ln().abs()
}

/// Задняя группа каталога и её «дом».
pub fn back_home(catalog: &ZoomCatalog) -> Option<(&LensGroup, &LensZoomPoint)> {
    let group = catalog.group(LensFacing::Back)?;
    home_lens(group).map(|home| (group, home))
}

/// Коэффициент плавного жеста, который переведёт движок с объектива `from` на `to`.
///
/// Вниз по шкале (`to` шире) хватает собственного коэффициента `to`: текущий объектив его показать не может, и переход
/// немедленный. Вверх селектор ждёт `native × (1 + гистерезис)`, поэтому берётся `native × overshoot`. Результат
/// ограничен диапазоном группы.
pub fn ratio_for_continuous_switch(
    group: &LensGroup,
    from: &LensZoomPoint,
    to: &LensZoomPoint,
    overshoot: f32,
) -> ZoomRatio {
    let native = to.native_ratio.value();
    let wanted = if native > from.native_ratio.value() {
        native * overshoot
    } else {
        native
    };
    // Значение конечно и положительно (оба множителя такие), `new` не откажет; на всякий случай — собственный коэффициент.
    let ratio = ZoomRatio::new(wanted).unwrap_or(to.native_ratio);
    group.range.clamp(ratio)
}

/// Частоты за окно: показываемая эпоха и тёплые эпохи (к/с).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowFps {
    /// Кадры показываемой эпохи, прошедшие в раздачу (`frames_published`).
    pub primary: f64,
    /// Кадры тёплых эпох, сразу возвращённые камере (`standby_frames_returned`).
    pub standby: f64,
}

/// Частоты между двумя снимками счётчиков потока кадров. Нулевое окно даёт `None`: делить не на что.
pub fn window_fps(before: &RawStreamCounters, after: &RawStreamCounters, elapsed: Duration) -> Option<WindowFps> {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return None;
    }
    let per_second = |before: u64, after: u64| after.saturating_sub(before) as f64 / seconds;
    Some(WindowFps {
        primary: per_second(before.frames_published, after.frames_published),
        standby: per_second(before.standby_frames_returned, after.standby_frames_returned),
    })
}

#[cfg(test)]
#[path = "standby_plan_tests.rs"]
mod tests;
