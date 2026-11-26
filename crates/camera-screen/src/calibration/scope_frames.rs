//! Рамки внутри зоны прибора: полоска сверху, область картинки под ней, поле уровней слева и сама картинка каждого
//! вида прибора (решения 258, 274, 291).
//!
//! ```text
//! ┌────────────────────────────── полоска (имя · точки · оси · белое) ┐  scope_strip_height_pt, 44
//! ├──────┬───────────────────────────────────────────────────────────┤  волосяная линия
//! │ 100  │                                                           │
//! │  50  │      картинка waveform / parade (вся область − поле)      │  область картинки
//! │   0  │                                                           │
//! └──────┴───────────────────────────────────────────────────────────┘
//! ```
//! Вектороскоп поля не имеет: его картинка — квадрат, вписанный в ВСЮ область картинки.
//!
//! Решение 202 в силе для области картинки: размер картинки в пикселях равен её прямоугольнику, а границы стоят на
//! целых пикселях, поэтому картинка ложится 1:1 без фильтрации. Всё считается один раз при сборке раскладки; этот
//! тип — единственный владелец «где какая картинка» (`ScreenLayout` владеет зонами, виджет рамок не выводит).

use scope_contract::{ScopeImageSize, ScopeKind};
use ui_core::geometry::{Point, Rect};

use super::config::CalibrationConfig;
use super::layout::{CalibrationLayoutError, snap_to_pixel};

/// Картинка одного вида прибора: куда её класть и сколько в ней пикселей.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScopePicture {
    kind: ScopeKind,
    /// Прямоугольник картинки в глазных координатах, pt: ровно `size` пикселей, края на целых пикселях.
    rect: Rect,
    size: ScopeImageSize,
}

/// Рамки зоны прибора.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScopeFrames {
    strip: Rect,
    picture_area: Rect,
    levels_gutter: Rect,
    /// Картинки видов в порядке `ScopeKind::ALL`.
    pictures: [ScopePicture; ScopeKind::ALL.len()],
}

impl ScopeFrames {
    /// Режет `zone` (зона прибора) на полоску и область картинки и ищет картинку каждого вида.
    pub(super) fn build(
        zone: Rect,
        config: &CalibrationConfig,
        pixels_per_point: f64,
    ) -> Result<Self, CalibrationLayoutError> {
        let snap = |length_pt: f64| snap_to_pixel(length_pt, pixels_per_point);
        let strip_height = snap(config.layout.scope_strip_height_pt);
        let gutter_width = snap(config.layout.scope_levels_gutter_pt);
        let strip = Rect::from_xywh(zone.min_x(), zone.min_y(), zone.size.width, strip_height);
        let picture_area = Rect::from_xywh(
            zone.min_x(),
            zone.min_y() + strip_height,
            zone.size.width,
            zone.size.height - strip_height,
        );
        // Поле уровней должно оставить картинке место, иначе у waveform нет картинки вообще.
        if picture_area.size.height < 1.0 / pixels_per_point || picture_area.size.width <= gutter_width {
            return Err(CalibrationLayoutError::NoRoomForScopePicture { scope: zone });
        }
        // Подпись уровня прижимается внутрь картинки по вертикали; картинка ниже подписи прижать не даёт.
        let label_height = config.layout.scope_level_label_height_pt;
        if picture_area.size.height < label_height {
            return Err(CalibrationLayoutError::ScopePictureShorterThanLevelLabels {
                scope: zone,
                picture_height_pt: picture_area.size.height,
                label_height_pt: label_height,
            });
        }
        let levels_gutter = Rect::from_xywh(
            picture_area.min_x(),
            picture_area.min_y(),
            gutter_width,
            picture_area.size.height,
        );
        // Поле нужно только тем видам, у которых есть подписи уровней; остальные берут всю область.
        let available_for = |kind: ScopeKind| {
            if kind.level_lines().is_empty() {
                picture_area
            } else {
                Rect::from_xywh(
                    picture_area.min_x() + gutter_width,
                    picture_area.min_y(),
                    picture_area.size.width - gutter_width,
                    picture_area.size.height,
                )
            }
        };
        let picture_of_kind = |kind: ScopeKind| {
            picture_in(
                kind,
                available_for(kind),
                config.max_scope_image_side_px,
                pixels_per_point,
            )
        };
        // Массив заполняется первой картинкой и перезаписывается по порядку словаря: у каждого вида своя ячейка.
        let mut pictures = [picture_of_kind(ScopeKind::ALL[0])?; ScopeKind::ALL.len()];
        for (slot, kind) in ScopeKind::ALL.into_iter().enumerate() {
            pictures[slot] = picture_of_kind(kind)?;
        }
        Ok(Self {
            strip,
            picture_area,
            levels_gutter,
            pictures,
        })
    }

    pub(super) fn strip(&self) -> Rect {
        self.strip
    }

    pub(super) fn picture_area(&self) -> Rect {
        self.picture_area
    }

    pub(super) fn levels_gutter(&self) -> Rect {
        self.levels_gutter
    }

    pub(super) fn image_rect_for(&self, kind: ScopeKind) -> Rect {
        self.picture_of(kind).rect
    }

    pub(super) fn image_size_for(&self, kind: ScopeKind) -> ScopeImageSize {
        self.picture_of(kind).size
    }

    fn picture_of(&self, kind: ScopeKind) -> &ScopePicture {
        // Картинки заведены для каждого вида словаря (`ScopeKind::ALL`), поэтому поиск не пуст; запасной ответ — первая.
        self.pictures
            .iter()
            .find(|picture| picture.kind == kind)
            .unwrap_or(&self.pictures[0])
    }
}

/// Картинка вида `kind` в доступном ей прямоугольнике `available`: форма вида (вся область или вписанный квадрат),
/// размер в пикселях зажат в предел, а сама картинка стоит по центру на целых пикселях.
fn picture_in(
    kind: ScopeKind,
    available: Rect,
    max_side_px: u32,
    pixels_per_point: f64,
) -> Result<ScopePicture, CalibrationLayoutError> {
    // Пиксели области как есть (`as` насыщает: огромное число даст предел `u32`, а не переполнение) и зажатые в предел
    // картинки: больше предела GPU картинку не строит, она стоит по центру области меньшей.
    let region_px = (
        (available.size.width * pixels_per_point).round().max(1.0) as u32,
        (available.size.height * pixels_per_point).round().max(1.0) as u32,
    );
    let limited_px = (region_px.0.min(max_side_px), region_px.1.min(max_side_px));
    let available_size = ScopeImageSize::try_new(limited_px.0, limited_px.1).map_err(|source| {
        CalibrationLayoutError::ScopeImageSize {
            width_px: limited_px.0,
            height_px: limited_px.1,
            source,
        }
    })?;
    let size = kind.picture_shape().fit_in(available_size);
    // Целые пиксели от левого верхнего угла области: полупиксельный сдвиг размыл бы края картинки (решение 202).
    let free_px = (
        f64::from(region_px.0 - size.width()),
        f64::from(region_px.1 - size.height()),
    );
    let origin = Point::new(
        available.min_x() + (free_px.0 / 2.0).floor() / pixels_per_point,
        available.min_y() + (free_px.1 / 2.0).floor() / pixels_per_point,
    );
    let rect = Rect::from_xywh(
        origin.x,
        origin.y,
        f64::from(size.width()) / pixels_per_point,
        f64::from(size.height()) / pixels_per_point,
    );
    Ok(ScopePicture { kind, rect, size })
}
