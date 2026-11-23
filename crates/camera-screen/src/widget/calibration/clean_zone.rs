//! Невидимая зона свайпа режимов над чистым превью раздела (решение 203).
//!
//! Чистая зона ничего не рисует и не ловит касаний: под ней живой видоискатель. Но распознаватели видоискателя
//! (`ModesPan`, `MenuOpenPan`) сидят в осях ПОВЕРХНОСТИ, а у раздела калибровки в альбоме глазная горизонталь — это
//! вертикаль поверхности. Поэтому в CALIBRATE свайп режимов по чистой зоне принадлежит разделу: узел здесь ждёт пан
//! по ГЛАЗНОЙ горизонтали (`SectionSpace::pan_direction`) и отдаёт его капсуле режимов, которая переводит смещение в
//! глазные оси (`SectionSpace::eye_pan`). Режим меняется так же, как свайпом по видоискателю в PHOTO: оба жеста
//! кормят одну и ту же карусель с теми же порогами.
//!
//! Инвариант: узел ничего не рисует (у него нет визуала), поэтому чистая зона остаётся чистой.

use ui_core::geometry::Rect;
use ui_core::gesture::{PanSpec, RecognizerSpec};
use ui_core::id::NodeId;

use super::space::{EyeAxis, SectionSpace};
use crate::widget::error::WidgetError;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;

/// Строит невидимый узел свайпа режимов поверх `frame` (рамка в системе корня раздела) и возвращает его id.
/// Жест приходит как `CalibrationModesPan` и обрабатывается капсулой раздела.
pub(super) fn build_mode_swipe_zone(
    kit: &mut SceneKit,
    space: &SectionSpace,
    (parent, parent_rect): (NodeId, Rect),
    frame: Rect,
) -> Result<NodeId, WidgetError> {
    let node = kit.add_node(parent, "calib_clean_zone", frame, parent_rect)?;
    kit.add_recognizer(
        node,
        RecognizerSpec::Pan(PanSpec {
            direction: space.pan_direction(EyeAxis::Horizontal),
            ..PanSpec::default()
        }),
        ScreenGesture::CalibrationModesPan,
    )?;
    Ok(node)
}
