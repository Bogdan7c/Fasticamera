//! Цели касаний раздела калибровки для сквозных стендов: где (в pt поверхности) стоит палец, чтобы взять линейку
//! строки, её «↻» или точку полосы, и где окно списка, которое листается пальцем.
//!
//! Центры берутся у узлов той самой сцены, что принимает касания, поэтому формул здесь нет и список не может
//! разойтись с тем, что реально ловит палец. Порядок линеек — порядок строк раздела: в виде шкал восемь ручек
//! ([`look_contract::GlobalKnob::ALL`]), затем три оси выбранной полосы; в виде колёс — одна линейка кольца яркости.

use ui_core::geometry::{Point, Rect};

use super::CameraScreen;

/// Цели касаний раздела калибровки, pt поверхности (портретные оси, как у касаний `ScreenTouch`).
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationTouchTargets {
    /// Зона колонки ручек целиком: строка, чья линейка вне неё по вертикали, пальцем не берётся — список листают.
    pub knobs_zone: Rect,
    /// Центр окна списка: отсюда начинают вертикальную протяжку, листающую список.
    pub list_window_center: Point,
    /// Центры линеек строк в порядке строк (в том числе вне окна списка: такие строки сначала листают в окно).
    pub rulers: Vec<Point>,
    /// Центры «↻» строк в том же порядке.
    pub resets: Vec<Point>,
    /// Центры точек полос в порядке полос модели вида.
    pub band_dots: Vec<Point>,
    /// Центр капсулы белого в полоске прибора (решение 290).
    pub white_capsule: Option<Point>,
    /// Центры чисел осей карточки белого: Кельвины, потом оттенок. Карточка закрыта — узлы стоят на месте, но
    /// касаний не ловят: сначала откройте её тапом по капсуле.
    pub white_axis_numbers: Vec<Point>,
    /// Центры кнопок ряда карточки белого слева направо: «Phone», потом кнопки света в порядке конфига («Auto» в
    /// разделе нет, решение 294).
    pub white_row: Vec<Point>,
    /// Центр зоны шкалы карточки белого.
    pub white_ruler: Option<Point>,
}

impl CameraScreen {
    /// Цели касаний раздела калибровки по текущей сцене; `None` — раздела на экране нет (не режим калибровки или
    /// нет размера поверхности).
    pub fn calibration_touch_targets(&self) -> Option<CalibrationTouchTargets> {
        let stage = self.stage.as_ref()?;
        let layout = self.state.calibration().layout()?;
        let list_window_center = *stage.node_centers("calib_knobs_window").first()?;
        Some(CalibrationTouchTargets {
            knobs_zone: layout.zones().knobs,
            list_window_center,
            rulers: stage.node_centers("calib_ruler"),
            resets: stage.node_centers("calib_reset"),
            band_dots: stage.node_centers("calib_band_dot"),
            white_capsule: stage.node_centers("calib_white_capsule").first().copied(),
            white_axis_numbers: stage.node_centers("white_axis"),
            white_row: stage.node_centers("white_light"),
            white_ruler: stage.node_centers("white_ruler").first().copied(),
        })
    }
}
