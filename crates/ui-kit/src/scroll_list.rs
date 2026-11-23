//! Список со свободной прокруткой вдоль одной оси (вертикальной или горизонтальной): ведение за пальцем, бросок,
//! «резинка» у краёв.
//!
//! Элемент владеет только физикой смещения. Содержимое (строки) строит экран: он берёт [`ScrollList::offset`]
//! на кадр и сдвигает узел-содержимое. Прилипания к строкам нет — список листается свободно. Так же, как
//! остальные элементы, список не владеет ареной жестов: экран передаёт данные пана намерениями.

use ui_core::time::MonotonicNanos;

use crate::drive::{AxisDrive, DragOrigin, DriveError, DriveSetup, GrowthDirection, MotionPhase, OffsetRange};
use crate::environment::ElementEnvironment;
use crate::pan::PanUpdate;
use crate::tokens::SnapRole;

/// Ошибка списка.
pub type ScrollListError = DriveError;

/// Вдоль какой оси идёт список.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    /// Вертикальный: палец вверх увозит содержимое вверх, смещение растёт (колонка ручек).
    Vertical,
    /// Горизонтальный: палец влево увозит содержимое влево, смещение растёт (ряд имён пресетов).
    Horizontal,
}

impl ScrollAxis {
    /// Направление роста смещения для оси.
    const fn growth(self) -> GrowthDirection {
        match self {
            ScrollAxis::Vertical => GrowthDirection::FingerUp,
            ScrollAxis::Horizontal => GrowthDirection::FingerLeft,
        }
    }
}

/// Свободно прокручиваемый список.
#[derive(Debug, Clone)]
pub struct ScrollList {
    drive: AxisDrive,
    content_extent_points: f64,
    viewport_extent_points: f64,
}

impl ScrollList {
    /// Список высотой содержимого `content_extent_points` в области `environment.viewport_extent_points`.
    /// Начальное смещение — 0 (верх списка).
    pub fn new(
        content_extent_points: f64,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, ScrollListError> {
        Self::along(ScrollAxis::Vertical, content_extent_points, 0.0, environment, now)
    }

    /// Список вдоль оси `axis` с начальным смещением `initial_offset_points` (зажимается в допустимый диапазон):
    /// так горизонтальный ряд сразу показывает нужное место, а не начало. `viewport_extent_points` окружения —
    /// протяжённость окна вдоль этой же оси.
    pub fn along(
        axis: ScrollAxis,
        content_extent_points: f64,
        initial_offset_points: f64,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, ScrollListError> {
        let viewport_extent_points = environment.viewport_extent_points;
        let range = Self::range_for(content_extent_points, viewport_extent_points);
        let initial_offset = initial_offset_points.clamp(range.min, range.max);
        let mut drive = AxisDrive::from_environment(
            environment,
            DriveSetup {
                // Движок прокрутки роли «шкала»: без точек прилипания он катит свободно и тормозит, как любой список.
                role: SnapRole::Scale,
                // Палец против роста оси — содержимое уезжает за пальцем, смещение растёт.
                direction: axis.growth(),
                drag_origin: DragOrigin::FromRecognition,
            },
            range,
            initial_offset,
        )?;
        drive.set_snap_points(&[], now)?;
        Ok(Self {
            drive,
            content_extent_points,
            viewport_extent_points,
        })
    }

    fn range_for(content_extent_points: f64, viewport_extent_points: f64) -> OffsetRange {
        OffsetRange {
            min: 0.0,
            max: (content_extent_points - viewport_extent_points).max(0.0),
        }
    }

    /// Смещение содержимого на `now`, pt (0 — верх; за краем включает «резинку»).
    pub fn offset(&self, now: MonotonicNanos) -> f64 {
        self.drive.sample(now).offset
    }

    /// Самое большое допустимое смещение (0, если всё помещается).
    pub fn max_offset(&self) -> f64 {
        Self::range_for(self.content_extent_points, self.viewport_extent_points).max
    }

    /// Фаза движения (покой, палец, бросок).
    pub fn phase(&self, now: MonotonicNanos) -> MotionPhase {
        self.drive.phase(now)
    }

    /// Палец ловит едущий список: он останавливается там, где был, без скачка.
    pub fn catch(&mut self, now: MonotonicNanos) {
        self.drive.catch(now);
    }

    /// Пан пальца (оси уже экранные оси списка: вниз по экрану списка — `dy` > 0).
    pub fn apply_pan(&mut self, time: MonotonicNanos, update: PanUpdate) -> Result<(), ScrollListError> {
        match update {
            PanUpdate::Began { translation } => self.drive.drag_began(time, translation)?,
            PanUpdate::Changed { translation } => self.drive.drag_moved(translation)?,
            PanUpdate::Ended { velocity } => {
                self.drive.drag_ended(velocity, time)?;
            }
            PanUpdate::Cancelled => {
                self.drive.drag_cancelled(time)?;
            }
        }
        Ok(())
    }

    /// Новая высота содержимого (список перестроен). Смещение при необходимости возвращается в пределы.
    pub fn set_content_extent(
        &mut self,
        content_extent_points: f64,
        now: MonotonicNanos,
    ) -> Result<(), ScrollListError> {
        if (content_extent_points - self.content_extent_points).abs() < f64::EPSILON {
            return Ok(());
        }
        self.drive.reconfigure(
            Self::range_for(content_extent_points, self.viewport_extent_points),
            &[],
            now,
        )?;
        self.content_extent_points = content_extent_points;
        Ok(())
    }
}
