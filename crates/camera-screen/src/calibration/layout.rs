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
//! боковой край для глаз. Чистое превью — живая картинка, оно по-прежнему до краёв экрана. Без отступов зоны ровно
//! покрывают глазной прямоугольник.
//!
//! # Форма зон и нижний ряд (14n)
//! Как резать глазной прямоугольник, говорит [`ZoneShape`] ориентации из конфига (решения 255, 260), а с какой стороны
//! колонка ручек — [`KnobsSide`] (решение 275). Нижний ряд — свои якоря раздела: капсула режимов на своей линии
//! ([`CapsuleLine`]), кнопка меню у правого, CLEAN у левого края безопасной зоны на той же линии (решения 256, 259).
//! Числа PHOTO раздел больше не читает.
//!
//! # Картинка прибора
//! Просится ровно размером своего прямоугольника в **глазных** пикселях (решение 202 и 14h: 1:1, иначе края следа
//! темнеют от фильтрации). Зона прибора режется на полоску, поле уровней и область картинки, а прямоугольник
//! картинки каждого вида считает [`ScopeFrames`]: waveform и parade — область без поля уровней, вектороскоп —
//! квадрат, вписанный во всю область (круг на прямоугольнике стал бы овалом, решение 14m). Поле развёртки
//! ([`ScopeScan`]) говорит прибору, как читать показанную картинку «слева направо» глазом.

use scope_contract::{ScopeImageSize, ScopeImageSizeError, ScopeKind, ScopeScan};

use shell_contract::edge_insets::ScreenEdge;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use thiserror::Error;
use ui_core::geometry::{Point, Rect, Size};

use super::config::CalibrationConfig;
use super::eye::{EyeTransform, is_landscape};
use super::layout_config::{CapsuleLine, KnobsSide, OrientationLayoutConfig, ZoneShape};
use super::scope_frames::ScopeFrames;
use crate::layout::ScreenLayout;
use crate::mode_behaviour::LayoutPolicy;
use crate::orientation::icon_rotation_clockwise_degrees;

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
    #[error("в зоне прибора {scope:?} не остаётся места для картинки под полоской")]
    NoRoomForScopePicture { scope: Rect },
    #[error("картинка прибора в зоне {scope:?} ниже подписи уровня ({picture_height_pt} pt < {label_height_pt} pt)")]
    ScopePictureShorterThanLevelLabels {
        scope: Rect,
        picture_height_pt: f64,
        label_height_pt: f64,
    },
}

/// Четыре зоны раздела в глазных координатах, без перекрытий. Без отступов безопасной зоны они ровно покрывают
/// глазной прямоугольник; с отступами полоса у края остаётся живой картинкой без управления.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationZones {
    /// Колонка ручек (сторону задаёт `KnobsSide`): от верха безопасной зоны до линии «верх / прибор» в портрете
    /// или до нижнего ряда в альбоме.
    pub knobs: Rect,
    /// Чистое превью на другой половине сверху: ничего поверх, без стекла (решение 203: кусок большого кадра).
    pub clean_preview: Rect,
    /// Прибор: в портрете на всю ширину безопасной зоны под ручками и превью, в альбоме под превью.
    pub scope: Rect,
    /// Нижний ряд на всю ширину: CLEAN, карусель режимов и кнопка меню «шесть точек».
    pub bottom_row: Rect,
}

/// Раскладка раздела калибровки для конкретной поверхности и положения телефона.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationLayout {
    /// Перевод «глаза ↔ поверхность» под положение, для которого раздел перестроен.
    eye: EyeTransform,
    zones: CalibrationZones,
    menu_button_center: Point,
    clean_button_center: Point,
    carousel_center: Point,
    /// Полоска, область картинки, поле уровней и картинка каждого вида прибора.
    scope_frames: ScopeFrames,
    scope_scan: ScopeScan,
}

impl CalibrationLayout {
    /// Строит раскладку. Вызывается только из `ScreenLayout::calibration_layout`.
    pub(crate) fn build(
        screen: &ScreenLayout,
        config: &CalibrationConfig,
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
        let orientation_config = if is_landscape(eye_orientation) {
            &config.layout.landscape
        } else {
            &config.layout.portrait
        };
        let pixels_per_point = screen.point_scale().pixels_per_point();
        let safe_rect = eye.surface_rect_in_eye(safe_surface_rect(surface_size, safe_area, pixels_per_point));
        let zones = split_zones(
            eye_size,
            safe_rect,
            (orientation_config, config.layout.knobs_side),
            pixels_per_point,
        )?;
        let anchors = BottomRowAnchors::of(
            orientation_config.capsule_line,
            (screen.bottom_anchor_shift_pt(), config.layout.side_button_inset_pt),
            (eye_size, safe_rect, &zones.bottom_row),
        );
        let scope_frames = ScopeFrames::build(zones.scope, config, pixels_per_point)?;

        Ok(Self {
            eye,
            zones,
            menu_button_center: anchors.menu_button,
            clean_button_center: anchors.clean_button,
            carousel_center: anchors.carousel,
            scope_frames,
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

    /// Центр кнопки CLEAN (глазные координаты): на линии капсулы, зеркально кнопке меню (решение 259).
    pub fn clean_button_center(&self) -> Point {
        self.clean_button_center
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

    /// Полоска прибора сверху зоны: имя, точки страниц, оси и белое (решения 258, 291).
    pub fn scope_strip(&self) -> Rect {
        self.scope_frames.strip()
    }

    /// Область картинки прибора: зона без полоски (рамка картинок и поля уровней). Свайп листает приборы по ВСЕЙ зоне
    /// прибора, включая полоску (`CalibrationZones::scope`), а не только здесь.
    pub fn scope_picture_area(&self) -> Rect {
        self.scope_frames.picture_area()
    }

    /// Поле уровней слева в области картинки: в нём числа waveform и parade (решение 274).
    pub fn scope_levels_gutter(&self) -> Rect {
        self.scope_frames.levels_gutter()
    }

    /// Прямоугольник картинки прибора вида `kind`: ровно столько, сколько в [`Self::scope_image_size_for`] пикселей.
    pub fn scope_image_rect_for(&self, kind: ScopeKind) -> Rect {
        self.scope_frames.image_rect_for(kind)
    }

    /// Размер картинки прибора вида `kind` в пикселях: равен её прямоугольнику ([`Self::scope_image_rect_for`]).
    /// Источник размера для запроса прибора и для рисования его картинки.
    pub fn scope_image_size_for(&self, kind: ScopeKind) -> ScopeImageSize {
        self.scope_frames.image_size_for(kind)
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

/// Горизонтальные промежутки зон по стороне колонки ручек, глазные pt.
struct ColumnSpans {
    /// Колонка ручек: от внешнего края безопасной зоны на свою ширину.
    knobs: (f64, f64),
    /// Чистое превью: от границы колонки до края ЭКРАНА (картинка идёт под системные отступы).
    preview: (f64, f64),
    /// Прибор рядом с колонкой (форма `ScopeUnderPreview`): от границы колонки до другого края безопасной зоны.
    scope_beside_column: (f64, f64),
}

impl ColumnSpans {
    fn of(side: KnobsSide, (safe_left, safe_right): (f64, f64), (eye_width, column_width): (f64, f64)) -> Self {
        match side {
            KnobsSide::Left => {
                let boundary = safe_left + column_width;
                Self {
                    knobs: (safe_left, boundary),
                    preview: (boundary, eye_width),
                    scope_beside_column: (boundary, safe_right),
                }
            }
            // Справа колонка стоит у ПРОТИВОПОЛОЖНОГО безопасного края, а не зеркалится по x: отступы безопасной зоны
            // у сторон разные (вырез камеры, полоса жестов).
            KnobsSide::Right => {
                let boundary = safe_right - column_width;
                Self {
                    knobs: (boundary, safe_right),
                    preview: (0.0, boundary),
                    scope_beside_column: (safe_left, boundary),
                }
            }
        }
    }
}

/// Режет глазной прямоугольник на четыре зоны. Каждая граница вычисляется один раз и берётся обеими соседними
/// зонами, поэтому зоны сходятся встык без щелей и перекрытий. Как резать, говорит [`ZoneShape`] ориентации.
///
/// Управление (ручки и прибор) стоит внутри безопасной зоны `safe`: колонка сдвигается от края и сохраняет свою ширину
/// (доля считается от всего экрана), прибор кончается у границы зоны. Линия «верх / прибор» от зоны не зависит:
/// картинка сверху идёт до края экрана.
///
/// Границы привязаны к сетке пикселей (`snap_to_pixel`): тогда зона прибора ровно целое число пикселей, и картинка
/// размером «зона в px» ложится 1:1, без полупикселя фильтрации на краях (решение 202).
fn split_zones(
    eye_size: Size,
    safe: Rect,
    (orientation, side): (&OrientationLayoutConfig, KnobsSide),
    pixels_per_point: f64,
) -> Result<CalibrationZones, CalibrationLayoutError> {
    let snap = |length_pt: f64| snap_to_pixel(length_pt, pixels_per_point);
    let content_bottom = snap(
        (eye_size.height - orientation.bottom_row_height_pt)
            .min(safe.max_y())
            .max(0.0),
    );
    let safe_top = snap(safe.min_y());
    let safe_x = (snap(safe.min_x()), snap(safe.max_x()));
    let column_width = snap(eye_size.width * orientation.knobs_width_fraction);
    let spans = ColumnSpans::of(side, safe_x, (eye_size.width, column_width));
    // Отступ сверху толще всего превью (не бывает у телефонов) прибор всё равно не накроет.
    let preview_boundary_y = snap(content_bottom * orientation.preview_height_fraction).max(safe_top);
    let band = |(from, to): (f64, f64), (top, bottom): (f64, f64)| Rect::from_xywh(from, top, to - from, bottom - top);
    let (knobs_bottom, scope_span) = match orientation.shape {
        // Прибор лежит под обеими половинами верха на всю ширину безопасной зоны; колонка кончается у линии владельца.
        ZoneShape::ScopeStripBelow => (preview_boundary_y, safe_x),
        // Колонка идёт на всю высоту, прибор — под превью.
        ZoneShape::ScopeUnderPreview => (content_bottom, spans.scope_beside_column),
    };
    let zones = CalibrationZones {
        knobs: band(spans.knobs, (safe_top, knobs_bottom)),
        clean_preview: band(spans.preview, (0.0, preview_boundary_y)),
        scope: band(scope_span, (preview_boundary_y, content_bottom)),
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
pub(super) fn snap_to_pixel(length_pt: f64, pixels_per_point: f64) -> f64 {
    (length_pt * pixels_per_point).round() / pixels_per_point
}

/// Центры трёх элементов нижнего ряда, глазные pt.
struct BottomRowAnchors {
    menu_button: Point,
    clean_button: Point,
    carousel: Point,
}

impl BottomRowAnchors {
    /// Капсула стоит по центру ширины на линии `line`; кнопка меню и CLEAN — на той же линии, на `side_inset_pt`
    /// от правого и левого краёв безопасной зоны: ближе кнопка попала бы в полосу системного жеста и не нажималась
    /// (решения 226, 256, 259).
    fn of(
        line: CapsuleLine,
        (bottom_anchor_shift_pt, side_inset_pt): (f64, f64),
        (eye_size, safe, bottom_row): (Size, Rect, &Rect),
    ) -> Self {
        let line_y = match line {
            CapsuleLine::ReferenceAnchoredToBottom { reference_center_y_pt } => {
                reference_center_y_pt + bottom_anchor_shift_pt
            }
            CapsuleLine::BottomRowCentre => bottom_row.center().y,
        };
        Self {
            menu_button: Point::new(safe.max_x() - side_inset_pt, line_y),
            clean_button: Point::new(safe.min_x() + side_inset_pt, line_y),
            carousel: Point::new(eye_size.width / 2.0, line_y),
        }
    }
}
