//! Механика площадки: относительное ведение, прижатие к краю, «действие рождает только палец».

use thiserror::Error;
use ui_core::geometry::Vector;

use super::point::{DiscPoint, DiscPointError};
use crate::pan::PanUpdate;
use crate::tokens::DiscPadTokens;

/// Почему площадку нельзя создать.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum DiscPadError {
    /// Радиус круга должен быть конечным и > 0.
    #[error("радиус круга должен быть конечным и > 0, получено {0} pt")]
    InvalidRadius(f64),
    /// Коэффициент ведения должен быть конечным и > 0.
    #[error("коэффициент ведения должен быть конечным и > 0, получено {0}")]
    InvalidDriveRatio(f64),
}

/// Спецификация площадки: размер круга и «передача» от пальца к точке.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiscPadSpec {
    /// Радиус круга на экране, pt: точка на краю круга = 1.
    pub radius_pt: f64,
    /// Сколько pt точки на 1 pt пальца. 0,5 = палец проходит 2 pt, точка — 1 pt (замедление).
    pub drive_ratio: f64,
}

impl DiscPadSpec {
    /// Круг заданного радиуса с коэффициентом ведения из токенов (`DiscPadTokens::drive_ratio`).
    pub fn with_token_drive(radius_pt: f64, tokens: &DiscPadTokens) -> Self {
        Self {
            radius_pt,
            drive_ratio: tokens.drive_ratio,
        }
    }

    fn validate(&self) -> Result<(), DiscPadError> {
        if !(self.radius_pt.is_finite() && self.radius_pt > 0.0) {
            return Err(DiscPadError::InvalidRadius(self.radius_pt));
        }
        if !(self.drive_ratio.is_finite() && self.drive_ratio > 0.0) {
            return Err(DiscPadError::InvalidDriveRatio(self.drive_ratio));
        }
        Ok(())
    }
}

/// Что случилось с программной установкой значения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgrammaticSet {
    /// Значение принято (событий оно не порождает).
    Applied,
    /// Палец ведёт точку: установка отброшена и посчитана. Факты придут заново после отпускания.
    DroppedWhileFingerDrives,
}

/// Жест ведения: от какой точки начали.
#[derive(Debug, Clone, Copy)]
struct Drag {
    /// Точка в момент `Began`: ведение относительное, точка НЕ прыгает под палец.
    start: DiscPoint,
}

/// Точка в круге с относительным ведением пальцем.
///
/// # Правила
/// * `Began` запоминает точку начала; `Changed` ставит `начало + смещение · коэффициент / радиус`, прижатое к
///   краю круга. Смещение берётся `from_recognition` (от распознавания пана), чтобы порог распознавания
///   не давал скачка точки на старте.
/// * Действие рождает только палец: [`DiscPad::apply_pan`] возвращает значение лишь когда оно изменилось
///   (палец упёрся в край и тянет дальше — повтора нет).
/// * Программная установка ([`DiscPad::set_point`]) событий не рождает, а пока палец ведёт — отбрасывается
///   (не перебивает палец; экран получит факты заново после отпускания).
/// * `Ended` — конец ведения, точка остаётся где есть. `Cancelled` — тоже конец ведения без отката: каждое
///   изменение уже ушло экрану как действие, откат был бы новым действием «из ниоткуда».
///
/// # Почему не `FingerGate`
/// `FingerGate` гасит эхо событий шкалы и ждёт `Settled`, когда шкала доехала по инерции. У площадки нет ни
/// инерции, ни `Settled`, ни потока событий значения: значение меняется синхронно внутри `apply_pan`
/// (палец) или `set_point` (программа), поэтому «кто двигает» — это само наличие ведения ([`Drag`]).
#[derive(Debug)]
pub struct DiscPad {
    spec: DiscPadSpec,
    point: DiscPoint,
    drag: Option<Drag>,
    /// Сколько программных установок отброшено, потому что палец вёл точку (диагностика).
    dropped_programmatic_sets: u64,
}

impl DiscPad {
    /// Площадка с начальным значением; спецификация проверяется один раз здесь.
    pub fn new(spec: DiscPadSpec, initial: DiscPoint) -> Result<Self, DiscPadError> {
        spec.validate()?;
        Ok(Self {
            spec,
            point: initial,
            drag: None,
            dropped_programmatic_sets: 0,
        })
    }

    pub fn spec(&self) -> DiscPadSpec {
        self.spec
    }

    /// Текущая точка.
    pub fn point(&self) -> DiscPoint {
        self.point
    }

    /// Ведёт ли точку палец сейчас.
    pub fn is_finger_driven(&self) -> bool {
        self.drag.is_some()
    }

    /// Сколько программных установок отброшено, потому что палец вёл точку.
    pub fn dropped_programmatic_sets(&self) -> u64 {
        self.dropped_programmatic_sets
    }

    /// Пан пальца. `Ok(Some(точка))` — значение изменилось пальцем, экран превращает его в действие;
    /// `Ok(None)` — менять нечего (конец жеста, палец уперся в край, `Changed` без `Began`).
    /// Ошибка — смещение не конечно; состояние при этом не меняется.
    pub fn apply_pan(&mut self, update: PanUpdate) -> Result<Option<DiscPoint>, DiscPointError> {
        match update {
            PanUpdate::Began { translation } => {
                self.drag = Some(Drag { start: self.point });
                self.follow_finger(translation.from_recognition)
            }
            PanUpdate::Changed { translation } => self.follow_finger(translation.from_recognition),
            PanUpdate::Ended { .. } | PanUpdate::Cancelled => {
                self.drag = None;
                Ok(None)
            }
        }
    }

    /// Программная установка значения (синхронизация из фактов): событий не рождает; пока ведёт палец —
    /// отбрасывается и считается.
    pub fn set_point(&mut self, point: DiscPoint) -> ProgrammaticSet {
        if self.drag.is_some() {
            self.dropped_programmatic_sets += 1;
            return ProgrammaticSet::DroppedWhileFingerDrives;
        }
        self.point = point;
        ProgrammaticSet::Applied
    }

    /// Новая точка от смещения пальца; `Some` только если она отличается от текущей.
    fn follow_finger(&mut self, finger_translation: Vector) -> Result<Option<DiscPoint>, DiscPointError> {
        let Some(drag) = self.drag else {
            return Ok(None);
        };
        let (delta_x, delta_y) = self.eye_delta_from_screen(finger_translation);
        let next = DiscPoint::clamped(drag.start.x() + delta_x, drag.start.y() + delta_y)?;
        if next == self.point {
            return Ok(None);
        }
        self.point = next;
        Ok(Some(next))
    }

    /// Смещение пальца (экран: x вправо, y ВНИЗ, pt) → смещение точки в единицах круга (глаз: x вправо,
    /// y ВВЕРХ). Единственное место, где переворачивается ось y.
    fn eye_delta_from_screen(&self, finger: Vector) -> (f64, f64) {
        let units_per_pt = self.spec.drive_ratio / self.spec.radius_pt;
        (finger.dx * units_per_pt, -finger.dy * units_per_pt)
    }
}
