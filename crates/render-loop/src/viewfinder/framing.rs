//! Кадрирование и место видоискателя на экране одной командой (решение 93, сессия 12).
//!
//! Место видоискателя решает экран камеры (он знает свою раскладку), а рендер лишь получает команду
//! «рисуй вот сюда». Кадрирование (какую часть сенсора берёт ISP) и место (куда на экране ложится
//! результат) обязаны меняться ВМЕСТЕ: если бы это были две команды, между ними мог бы показаться кадр
//! «новое кадрирование в старом месте» (картинка другого формата, втиснутая в прежний прямоугольник).
//! Поэтому [`ViewfinderFraming`] применяется одной транзакцией: настройки меняются целиком или не
//! меняются вовсе.
//!
//! Прямоугольник задаётся долями ЛОГИЧЕСКОЙ цели ([`NormalizedScreenRect`]): он не зависит от размера
//! surface в пикселях и переживает её пересоздание — пиксели считаются заново на каждом кадре.

use camera_contract::ids::{CameraId, LensFacing};
use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop};
use camera_contract::zoom::{FramedCrop, PreviewMirror, ZoomFit, ZoomFraming, ZoomRatio};

use crate::config::ViewfinderPlacement;

/// Допуск на погрешность f32 при проверке, что правый/нижний край не выходит за 1.0 (`0.7 + 0.3` в f32
/// даёт `1.0000001`; такой прямоугольник честно «до края», а не «за краем»).
const EDGE_TOLERANCE: f32 = 1e-6;

/// Зум, который хочет экран: коэффициент вместе с направлением, на чьей шкале он задан (шкалы задних и
/// фронтальной камер несравнимы).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WantedZoom {
    /// Направление шкалы.
    pub facing: LensFacing,
    /// Коэффициент на этой шкале.
    pub ratio: ZoomRatio,
}

/// Прямоугольник в долях логической (портретной, как видит пользователь) цели, 0..1, от верхнего левого угла.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedScreenRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

/// Почему прямоугольник не построен.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ScreenRectError {
    /// Одно из чисел не конечное (NaN или бесконечность).
    #[error("доля прямоугольника не конечная: {name} = {value}")]
    NotFinite {
        /// Имя поля.
        name: &'static str,
        /// Значение.
        value: f32,
    },
    /// Левый или верхний край вне `0..=1`.
    #[error("начало прямоугольника вне 0..=1: {name} = {value}")]
    OriginOutOfRange {
        /// Имя поля.
        name: &'static str,
        /// Значение.
        value: f32,
    },
    /// Ширина или высота не положительна (пустой прямоугольник рисовать нечем).
    #[error("размер прямоугольника не положителен: {name} = {value}")]
    EmptySize {
        /// Имя поля.
        name: &'static str,
        /// Значение.
        value: f32,
    },
    /// Правый или нижний край выходит за 1.0.
    #[error("прямоугольник выходит за цель: {name} = {value} > 1")]
    ExtendsPastTarget {
        /// Имя края (`right` или `bottom`).
        name: &'static str,
        /// Значение края.
        value: f32,
    },
}

impl NormalizedScreenRect {
    /// Проверяемый конструктор: числа конечны, начало внутри `0..=1`, размер положителен, прямоугольник
    /// целиком внутри цели.
    pub fn new(left: f32, top: f32, width: f32, height: f32) -> Result<Self, ScreenRectError> {
        for (name, value) in [("left", left), ("top", top), ("width", width), ("height", height)] {
            if !value.is_finite() {
                return Err(ScreenRectError::NotFinite { name, value });
            }
        }
        for (name, value) in [("left", left), ("top", top)] {
            if !(0.0..=1.0).contains(&value) {
                return Err(ScreenRectError::OriginOutOfRange { name, value });
            }
        }
        for (name, value) in [("width", width), ("height", height)] {
            if value <= 0.0 {
                return Err(ScreenRectError::EmptySize { name, value });
            }
        }
        for (name, value) in [("right", left + width), ("bottom", top + height)] {
            if value > 1.0 + EDGE_TOLERANCE {
                return Err(ScreenRectError::ExtendsPastTarget { name, value });
            }
        }
        Ok(Self {
            left,
            top,
            width,
            height,
        })
    }

    /// Левый край, доля ширины цели.
    pub fn left(&self) -> f32 {
        self.left
    }

    /// Верхний край, доля высоты цели.
    pub fn top(&self) -> f32 {
        self.top
    }

    /// Ширина, доля ширины цели.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Высота, доля высоты цели.
    pub fn height(&self) -> f32 {
        self.height
    }
}

/// Куда на экране ложится видоискатель по команде.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FramingDestination {
    /// Как в конфиге (`viewfinder.placement`): прежнее поведение до решения 93. По умолчанию.
    #[default]
    Configured,
    /// В этот прямоугольник логической цели; изображение вписывается с сохранением пропорций.
    ScreenRect(NormalizedScreenRect),
}

/// Как выбирается кадрирование: фиксированное для любой камеры или по зуму (своё у каждой камеры).
///
/// `Zoom` решается ПО КАМЕРЕ КАДРА, а не по «текущей» камере: кадр старой камеры, который ещё идёт через
/// ISP во время переключения объектива, получает кадрирование старой камеры, а не новой (нет скачка на один
/// кадр). Формулу даёт `camera-contract` ([`ZoomFraming::crop_for_camera`]) — ту же, что считает экран.
#[derive(Debug, Clone, PartialEq)]
pub enum FramingCrop {
    /// Одно и то же кадрирование для кадров любой камеры (сценарии, умолчание из конфига).
    Fixed(ViewfinderCrop),
    /// Кадрирование по коэффициенту зума и шкалам камер.
    Zoom(ZoomFraming),
}

impl FramingCrop {
    /// Соотношение сторон (для проверки команды и для ёмкости выходного изображения ISP).
    pub fn aspect(&self) -> CropAspect {
        match self {
            Self::Fixed(crop) => crop.aspect,
            Self::Zoom(zoom) => zoom.aspect,
        }
    }

    /// Итог кадрирования для кадра камеры `camera`. У фиксированного кадрирования нет зеркала и
    /// сдвига шкалы: показанный коэффициент — опорный.
    pub fn framed_for(&self, camera: &CameraId) -> FramedCrop {
        match self {
            Self::Fixed(crop) => FramedCrop {
                crop: *crop,
                mirror: PreviewMirror::None,
                shown_ratio: ZoomRatio::ONE,
                fit: ZoomFit::Exact,
            },
            Self::Zoom(zoom) => zoom.crop_for_camera(camera),
        }
    }

    /// Направление камеры `camera` по каталогу зума (`None` — фиксированное кадрирование или камеры нет в каталоге).
    pub fn facing_of(&self, camera: &CameraId) -> Option<LensFacing> {
        match self {
            Self::Fixed(_) => None,
            Self::Zoom(zoom) => zoom.catalog.lens_for_camera(camera).map(|(group, _)| group.facing),
        }
    }

    /// Зум, который хочет экран: коэффициент и направление, на шкале которого он задан (`None` у
    /// фиксированного кадрирования — зума по шкале нет).
    pub fn wanted_zoom(&self) -> Option<WantedZoom> {
        match self {
            Self::Fixed(_) => None,
            Self::Zoom(zoom) => Some(WantedZoom {
                facing: zoom.facing,
                ratio: zoom.ratio,
            }),
        }
    }

    /// Почему кадрирование неисполнимо без состояния бэкенда (`None` — исполнимо).
    pub fn rejection_reason(&self) -> Option<String> {
        match self {
            Self::Fixed(_) => None,
            // Пустой каталог — ошибка сборки команды: ни одна камера не получит свой центр и зеркало, а
            // молчаливо показать «весь кадр» значило бы скрыть поломку.
            Self::Zoom(zoom) if zoom.catalog.back.is_none() && zoom.catalog.front.is_none() => {
                Some("каталог зума пуст: нет ни одной камеры".to_owned())
            }
            Self::Zoom(_) => None,
        }
    }
}

impl From<ViewfinderCrop> for FramingCrop {
    fn from(crop: ViewfinderCrop) -> Self {
        Self::Fixed(crop)
    }
}

/// Кадрирование + место одной транзакцией.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewfinderFraming {
    /// Какую часть сенсора берёт ISP (соотношение сторон и зум), по камере кадра.
    pub crop: FramingCrop,
    /// Куда на экране положить результат.
    pub destination: FramingDestination,
}

/// Действующее место видоискателя: что реально применяется на кадре (конфиг или команда).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectivePlacement {
    /// Правило из конфига.
    Configured(ViewfinderPlacement),
    /// Прямоугольник из команды [`ViewfinderFraming`].
    ScreenRect(NormalizedScreenRect),
}

impl EffectivePlacement {
    /// Место по настроенному правилу и месту из команды.
    pub fn resolve(configured: ViewfinderPlacement, destination: FramingDestination) -> Self {
        match destination {
            FramingDestination::Configured => Self::Configured(configured),
            FramingDestination::ScreenRect(rect) => Self::ScreenRect(rect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_constructor_accepts_valid_and_rejects_each_kind_of_broken_input() {
        let rect = NormalizedScreenRect::new(0.0, 0.25, 1.0, 0.5).expect("целиком внутри цели");
        assert_eq!(
            (rect.left(), rect.top(), rect.width(), rect.height()),
            (0.0, 0.25, 1.0, 0.5)
        );
        // Погрешность f32 на краю допустима.
        assert!(NormalizedScreenRect::new(0.7, 0.0, 0.3, 1.0).is_ok());

        assert!(matches!(
            NormalizedScreenRect::new(f32::NAN, 0.0, 0.5, 0.5),
            Err(ScreenRectError::NotFinite { name: "left", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(0.0, 0.0, f32::INFINITY, 0.5),
            Err(ScreenRectError::NotFinite { name: "width", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(-0.1, 0.0, 0.5, 0.5),
            Err(ScreenRectError::OriginOutOfRange { name: "left", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(0.0, 1.5, 0.5, 0.5),
            Err(ScreenRectError::OriginOutOfRange { name: "top", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(0.0, 0.0, 0.0, 0.5),
            Err(ScreenRectError::EmptySize { name: "width", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(0.0, 0.0, 0.5, -1.0),
            Err(ScreenRectError::EmptySize { name: "height", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(0.6, 0.0, 0.5, 0.5),
            Err(ScreenRectError::ExtendsPastTarget { name: "right", .. })
        ));
        assert!(matches!(
            NormalizedScreenRect::new(0.0, 0.6, 0.5, 0.5),
            Err(ScreenRectError::ExtendsPastTarget { name: "bottom", .. })
        ));
    }

    #[test]
    fn effective_placement_follows_the_command_and_falls_back_to_config() {
        let rect = NormalizedScreenRect::new(0.0, 0.0, 1.0, 0.5).expect("прямоугольник");
        assert_eq!(
            EffectivePlacement::resolve(ViewfinderPlacement::FitWidthTop, FramingDestination::Configured),
            EffectivePlacement::Configured(ViewfinderPlacement::FitWidthTop)
        );
        assert_eq!(
            EffectivePlacement::resolve(ViewfinderPlacement::FitWidthTop, FramingDestination::ScreenRect(rect)),
            EffectivePlacement::ScreenRect(rect)
        );
    }
}
