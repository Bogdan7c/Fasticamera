//! Словарь зума (сессия 13, механизм М6 «политики зума»): единая шкала коэффициентов для всех
//! камер одного направления и единая формула «коэффициент → кадрирование этой камеры».
//!
//! # Кто что решает
//! * **Экран камеры** владеет значением зума, которое видит пользователь (кнопки, колесо, щипок), и
//!   публикует намерение [`LensIntent`] (направление + коэффициент) в ящик [`LensIntentChannel`].
//! * **Движок камеры** (`camera-core::zoom`) строит [`ZoomCatalog`] из характеристик камер и по
//!   намерению выбирает камеру с гистерезисом. Каталог он публикует в статусе.
//! * **Кадрирование** считают и экран (перевод тап ↔ сенсор), и рендер (ISP кадра), но одной
//!   функцией [`ZoomFraming::crop_for_camera`]. Рендер вызывает её для камеры **того кадра**,
//!   который обрабатывает, поэтому кадр старой камеры никогда не получает кадрирование новой
//!   (решение 93: у геометрии один владелец — экран, — и одна формула).
//!
//! # Шкала
//! [`ZoomRatio`] — во сколько раз поле зрения уже, чем у опорной камеры направления (у задних —
//! основной камеры, «1×»). Ширик OnePlus 11 — 0,59 (решение 111), 2× — кроп основной.

use std::sync::Arc;

use serde::Serialize;

use crate::auto_exposure::{LateBoundRefreshSignal, SignalDelivery};
use crate::geometry::PixelSize;
use crate::ids::{CameraId, LensFacing, LensRole};
use crate::lens_calibration::OpticalAxis;
use crate::three_a::{DirectiveGeneration, DirectiveSlot};
use crate::viewfinder_crop::{CropAspect, CropCenter, ViewfinderCrop, ZoomFactor};

/// Коэффициент зума относительно опорной камеры направления. Всегда конечен и больше нуля.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ZoomRatio(f32);

/// Коэффициент вне допустимого диапазона.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("коэффициент зума {value} должен быть конечным и больше нуля")]
pub struct InvalidZoomRatio {
    pub value: f32,
}

impl ZoomRatio {
    /// Опорная камера без цифрового зума («1×»).
    pub const ONE: Self = Self(1.0);

    pub fn new(value: f32) -> Result<Self, InvalidZoomRatio> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(InvalidZoomRatio { value })
        }
    }

    pub fn value(self) -> f32 {
        self.0
    }
}

/// Зеркалить ли превью этой камеры (фронтальная — да, как у iOS; решение 115).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewMirror {
    #[default]
    None,
    /// Зеркало по горизонтали экрана.
    Horizontal,
}

/// Одна физическая камера на шкале зума.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LensZoomPoint {
    pub role: LensRole,
    pub camera: CameraId,
    /// Коэффициент камеры без цифрового зума (ширик 0,59, основная 1).
    pub native_ratio: ZoomRatio,
    /// Эквивалентное фокусное расстояние (35 мм, по диагонали сенсора) без цифрового зума, мм.
    pub equivalent_focal_mm: f32,
    /// Оптическая ось (центр кадрирования при зуме) или причина, почему её нет.
    pub optical_axis: OpticalAxis,
    /// Активная область сенсора: в ней пиксели оптической оси.
    pub active_array: PixelSize,
    pub preview_mirror: PreviewMirror,
    /// Наибольший цифровой зум этой камеры (относительно её собственного поля зрения).
    pub max_digital_zoom: ZoomFactor,
}

impl LensZoomPoint {
    /// Наибольший коэффициент шкалы, который эта камера показывает кадрированием.
    pub fn max_ratio(&self) -> ZoomRatio {
        ZoomRatio(self.native_ratio.0 * self.max_digital_zoom.value())
    }
}

/// Чем является «упор» шкалы (кнопка зума, засечка колеса).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "role")]
pub enum ZoomStopKind {
    /// Собственное поле зрения камеры.
    Lens(LensRole),
    /// Цифровой кроп (2× основной — решение 111). Честно отличим от объектива в данных.
    DigitalCrop,
}

/// Упор шкалы зума: кнопка в ленте и сильная засечка колеса.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ZoomStop {
    pub ratio: ZoomRatio,
    pub kind: ZoomStopKind,
}

/// Допустимый отрезок коэффициентов направления.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ZoomRange {
    pub min: ZoomRatio,
    pub max: ZoomRatio,
}

impl ZoomRange {
    /// Ближайший допустимый коэффициент.
    pub fn clamp(&self, ratio: ZoomRatio) -> ZoomRatio {
        ZoomRatio(ratio.0.clamp(self.min.0, self.max.0))
    }
}

/// Камеры одного направления на общей шкале.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LensGroup {
    pub facing: LensFacing,
    /// Камеры по возрастанию `native_ratio`.
    pub lenses: Vec<LensZoomPoint>,
    /// Упоры по возрастанию коэффициента.
    pub stops: Vec<ZoomStop>,
    pub range: ZoomRange,
}

impl LensGroup {
    pub fn lens_for_camera(&self, camera: &CameraId) -> Option<&LensZoomPoint> {
        self.lenses.iter().find(|lens| &lens.camera == camera)
    }

    pub fn lens_for_role(&self, role: LensRole) -> Option<&LensZoomPoint> {
        self.lenses.iter().find(|lens| lens.role == role)
    }
}

/// Шкалы зума всех направлений. Строит `camera-core::zoom` из каталога камер.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ZoomCatalog {
    pub back: Option<LensGroup>,
    pub front: Option<LensGroup>,
}

impl ZoomCatalog {
    pub fn group(&self, facing: LensFacing) -> Option<&LensGroup> {
        match facing {
            LensFacing::Back => self.back.as_ref(),
            LensFacing::Front => self.front.as_ref(),
            LensFacing::External => None,
        }
    }

    /// Группа и точка камеры по её ID (любого направления).
    pub fn lens_for_camera(&self, camera: &CameraId) -> Option<(&LensGroup, &LensZoomPoint)> {
        [self.back.as_ref(), self.front.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|group| group.lens_for_camera(camera).map(|lens| (group, lens)))
    }
}

/// Каким жестом пришло намерение. От этого зависит, как движок выбирает камеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoomGesture {
    /// Явный одиночный выбор: тап по кнопке зума, переворот, программная установка.
    /// Камера выбирается без гистерезиса (тап по «1» — основная, а не кроп ширика), и
    /// заблокированная после отказа камера пробуется снова: это прямая просьба пользователя.
    Discrete,
    /// Плавный зум колесом или щипком. Работает гистерезис (дрожь пальца у порога не дёргает
    /// камеры), заблокированная камера не пробуется, пока жест не уведёт к другой. В 13b — ещё и
    /// сигнал «держать соседнюю камеру тёплой» (решение 117).
    Continuous,
}

/// Намерение экрана: какое направление и какой коэффициент показать. Движок сам решает, какой
/// камерой — экран камеры не выбирает.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LensIntent {
    pub facing: LensFacing,
    pub ratio: ZoomRatio,
    pub gesture: ZoomGesture,
}

/// Ящик намерений «экран → движок»: последнее значение побеждает, промежуточные (каждый кадр
/// колеса) не копятся в очереди команд движка. Толчок с поздней привязкой: экран может появиться
/// раньше движка.
#[derive(Debug, Clone, Default)]
pub struct LensIntentChannel {
    slot: DirectiveSlot<LensIntent>,
    signal: LateBoundRefreshSignal,
}

impl LensIntentChannel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Положить намерение и толкнуть движок.
    pub fn publish(&self, intent: LensIntent) -> (DirectiveGeneration, SignalDelivery) {
        let generation = self.slot.publish(intent);
        (generation, self.signal.signal())
    }

    /// Самое свежее намерение с поколением.
    pub fn latest(&self) -> Option<(DirectiveGeneration, LensIntent)> {
        self.slot.latest()
    }

    /// Намерение новее `seen`.
    pub fn newer_than(&self, seen: Option<DirectiveGeneration>) -> Option<(DirectiveGeneration, LensIntent)> {
        self.slot.newer_than(seen)
    }

    /// Толчок для привязки движка композицией.
    pub fn refresh_signal(&self) -> &LateBoundRefreshSignal {
        &self.signal
    }
}

/// Насколько кадрирование камеры совпало с запрошенным коэффициентом.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ZoomFit {
    /// Камера показывает ровно запрошенный коэффициент.
    Exact,
    /// Запрошено шире, чем камера может (идёт переключение на более широкую): показан её минимум.
    WiderThanLens,
    /// Запрошено больше предела цифрового зума камеры: показан предел.
    BeyondDigitalLimit,
    /// Камеры нет в каталоге зума (логическая камера, сценарий): кадр целиком по центру.
    CameraNotInCatalog,
}

/// Итог формулы кадрирования для одной камеры.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FramedCrop {
    pub crop: ViewfinderCrop,
    pub mirror: PreviewMirror,
    /// Коэффициент шкалы, который реально покажет кадр (для масштабирования удержанного кадра).
    pub shown_ratio: ZoomRatio,
    pub fit: ZoomFit,
}

/// Кадрирование по зуму: соотношение сторон + коэффициент + шкалы камер. Решает экран камеры.
/// (Без `Serialize`: каталог — общий `Arc`, в отчёты идёт [`FramedCrop`].)
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomFraming {
    pub aspect: CropAspect,
    /// Направление, на шкале которого задан `ratio` (у задних и фронтальной шкалы разные).
    pub facing: LensFacing,
    pub ratio: ZoomRatio,
    pub catalog: Arc<ZoomCatalog>,
}

impl ZoomFraming {
    /// Кадрирование для кадра камеры `camera` — единственная формула и для экрана, и для рендера.
    ///
    /// Цифровой зум = `ratio / native_ratio`, зажатый в `[1, max_digital_zoom]`; центр — оптическая
    /// ось камеры (без калибровки — центр полезной области); зеркало — по камере.
    pub fn crop_for_camera(&self, camera: &CameraId) -> FramedCrop {
        let Some((_, lens)) = self.catalog.lens_for_camera(camera) else {
            return FramedCrop {
                crop: ViewfinderCrop::centered(self.aspect, ZoomFactor::NONE),
                mirror: PreviewMirror::None,
                shown_ratio: self.ratio,
                fit: ZoomFit::CameraNotInCatalog,
            };
        };
        let wanted = self.ratio.0 / lens.native_ratio.0;
        let limit = lens.max_digital_zoom.value();
        let (relative, fit) = if wanted < 1.0 {
            (1.0, ZoomFit::WiderThanLens)
        } else if wanted > limit {
            (limit, ZoomFit::BeyondDigitalLimit)
        } else {
            (wanted, ZoomFit::Exact)
        };
        // `relative` ≥ 1 и конечен по построению (native и ratio проверены конструкторами).
        let zoom = ZoomFactor::new(relative).unwrap_or(ZoomFactor::NONE);
        FramedCrop {
            crop: ViewfinderCrop {
                aspect: self.aspect,
                zoom,
                center: crop_center(lens),
            },
            mirror: lens.preview_mirror,
            shown_ratio: ZoomRatio(lens.native_ratio.0 * zoom.value()),
            fit,
        }
    }
}

/// Центр кадрирования: оптическая ось в пикселях буфера (пиксель буфера = пиксель активной области).
fn crop_center(lens: &LensZoomPoint) -> CropCenter {
    match lens.optical_axis {
        OpticalAxis::Calibrated { point } => CropCenter::BufferPixel {
            x: point.x() * lens.active_array.width as f32,
            y: point.y() * lens.active_array.height as f32,
        },
        OpticalAxis::Uncalibrated { .. } => CropCenter::ValidAreaCenter,
    }
}

#[cfg(test)]
#[path = "zoom_tests.rs"]
mod tests;
