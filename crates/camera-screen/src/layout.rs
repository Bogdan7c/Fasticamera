//! Геометрия экрана: прямоугольник кадра видоискателя для каждого соотношения сторон.
//!
//! **Единственный владелец геометрии кадра** (решение 93): маска, перевод «тап → сенсор» и команда
//! рендеру «рисуй видоискатель сюда» берут прямоугольник отсюда и больше ниоткуда.
//!
//! # Правила (`docs/ios-camera/layout.md`, разделы 2 и 4)
//! 1. Масштаб по ширине (решение 82): `1 pt = ширина surface / 440 px`.
//! 2. Раскладка привязана к низу: все координаты iPhone (экран высотой 956 pt) сдвигаются вниз на
//!    `высота нашего экрана в pt − 956` — лишняя высота уходит в чёрную полосу сверху.
//! 3. Кадр занимает всю ширину экрана; 16:9 и низ кадра 4:3 заданы измерениями, верх 4:3 и кадр 1:1
//!    выводятся из соотношения сторон (1:1 стоит по центру кадра 4:3).

use std::sync::Arc;

use camera_contract::ids::LensFacing;
use camera_contract::viewfinder_crop::CropAspect;
use camera_contract::zoom::{ZoomCatalog, ZoomFraming, ZoomRatio};
use serde::{Deserialize, Serialize};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use shell_contract::surface::SurfaceSize;
use thiserror::Error;
use ui_core::geometry::{Point, Rect, Size};
use ui_core::input::{PointScale, PointScaleError, PointScaleRule};

use crate::calibration::{CalibrationConfig, CalibrationLayout, CalibrationLayoutError};
use crate::mode_behaviour::LayoutPolicy;
use crate::settings::AspectRatio;
use crate::widgets_config::BottomLayout;

/// Числа раскладки в координатах эталонного iPhone (все в pt, привязка к низу — в [`ScreenLayout`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    /// Правило «пиксели → pt». `[review]` 440 pt по ширине (решение 82, `layout.md` 4.1).
    pub point_scale: PointScaleRule,
    /// Высота эталонного экрана. `[review]` 956 pt у iPhone 17 Pro Max (`layout.md` раздел 1).
    pub reference_height_pt: f64,
    /// Верх кадра 16:9. `[measured]` 75,7 pt по видео A (`layout.md` 2.1).
    pub frame_16x9_top_pt: f64,
    /// Низ кадра 16:9. `[measured]` 858,2 pt по видео A.
    pub frame_16x9_bottom_pt: f64,
    /// Низ кадра 4:3. `[measured: R00 t=10.027]` 746,0 pt; верх выводится из 4:3 (159,3).
    pub frame_4x3_bottom_pt: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            point_scale: PointScaleRule::default(),
            reference_height_pt: 956.0,
            frame_16x9_top_pt: 75.7,
            frame_16x9_bottom_pt: 858.2,
            frame_4x3_bottom_pt: 746.0,
        }
    }
}

impl LayoutConfig {
    /// Проверка значений конфига (не зависит от размера surface).
    pub fn validate(&self) -> Result<(), LayoutError> {
        PointScale::for_surface_width(self.point_scale, 1).map_err(LayoutError::Scale)?;
        let values = [
            ("reference_height_pt", self.reference_height_pt),
            ("frame_16x9_top_pt", self.frame_16x9_top_pt),
            ("frame_16x9_bottom_pt", self.frame_16x9_bottom_pt),
            ("frame_4x3_bottom_pt", self.frame_4x3_bottom_pt),
        ];
        for (field, value) in values {
            if !value.is_finite() || value <= 0.0 {
                return Err(LayoutError::InvalidValue { field, value });
            }
        }
        if self.frame_16x9_top_pt >= self.frame_16x9_bottom_pt {
            return Err(LayoutError::InvalidValue {
                field: "frame_16x9_top_pt (должен быть выше низа кадра)",
                value: self.frame_16x9_top_pt,
            });
        }
        let max_bottom = self.frame_16x9_bottom_pt.max(self.frame_4x3_bottom_pt);
        if max_bottom > self.reference_height_pt {
            return Err(LayoutError::InvalidValue {
                field: "низ кадра не может быть ниже эталонного экрана",
                value: max_bottom,
            });
        }
        Ok(())
    }
}

/// Ошибка раскладки.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum LayoutError {
    #[error("масштаб pt: {0}")]
    Scale(PointScaleError),
    #[error("значение раскладки {field} недопустимо: {value}")]
    InvalidValue { field: &'static str, value: f64 },
    #[error("высота surface {height_px} px равна нулю")]
    EmptySurface { height_px: u32 },
    #[error(
        "кадр {aspect:?} выходит за экран: верх {top_pt:.1} pt, низ {bottom_pt:.1} pt при высоте {screen_height_pt:.1} pt"
    )]
    FrameOutsideScreen {
        aspect: AspectRatio,
        top_pt: f64,
        bottom_pt: f64,
        screen_height_pt: f64,
    },
}

/// Прямоугольник в долях surface (0..1, начало — левый верхний угол). Так его понимает рендер.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct NormalizedScreenRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Команда рендеру: какое кадрирование сенсора нарисовать и в какое место экрана.
///
/// С сессии 13 кадрирование — не готовый прямоугольник, а **входы** общей формулы [`ZoomFraming`]
/// (соотношение сторон, коэффициент зума, каталог камер): рендер сам применяет её к камере КАЖДОГО кадра
/// (`ZoomFraming::crop_for_camera`), поэтому кадр старой камеры при переключении не получает кадрирование
/// новой (решение 93). Экран считает по той же формуле перевод тап ↔ сенсор.
#[derive(Debug, Clone, PartialEq)]
pub struct FramingIntent {
    /// Входы формулы кадрирования.
    pub zoom: ZoomFraming,
    /// Куда на экране её положить.
    pub destination: NormalizedScreenRect,
}

/// Раскладка кадров для конкретного размера surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenLayout {
    /// Размер surface в пикселях: нужен, чтобы кадр «на весь экран» получил точное целое соотношение сторон.
    surface: SurfaceSize,
    scale: PointScale,
    screen_size: Size,
    bottom_anchor_shift_pt: f64,
    frame_4x3: Rect,
    frame_16x9: Rect,
    frame_square: Rect,
}

impl ScreenLayout {
    /// Раскладка из конфига и размера surface. Проверяет, что все три кадра помещаются на экран.
    pub fn new(config: &LayoutConfig, surface: SurfaceSize) -> Result<Self, LayoutError> {
        config.validate()?;
        if surface.height_px == 0 {
            return Err(LayoutError::EmptySurface {
                height_px: surface.height_px,
            });
        }
        let scale = PointScale::for_surface_width(config.point_scale, surface.width_px).map_err(LayoutError::Scale)?;
        let screen_size = Size::new(
            config.point_scale.reference_width_pt,
            f64::from(surface.height_px) / scale.pixels_per_point(),
        );
        // Привязка к низу (решение 82): всё, что у iPhone считалось от верха, едет вниз на разницу высот.
        let shift = screen_size.height - config.reference_height_pt;
        let width = screen_size.width;
        // 16:9 — по измеренным верху и низу.
        let frame_16x9 = Rect::from_xywh(
            0.0,
            config.frame_16x9_top_pt + shift,
            width,
            config.frame_16x9_bottom_pt - config.frame_16x9_top_pt,
        );
        // 4:3 в портрете: высота = ширина × 4/3, низ измерен.
        let height_4x3 = width * 4.0 / 3.0;
        let bottom_4x3 = config.frame_4x3_bottom_pt + shift;
        let frame_4x3 = Rect::from_xywh(0.0, bottom_4x3 - height_4x3, width, height_4x3);
        // 1:1 — `[estimate]` по центру кадра 4:3 (`layout.md` 2.1).
        let frame_square = Rect::centered_at(frame_4x3.center(), Size::new(width, width));
        let layout = Self {
            surface,
            scale,
            screen_size,
            bottom_anchor_shift_pt: shift,
            frame_4x3,
            frame_16x9,
            frame_square,
        };
        layout.check_frames_on_screen()?;
        Ok(layout)
    }

    fn check_frames_on_screen(&self) -> Result<(), LayoutError> {
        for aspect in AspectRatio::ALL {
            let rect = self.frame_rect(aspect);
            if rect.min_y() < 0.0 || rect.max_y() > self.screen_size.height {
                return Err(LayoutError::FrameOutsideScreen {
                    aspect,
                    top_pt: rect.min_y(),
                    bottom_pt: rect.max_y(),
                    screen_height_pt: self.screen_size.height,
                });
            }
        }
        Ok(())
    }

    /// Масштаб «пиксели ↔ pt» этой surface (виджеты переводят им касания и размеры).
    pub fn point_scale(&self) -> PointScale {
        self.scale
    }

    /// Размер экрана в pt (ширина всегда эталонная, высота — по пропорциям surface).
    pub fn screen_size(&self) -> Size {
        self.screen_size
    }

    /// На сколько pt вся раскладка iPhone сдвинута вниз (наша высота − эталонная).
    pub fn bottom_anchor_shift_pt(&self) -> f64 {
        self.bottom_anchor_shift_pt
    }

    /// Прямоугольник кадра видоискателя для соотношения, pt.
    pub fn frame_rect(&self, aspect: AspectRatio) -> Rect {
        match aspect {
            AspectRatio::FourByThree => self.frame_4x3,
            AspectRatio::SixteenByNine => self.frame_16x9,
            AspectRatio::Square => self.frame_square,
        }
    }

    /// Прямоугольник кадра в долях surface.
    pub fn frame_rect_normalized(&self, aspect: AspectRatio) -> NormalizedScreenRect {
        let rect = self.frame_rect(aspect);
        NormalizedScreenRect {
            left: (rect.min_x() / self.screen_size.width) as f32,
            top: (rect.min_y() / self.screen_size.height) as f32,
            width: (rect.size.width / self.screen_size.width) as f32,
            height: (rect.size.height / self.screen_size.height) as f32,
        }
    }

    /// Команда рендеру для соотношения: входы формулы кадрирования + место на экране. Кадрирование
    /// собирается только здесь (одно место для экрана и рендера).
    pub fn framing_intent(
        &self,
        aspect: AspectRatio,
        facing: LensFacing,
        ratio: ZoomRatio,
        catalog: Arc<ZoomCatalog>,
    ) -> FramingIntent {
        FramingIntent {
            zoom: ZoomFraming {
                aspect: aspect.crop_aspect(),
                facing,
                ratio,
                catalog,
            },
            destination: self.frame_rect_normalized(aspect),
        }
    }

    /// Весь экран как кадр видоискателя (режим «cover», решение 203), pt.
    pub fn cover_frame_rect(&self) -> Rect {
        Rect::new(Point::new(0.0, 0.0), self.screen_size)
    }

    /// Соотношение сторон кадра «на весь экран»: пиксели surface как целые числа (длинная : короткая; ориентацию
    /// кадра задаёт сенсор, а не это значение).
    pub fn cover_crop_aspect(&self) -> CropAspect {
        let long = self.surface.width_px.max(self.surface.height_px);
        let short = self.surface.width_px.min(self.surface.height_px);
        CropAspect::LongToShort { long, short }
    }

    /// Команда рендеру для видоискателя на весь экран: входы той же формулы кадрирования, что и у обычного кадра
    /// (`framing_intent`), но соотношение — экрана, а место — вся поверхность (решение 203: один видоискатель,
    /// стекло ручек и прибора лежит поверх, чистая четверть — просто кусок большого кадра).
    pub fn cover_framing_intent(
        &self,
        facing: LensFacing,
        ratio: ZoomRatio,
        catalog: Arc<ZoomCatalog>,
    ) -> FramingIntent {
        FramingIntent {
            zoom: ZoomFraming {
                aspect: self.cover_crop_aspect(),
                facing,
                ratio,
                catalog,
            },
            destination: NormalizedScreenRect {
                left: 0.0,
                top: 0.0,
                width: 1.0,
                height: 1.0,
            },
        }
    }

    /// Раскладка раздела калибровки для политики режима, положения телефона и безопасной зоны. Единственное место, где она строится
    /// (решение 93: геометрию держит `ScreenLayout`).
    pub fn calibration_layout(
        &self,
        config: &CalibrationConfig,
        bottom: &BottomLayout,
        policy: LayoutPolicy,
        device_orientation: DeviceOrientation,
        safe_area: SafeAreaInsets,
    ) -> Result<CalibrationLayout, CalibrationLayoutError> {
        CalibrationLayout::build(self, config, bottom, policy, device_orientation, safe_area)
    }

    /// Лежит ли точка (pt) внутри кадра выбранного соотношения.
    pub fn frame_contains(&self, aspect: AspectRatio, point: Point) -> bool {
        self.frame_rect(aspect).contains(point)
    }
}
