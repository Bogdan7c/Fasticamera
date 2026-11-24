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
//! # Безопасная зона
//! Управление раздела (колонка ручек, прибор, кнопка меню в альбоме) стоит внутри безопасной зоны поверхности: вырез
//! камеры его не закрывает, а касание не уходит системе (решения 95, 226). Зона приходит отступами от краёв
//! ПОВЕРХНОСТИ и переводится в глазные координаты тем же поворотом, что и вся раскладка: верх поверхности в альбоме —
//! боковой край для глаз. Чистое превью — живая картинка, оно по-прежнему до краёв экрана. Портретный нижний ряд стоит
//! на местах PHOTO (решения 82, 199) и зоной не сдвигается. Без отступов раскладка ровно та, что была до 14k.
//!
//! # Картинка прибора
//! Просится ровно размером зоны прибора в **глазных** пикселях (решение 202 и 14h: 1:1, иначе края следа темнеют
//! от фильтрации). Виджет рисует её в глазном прямоугольнике зоны, повёрнутым вместе со всем разделом; поле
//! развёртки ([`ScopeScan`]) говорит прибору, как читать показанную картинку «слева направо» глазом.

use scope_contract::{ScopeImageSize, ScopeImageSizeError, ScopeScan};
use shell_contract::edge_insets::ScreenEdge;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use thiserror::Error;
use ui_core::geometry::{Point, Rect, Size};

use super::config::{CalibrationConfig, ZoneFractions};
use super::eye::{EyeTransform, is_landscape};
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
    #[error("в безопасной зоне нет места: колонка ручек {knobs:?}, прибор {scope:?}")]
    NoRoomInSafeArea { knobs: Rect, scope: Rect },
}

/// Четыре зоны раздела в глазных координатах, без перекрытий. Без отступов безопасной зоны они ровно покрывают
/// глазной прямоугольник; с отступами полоса у края остаётся живой картинкой без управления.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationZones {
    /// Колонка ручек слева: от верха безопасной зоны до нижнего ряда, под стеклом.
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
    /// Перевод «глаза ↔ поверхность» под положение, для которого раздел перестроен.
    eye: EyeTransform,
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
        safe_area: SafeAreaInsets,
    ) -> Result<Self, CalibrationLayoutError> {
        let eye_orientation = match policy {
            LayoutPolicy::PortraitFixed => DeviceOrientation::Portrait,
            LayoutPolicy::FollowsOrientation => device_orientation,
        };
        let surface_size = screen.screen_size();
        let eye = EyeTransform::new(eye_orientation, surface_size);
        let eye_size = eye.eye_size();
        let (fractions, bottom_row_height_pt) = if is_landscape(eye_orientation) {
            (config.layout.landscape, config.layout.landscape_bottom_row_height_pt)
        } else {
            (config.layout.portrait, config.layout.portrait_bottom_row_height_pt)
        };
        let pixels_per_point = screen.point_scale().pixels_per_point();
        let safe_rect = eye.surface_rect_in_eye(safe_surface_rect(surface_size, safe_area, pixels_per_point));
        let zones = split_zones(eye_size, safe_rect, fractions, bottom_row_height_pt, pixels_per_point)?;

        let (menu_button_center, carousel_center) = if is_landscape(eye_orientation) {
            landscape_bottom_row_anchors(eye_size, safe_rect, &zones.bottom_row, bottom, surface_size.width)
        } else {
            portrait_bottom_row_anchors(screen, bottom)
        };

        let scope_image_size = scope_size_for_zone(zones.scope, pixels_per_point, config.max_scope_image_side_px)?;

        Ok(Self {
            eye,
            zones,
            menu_button_center,
            carousel_center,
            scope_image_size,
            scope_scan: scan_for(eye_orientation),
        })
    }

    /// Размер глазного прямоугольника, pt.
    pub fn eye_size(&self) -> Size {
        self.eye.eye_size()
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
        icon_rotation_clockwise_degrees(self.eye.orientation())
    }

    /// Положение, под которое построена раскладка (виджетам нужно, чтобы выбрать сторону/поворот).
    pub fn eye_orientation(&self) -> DeviceOrientation {
        self.eye.orientation()
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
        self.eye.eye_point_on_surface(eye_point)
    }

    /// Точка поверхности (pt) → точка глазных координат: касание на поверхности попадает в зону раздела.
    pub fn surface_point_in_eye(&self, surface_point: Point) -> Point {
        self.eye.surface_point_in_eye(surface_point)
    }

    /// Глазной прямоугольник → ось-выровненный прямоугольник на поверхности (поворот на кратный 90°
    /// переводит прямоугольник в прямоугольник). Виджет кладёт сюда узел и поворачивает содержимое на
    /// [`Self::rotation_clockwise_degrees`].
    pub fn eye_rect_on_surface(&self, eye_rect: Rect) -> Rect {
        self.eye.eye_rect_on_surface(eye_rect)
    }
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

/// Безопасная зона на поверхности, pt: экран без отступов у каждого края.
fn safe_surface_rect(surface_size: Size, safe_area: SafeAreaInsets, pixels_per_point: f64) -> Rect {
    let inset_pt = |edge: ScreenEdge| f64::from(safe_area.thickness_px(edge)) / pixels_per_point;
    let (left, top) = (inset_pt(ScreenEdge::Left), inset_pt(ScreenEdge::Top));
    Rect::from_xywh(
        left,
        top,
        surface_size.width - left - inset_pt(ScreenEdge::Right),
        surface_size.height - top - inset_pt(ScreenEdge::Bottom),
    )
}

/// Режет глазной прямоугольник на четыре зоны. Каждая граница вычисляется один раз и берётся обеими соседними
/// зонами, поэтому зоны сходятся встык без щелей и перекрытий.
///
/// Управление (ручки и прибор) стоит внутри безопасной зоны `safe`: колонка сдвигается от края и сохраняет свою ширину
/// (доля считается от всего экрана), прибор кончается у границы зоны. Линия «превью / прибор» от зоны не зависит:
/// картинка сверху идёт до края экрана.
///
/// Границы привязаны к сетке пикселей (`snap_to_pixel`): тогда зона прибора ровно целое число пикселей, и картинка
/// размером «зона в px» ложится 1:1, без полупикселя фильтрации на краях (решение 202).
fn split_zones(
    eye_size: Size,
    safe: Rect,
    fractions: ZoneFractions,
    bottom_row_height_pt: f64,
    pixels_per_point: f64,
) -> Result<CalibrationZones, CalibrationLayoutError> {
    let snap = |length_pt: f64| snap_to_pixel(length_pt, pixels_per_point);
    let content_bottom = snap((eye_size.height - bottom_row_height_pt).min(safe.max_y()).max(0.0));
    let (knobs_left, knobs_top, safe_right) = (snap(safe.min_x()), snap(safe.min_y()), snap(safe.max_x()));
    let column_boundary_x = knobs_left + snap(eye_size.width * fractions.knobs_width_fraction);
    // Отступ сверху толще всего превью (не бывает у телефонов) прибор всё равно не накроет.
    let preview_boundary_y = snap(content_bottom * fractions.preview_height_fraction).max(knobs_top);
    let zones = CalibrationZones {
        knobs: Rect::from_xywh(
            knobs_left,
            knobs_top,
            column_boundary_x - knobs_left,
            content_bottom - knobs_top,
        ),
        clean_preview: Rect::from_xywh(
            column_boundary_x,
            0.0,
            eye_size.width - column_boundary_x,
            preview_boundary_y,
        ),
        scope: Rect::from_xywh(
            column_boundary_x,
            preview_boundary_y,
            safe_right - column_boundary_x,
            content_bottom - preview_boundary_y,
        ),
        bottom_row: Rect::from_xywh(0.0, content_bottom, eye_size.width, eye_size.height - content_bottom),
    };
    let has_room = |zone: &Rect| zone.size.width > 0.0 && zone.size.height > 0.0;
    if has_room(&zones.knobs) && has_room(&zones.scope) {
        Ok(zones)
    } else {
        Err(CalibrationLayoutError::NoRoomInSafeArea {
            knobs: zones.knobs,
            scope: zones.scope,
        })
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

/// Альбом: одна линия по середине нижнего ряда; кнопка меню — у правого края безопасной зоны с тем же отступом, что
/// в PHOTO от края экрана (иначе она оказалась бы в полосе системного жеста и не нажималась), капсула — по центру
/// ширины.
fn landscape_bottom_row_anchors(
    eye_size: Size,
    safe: Rect,
    bottom_row: &Rect,
    bottom: &BottomLayout,
    reference_width_pt: f64,
) -> (Point, Point) {
    let row_center_y = bottom_row.center().y;
    // Отступ центра кнопки от правого края в PHOTO: ширина эталона − центр по x (440 − 383,6 = 56,4).
    let menu_inset_from_right = reference_width_pt - bottom.menu_button.center_x_pt;
    (
        Point::new(safe.max_x() - menu_inset_from_right, row_center_y),
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
