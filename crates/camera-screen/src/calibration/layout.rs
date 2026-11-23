//! Геометрия раздела калибровки: три зоны и нижний ряд в «глазных» координатах и перевод на поверхность.
//!
//! # Две системы координат
//! * **Глазная** (eye): так раздел видит человек. В портрете она совпадает с поверхностью; в альбоме ширина и высота
//!   меняются местами (ширина = высота поверхности). Все зоны заданы здесь, в pt, началом в левом верхнем углу.
//! * **Поверхность** (surface): портретный экран, на котором рисует рендер. Раздел в альбоме — это «повёрнутый кадр»:
//!   всё содержимое глазного прямоугольника рисуется на поверхности повёрнутым на [`CalibrationLayout::rotation_clockwise_degrees`]
//!   (то же соглашение, что у поворота значков, `orientation.rs`: телефон `LandscapeLeft` — значки на 90° по часовой).
//!
//! Владелец геометрии — `ScreenLayout` (решение 93): этот тип строится только им (`ScreenLayout::calibration_layout`).
//!
//! # Картинка прибора
//! Просится ровно размером зоны прибора в **глазных** пикселях (решение 202 и 14h: 1:1, иначе края следа темнеют
//! от фильтрации). Виджет рисует её в глазном прямоугольнике зоны, повёрнутым вместе со всем разделом; поле
//! развёртки ([`ScopeScan`]) говорит прибору, как читать показанную картинку «слева направо» глазом.

use scope_contract::{ScopeImageSize, ScopeImageSizeError, ScopeScan};
use shell_contract::orientation::DeviceOrientation;
use thiserror::Error;
use ui_core::geometry::{Point, Rect, Size};

use super::config::{CalibrationConfig, ZoneFractions};
use crate::layout::ScreenLayout;
use crate::mode_behaviour::LayoutPolicy;
use crate::orientation::icon_rotation_clockwise_degrees;
use crate::widgets_config::BottomLayout;

/// Ошибка построения раскладки раздела.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum CalibrationLayoutError {
    #[error("размер картинки прибора {width_px}×{height_px} px недопустим: {source}")]
    ScopeImageSize {
        width_px: u32,
        height_px: u32,
        source: ScopeImageSizeError,
    },
}

/// Четыре зоны раздела в глазных координатах. Вместе они ровно покрывают глазной прямоугольник без перекрытий.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationZones {
    /// Колонка ручек слева: на всю высоту над нижним рядом, под стеклом.
    pub knobs: Rect,
    /// Чистое превью справа сверху: ничего поверх, без стекла (решение 203: кусок большого кадра).
    pub clean_preview: Rect,
    /// Прибор справа снизу: под стеклом.
    pub scope: Rect,
    /// Нижний ряд на всю ширину: карусель режимов и кнопка меню «шесть точек».
    pub bottom_row: Rect,
}

/// Раскладка раздела калибровки для конкретной поверхности и положения телефона.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationLayout {
    /// Положение, под которое раздел перестроен (для политики «портрет всегда» — `Portrait`).
    eye_orientation: DeviceOrientation,
    /// Размер глазного прямоугольника, pt.
    eye_size: Size,
    /// Размер поверхности, pt.
    surface_size: Size,
    zones: CalibrationZones,
    menu_button_center: Point,
    carousel_center: Point,
    scope_image_size: ScopeImageSize,
    scope_scan: ScopeScan,
}

impl CalibrationLayout {
    /// Строит раскладку. Вызывается только из `ScreenLayout::calibration_layout`.
    ///
    /// `bottom` — числа нижней зоны PHOTO: в портрете кнопка меню и карусель стоят там же, где в PHOTO
    /// (решение 199), с той же привязкой к низу.
    pub(crate) fn build(
        screen: &ScreenLayout,
        config: &CalibrationConfig,
        bottom: &BottomLayout,
        policy: LayoutPolicy,
        device_orientation: DeviceOrientation,
    ) -> Result<Self, CalibrationLayoutError> {
        let eye_orientation = match policy {
            LayoutPolicy::PortraitFixed => DeviceOrientation::Portrait,
            LayoutPolicy::FollowsOrientation => device_orientation,
        };
        let surface_size = screen.screen_size();
        let eye_size = if is_landscape(eye_orientation) {
            Size::new(surface_size.height, surface_size.width)
        } else {
            surface_size
        };
        let (fractions, bottom_row_height_pt) = if is_landscape(eye_orientation) {
            (config.layout.landscape, config.layout.landscape_bottom_row_height_pt)
        } else {
            (config.layout.portrait, config.layout.portrait_bottom_row_height_pt)
        };
        let pixels_per_point = screen.point_scale().pixels_per_point();
        let zones = split_zones(eye_size, fractions, bottom_row_height_pt, pixels_per_point);

        let (menu_button_center, carousel_center) = if is_landscape(eye_orientation) {
            landscape_bottom_row_anchors(eye_size, &zones.bottom_row, bottom, surface_size.width)
        } else {
            portrait_bottom_row_anchors(screen, bottom)
        };

        let scope_image_size = scope_size_for_zone(zones.scope, pixels_per_point, config.max_scope_image_side_px)?;

        Ok(Self {
            eye_orientation,
            eye_size,
            surface_size,
            zones,
            menu_button_center,
            carousel_center,
            scope_image_size,
            scope_scan: scan_for(eye_orientation),
        })
    }

    /// Размер глазного прямоугольника, pt.
    pub fn eye_size(&self) -> Size {
        self.eye_size
    }

    /// Зоны раздела в глазных координатах.
    pub fn zones(&self) -> &CalibrationZones {
        &self.zones
    }

    /// Центр кнопки меню «шесть точек» (глазные координаты). Ø — `bottom.menu_button.diameter_pt` виджетов.
    pub fn menu_button_center(&self) -> Point {
        self.menu_button_center
    }

    /// Центр капсулы режимов (глазные координаты).
    pub fn carousel_center(&self) -> Point {
        self.carousel_center
    }

    /// Поворот всего содержимого раздела относительно поверхности, градусы по часовой.
    pub fn rotation_clockwise_degrees(&self) -> f32 {
        icon_rotation_clockwise_degrees(self.eye_orientation)
    }

    /// Положение, под которое построена раскладка (виджетам нужно, чтобы выбрать сторону/поворот).
    pub fn eye_orientation(&self) -> DeviceOrientation {
        self.eye_orientation
    }

    /// Размер картинки прибора в пикселях: ровно зона прибора, округлённая и зажатая в `1..=предел`.
    pub fn scope_image_size(&self) -> ScopeImageSize {
        self.scope_image_size
    }

    /// Как глаз читает показанную картинку слева направо в этом положении (поле запроса прибора).
    pub fn scope_scan(&self) -> ScopeScan {
        self.scope_scan
    }

    /// Точка глазных координат → точка поверхности (pt).
    pub fn eye_point_on_surface(&self, eye_point: Point) -> Point {
        let (dx, dy) = (
            eye_point.x - self.eye_size.width / 2.0,
            eye_point.y - self.eye_size.height / 2.0,
        );
        let (sx, sy) = match self.eye_orientation {
            DeviceOrientation::Portrait => (dx, dy),
            // Поворот на 90° по часовой в координатах «y вниз»: (dx, dy) → (−dy, dx).
            DeviceOrientation::LandscapeLeft => (-dy, dx),
            DeviceOrientation::PortraitUpsideDown => (-dx, -dy),
            // На 90° против часовой: (dx, dy) → (dy, −dx).
            DeviceOrientation::LandscapeRight => (dy, -dx),
        };
        Point::new(self.surface_size.width / 2.0 + sx, self.surface_size.height / 2.0 + sy)
    }

    /// Точка поверхности (pt) → точка глазных координат: касание на поверхности попадает в зону раздела.
    pub fn surface_point_in_eye(&self, surface_point: Point) -> Point {
        let (sx, sy) = (
            surface_point.x - self.surface_size.width / 2.0,
            surface_point.y - self.surface_size.height / 2.0,
        );
        let (dx, dy) = match self.eye_orientation {
            DeviceOrientation::Portrait => (sx, sy),
            DeviceOrientation::LandscapeLeft => (sy, -sx),
            DeviceOrientation::PortraitUpsideDown => (-sx, -sy),
            DeviceOrientation::LandscapeRight => (-sy, sx),
        };
        Point::new(self.eye_size.width / 2.0 + dx, self.eye_size.height / 2.0 + dy)
    }

    /// Глазной прямоугольник → ось-выровненный прямоугольник на поверхности (поворот на кратный 90°
    /// переводит прямоугольник в прямоугольник). Виджет кладёт сюда узел и поворачивает содержимое на
    /// [`Self::rotation_clockwise_degrees`].
    pub fn eye_rect_on_surface(&self, eye_rect: Rect) -> Rect {
        let first = self.eye_point_on_surface(Point::new(eye_rect.min_x(), eye_rect.min_y()));
        let second = self.eye_point_on_surface(Point::new(eye_rect.max_x(), eye_rect.max_y()));
        Rect::from_xywh(
            first.x.min(second.x),
            first.y.min(second.y),
            (first.x - second.x).abs(),
            (first.y - second.y).abs(),
        )
    }
}

fn is_landscape(orientation: DeviceOrientation) -> bool {
    matches!(
        orientation,
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight
    )
}

/// Поле развёртки по положению раздела (таблица `scope-contract/src/scan.rs`).
fn scan_for(eye_orientation: DeviceOrientation) -> ScopeScan {
    match eye_orientation {
        DeviceOrientation::Portrait => ScopeScan::Upright,
        DeviceOrientation::LandscapeLeft => ScopeScan::RotatedClockwise,
        DeviceOrientation::PortraitUpsideDown => ScopeScan::UpsideDown,
        DeviceOrientation::LandscapeRight => ScopeScan::RotatedCounterClockwise,
    }
}

/// Режет глазной прямоугольник на четыре зоны. Каждая граница вычисляется один раз и берётся обеими соседними
/// зонами, поэтому зоны сходятся встык без щелей и перекрытий.
///
/// Границы привязаны к сетке пикселей (`snap_to_pixel`): тогда зона прибора ровно целое число пикселей, и картинка
/// размером «зона в px» ложится 1:1, без полупикселя фильтрации на краях (решение 202).
fn split_zones(
    eye_size: Size,
    fractions: ZoneFractions,
    bottom_row_height_pt: f64,
    pixels_per_point: f64,
) -> CalibrationZones {
    let snap = |length_pt: f64| snap_to_pixel(length_pt, pixels_per_point);
    let content_height = snap((eye_size.height - bottom_row_height_pt).max(0.0));
    let column_boundary_x = snap(eye_size.width * fractions.knobs_width_fraction);
    let preview_boundary_y = snap(content_height * fractions.preview_height_fraction);
    let right_width = eye_size.width - column_boundary_x;
    CalibrationZones {
        knobs: Rect::from_xywh(0.0, 0.0, column_boundary_x, content_height),
        clean_preview: Rect::from_xywh(column_boundary_x, 0.0, right_width, preview_boundary_y),
        scope: Rect::from_xywh(
            column_boundary_x,
            preview_boundary_y,
            right_width,
            content_height - preview_boundary_y,
        ),
        bottom_row: Rect::from_xywh(0.0, content_height, eye_size.width, eye_size.height - content_height),
    }
}

/// Ближайшая граница на сетке пикселей, pt.
fn snap_to_pixel(length_pt: f64, pixels_per_point: f64) -> f64 {
    (length_pt * pixels_per_point).round() / pixels_per_point
}

/// Портрет: кнопка меню и капсула — на тех же местах, что в PHOTO (с привязкой к низу, решение 82).
fn portrait_bottom_row_anchors(screen: &ScreenLayout, bottom: &BottomLayout) -> (Point, Point) {
    let shift = screen.bottom_anchor_shift_pt();
    (
        Point::new(bottom.menu_button.center_x_pt, bottom.menu_button.center_y_pt + shift),
        Point::new(bottom.capsule_center_x_pt, bottom.capsule_center_y_pt + shift),
    )
}

/// Альбом: одна линия по середине нижнего ряда; кнопка меню — у правого края с тем же отступом, что в PHOTO,
/// капсула — по центру ширины.
fn landscape_bottom_row_anchors(
    eye_size: Size,
    bottom_row: &Rect,
    bottom: &BottomLayout,
    reference_width_pt: f64,
) -> (Point, Point) {
    let row_center_y = bottom_row.center().y;
    // Отступ центра кнопки от правого края в PHOTO: ширина эталона − центр по x (440 − 383,6 = 56,4).
    let menu_inset_from_right = reference_width_pt - bottom.menu_button.center_x_pt;
    (
        Point::new(eye_size.width - menu_inset_from_right, row_center_y),
        Point::new(eye_size.width / 2.0, row_center_y),
    )
}

/// Размер картинки прибора по зоне: pt → px по масштабу, округление, зажим в `1..=предел`.
fn scope_size_for_zone(
    zone: Rect,
    pixels_per_point: f64,
    max_side_px: u32,
) -> Result<ScopeImageSize, CalibrationLayoutError> {
    let width_px = pixels_for(zone.size.width, pixels_per_point, max_side_px);
    let height_px = pixels_for(zone.size.height, pixels_per_point, max_side_px);
    ScopeImageSize::try_new(width_px, height_px).map_err(|source| CalibrationLayoutError::ScopeImageSize {
        width_px,
        height_px,
        source,
    })
}

fn pixels_for(length_pt: f64, pixels_per_point: f64, max_side_px: u32) -> u32 {
    let rounded = (length_pt * pixels_per_point).round().max(1.0);
    // Зажим сверху до приведения к u32: огромное число не переполнит, а даст предел.
    if rounded >= f64::from(max_side_px) {
        max_side_px
    } else {
        rounded as u32
    }
}
