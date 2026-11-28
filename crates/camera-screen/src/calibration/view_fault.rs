//! Почему вид колонки (колёса или полоски) не поместился: общая «причина» для состояния раздела.
//!
//! Раздел при этом живёт (шапка и переключатель на месте, зона вида пуста). Состояние запоминает причину, чтобы отказ
//! `InvalidSurface` считался один раз на смену причины, а не на каждый тик.

use super::bars_frames::{BarsFramesError, bars_ruler_placements};
use super::state::ControlViewKind;
use super::wheels_frames::{WheelsFramesError, wheel_pad_placement};
use super::{CalibrationLayout, CalibrationWidgetsConfig};

/// Причина, по которой выбранный вид колонки не разложился.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ControlViewFault {
    /// Круг колеса не поместился.
    Wheels(WheelsFramesError),
    /// Вкладки и три шкалы не поместились.
    Bars(BarsFramesError),
}

/// Причина неудачи раскладки вида `view` в колонке; `None` — вид помещается (или это вид шкал, у него прокрутка).
pub(crate) fn control_view_fault(
    view: ControlViewKind,
    layout: &CalibrationLayout,
    config: &CalibrationWidgetsConfig,
    pixels_per_point: f64,
) -> Option<ControlViewFault> {
    match view {
        ControlViewKind::Scales => None,
        ControlViewKind::Wheels => wheel_pad_placement(layout, config, pixels_per_point)
            .err()
            .map(ControlViewFault::Wheels),
        ControlViewKind::Bars => bars_ruler_placements(layout, config).err().map(ControlViewFault::Bars),
    }
}
