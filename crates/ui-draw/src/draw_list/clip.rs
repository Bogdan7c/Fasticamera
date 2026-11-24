//! Прямоугольная обрезка содержимого: группа элементов, видимых только внутри прямоугольника экрана.
//!
//! Простыми словами: прокручиваемый список рисует строки, которые частично выезжают за панель. Группа
//! [`ClipGroup`] говорит рендеру «всё, что внутри, показывай только в этом прямоугольнике». Смысл обрезки
//! (какие пиксели остаются) определён здесь, в одном месте, а рендер лишь исполняет его: так формулы не
//! расходятся между `ui-draw` и `ui-render-vulkan`.
//!
//! # Правило центра пикселя
//! Пиксель `(x, y)` принадлежит обрезке тогда и только тогда, когда его центр `(x + 0,5; y + 0,5)` лежит в
//! полуоткрытом прямоугольнике `[min, max)`. Для целых границ это обычный scissor; для дробных границ пиксель
//! либо остаётся целиком, либо отсекается целиком (полупрозрачных краёв обрезка не даёт).
//! Правило записано дважды и тестом сверено: поточечно — [`clip_keeps_pixel`], целыми границами —
//! [`clip_pixel_rect`] (его и отдаём рендеру как scissor).
//!
//! # Вложенность
//! Группы можно вкладывать: действующая обрезка листа — пересечение прямоугольников всех охватывающих групп
//! ([`EffectiveClip`]). Обход листьев с их действующей обрезкой ([`clipped_leaves`]) живёт здесь, чтобы
//! рендеры не повторяли логику вложенности.

use crate::draw_list::items::DrawItem;
use crate::geometry::{PxPoint, PxRect, PxSize};

/// Группа элементов, видимых только внутри прямоугольника экрана.
///
/// `rect` задан в ПИКСЕЛЯХ ЭКРАНА, со сторонами вдоль осей (не в локальных координатах элементов).
/// Порядок рисования внутри группы и относительно соседей сохраняется.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipGroup {
    /// Прямоугольник обрезки (экранные px). Нулевой размер допустим: обрезка пуста, группа ничего не рисует.
    pub rect: PxRect,
    /// Содержимое в порядке рисования; может содержать вложенные группы.
    pub items: Vec<DrawItem>,
}

/// Поточечная форма правила: остаётся ли пиксель с центром `pixel_center` внутри обрезки `rect`.
pub fn clip_keeps_pixel(rect: &PxRect, pixel_center: PxPoint) -> bool {
    pixel_center.x >= rect.origin.x
        && pixel_center.x < rect.max_x()
        && pixel_center.y >= rect.origin.y
        && pixel_center.y < rect.max_y()
}

/// Целая форма правила: прямоугольник пикселей (границы — целые числа), центры которых проходят
/// [`clip_keeps_pixel`]. Левая граница — `ceil(min − 0,5)`, правая — `ceil(max − 0,5)`: пиксель `p` остаётся
/// при `p + 0,5 ≥ min` и `p + 0,5 < max`. Схлопнутая или нечисловая обрезка даёт пустой прямоугольник
/// (нулевой размер).
pub fn clip_pixel_rect(rect: &PxRect) -> PxRect {
    let (min_x, min_y) = (rect.origin.x, rect.origin.y);
    let (max_x, max_y) = (rect.max_x(), rect.max_y());
    if ![min_x, min_y, max_x, max_y].iter().all(|edge| edge.is_finite()) {
        return PxRect::default();
    }
    let left = (min_x - 0.5).ceil();
    let top = (min_y - 0.5).ceil();
    // Правая граница не может оказаться левее левой: иначе размер стал бы отрицательным.
    let right = (max_x - 0.5).ceil().max(left);
    let bottom = (max_y - 0.5).ceil().max(top);
    PxRect::new(PxPoint::new(left, top), PxSize::new(right - left, bottom - top))
}

impl ClipGroup {
    /// Пиксели, оставшиеся после обрезки этой группы (без учёта охватывающих): см. [`clip_pixel_rect`].
    pub fn pixel_rect(&self) -> PxRect {
        clip_pixel_rect(&self.rect)
    }

    /// Квад рисования группы: объединение квадов рисования детей, обрезанное пикселями группы (и вложенных
    /// групп). `None` — после обрезки рисовать нечего.
    pub fn draw_rect_px(&self) -> Option<PxRect> {
        self.union_of_visible_parts(|leaf| leaf.draw_rect_px())
    }

    /// Консервативные границы группы: объединение границ детей. Стекло сохраняет свои полные границы
    /// (прямоугольник адаптации яркости читает фон и вне обрезки), остальные дети обрезаются.
    pub fn bounds_px(&self) -> Option<PxRect> {
        let outer = self.pixel_rect();
        union_of(clipped_leaves(&self.items).filter_map(|leaf| match leaf.item {
            DrawItem::Glass(glass) => glass.bounds_px(),
            other => leaf.clip.visible_part(other.bounds_px()?)?.intersection(&outer),
        }))
    }

    /// Объединение видимых частей прямоугольников листьев, выбранных `rect_of`.
    fn union_of_visible_parts(&self, rect_of: impl Fn(&DrawItem) -> Option<PxRect>) -> Option<PxRect> {
        let outer = self.pixel_rect();
        union_of(
            clipped_leaves(&self.items)
                .filter_map(|leaf| leaf.clip.visible_part(rect_of(leaf.item)?)?.intersection(&outer)),
        )
    }
}

/// Объединение прямоугольников; `None` для пустого набора.
fn union_of(rects: impl Iterator<Item = PxRect>) -> Option<PxRect> {
    rects.reduce(|accumulated, rect| accumulated.union(&rect))
}

/// Обрезка, действующая на лист: пересечение прямоугольников всех охватывающих групп.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectiveClip {
    /// Охватывающих групп нет: лист виден целиком.
    WholeTarget,
    /// Пересечение прямоугольников групп (экранные px, границы могут быть дробными). Пересечение, в котором
    /// не осталось площади, хранится как прямоугольник нулевого размера.
    Rect(PxRect),
}

impl EffectiveClip {
    /// Обрезка после входа в ещё одну группу с прямоугольником `rect`.
    pub fn narrowed_by(self, rect: PxRect) -> EffectiveClip {
        match self {
            EffectiveClip::WholeTarget => EffectiveClip::Rect(rect),
            EffectiveClip::Rect(current) => EffectiveClip::Rect(
                current
                    .intersection(&rect)
                    .unwrap_or_else(|| PxRect::new(current.origin, PxSize::default())),
            ),
        }
    }

    /// Пиксели, которые обрезка оставляет, как целый прямоугольник ([`clip_pixel_rect`]).
    /// `None` — обрезки нет (виден весь экран). Пересечение пиксельных правил равно пиксельному правилу
    /// пересечения, поэтому вложенные группы сворачиваются в один прямоугольник без потерь.
    pub fn pixel_rect(self) -> Option<PxRect> {
        match self {
            EffectiveClip::WholeTarget => None,
            EffectiveClip::Rect(rect) => Some(clip_pixel_rect(&rect)),
        }
    }

    /// Пикселей не остаётся вовсе (обрезка схлопнулась).
    pub fn keeps_nothing(self) -> bool {
        self.pixel_rect()
            .is_some_and(|pixels| !(pixels.size.width > 0.0 && pixels.size.height > 0.0))
    }

    /// Часть `rect`, которая может оказаться видимой: пересечение с пикселями обрезки.
    /// `None` — ничего не видно (пересечения нет или обрезка пуста).
    pub fn visible_part(self, rect: PxRect) -> Option<PxRect> {
        match self.pixel_rect() {
            None => Some(rect),
            Some(pixels) => rect.intersection(&pixels),
        }
    }
}

/// Лист (не группа обрезки) с обрезкой, которая на него действует.
#[derive(Debug, Clone, Copy)]
pub struct ClippedLeaf<'a> {
    /// Номер элемента верхнего уровня списка, внутри которого лежит лист (для сообщений об ошибках).
    pub top_level_index: usize,
    pub item: &'a DrawItem,
    pub clip: EffectiveClip,
}

/// Обход листьев в глубину в порядке рисования. Группы обрезки не выдаются, их прямоугольники
/// накапливаются в [`EffectiveClip`] листа.
pub fn clipped_leaves(items: &[DrawItem]) -> ClippedLeaves<'_> {
    ClippedLeaves {
        top_level: items.iter().enumerate(),
        nested: Vec::new(),
    }
}

/// Итератор [`clipped_leaves`].
pub struct ClippedLeaves<'a> {
    top_level: std::iter::Enumerate<std::slice::Iter<'a, DrawItem>>,
    /// Раскрытые группы: самая внутренняя — последняя.
    nested: Vec<NestedLevel<'a>>,
}

/// Раскрытая группа обхода.
struct NestedLevel<'a> {
    top_level_index: usize,
    remaining: std::slice::Iter<'a, DrawItem>,
    clip: EffectiveClip,
}

impl<'a> Iterator for ClippedLeaves<'a> {
    type Item = ClippedLeaf<'a>;

    fn next(&mut self) -> Option<ClippedLeaf<'a>> {
        loop {
            // Берём следующий элемент самой внутренней раскрытой группы, иначе — верхнего уровня.
            let (top_level_index, item, clip) = match self.nested.last_mut() {
                Some(level) => match level.remaining.next() {
                    Some(item) => (level.top_level_index, item, level.clip),
                    None => {
                        self.nested.pop();
                        continue;
                    }
                },
                None => {
                    let (index, item) = self.top_level.next()?;
                    (index, item, EffectiveClip::WholeTarget)
                }
            };
            match item {
                DrawItem::Clipped(group) => self.nested.push(NestedLevel {
                    top_level_index,
                    remaining: group.items.iter(),
                    clip: clip.narrowed_by(group.rect),
                }),
                leaf => {
                    return Some(ClippedLeaf {
                        top_level_index,
                        item: leaf,
                        clip,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "тесты")]

    use super::*;
    use crate::color::{DisplayColor, Opacity};
    use crate::draw_list::items::ShapeItem;
    use crate::draw_list::{DrawList, DrawListError, GlassGroupItem, GlassMember, PressState};
    use crate::glass::{GlassStyle, ShadowParamsPx};
    use crate::light::LightDirection;
    use crate::shape::ShapeGeometry;
    use crate::transform::Affine2;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> PxRect {
        PxRect::new(PxPoint::new(x, y), PxSize::new(width, height))
    }

    /// Круг диаметром `diameter` с центром в `(x, y)`: границы = ровно диаметр (запаса нет).
    fn circle(x: f32, y: f32, diameter: f32) -> DrawItem {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle { diameter },
            transform: Affine2::translate(x, y),
            fill: DisplayColor::WHITE,
            stroke: None,
            shadow: None,
            opacity: Opacity::OPAQUE,
        })
    }

    fn clipped(rect: PxRect, items: Vec<DrawItem>) -> DrawItem {
        DrawItem::Clipped(ClipGroup { rect, items })
    }

    fn list(items: Vec<DrawItem>) -> DrawList {
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items = items;
        list
    }

    #[test]
    fn pixel_rect_agrees_with_the_per_pixel_rule_for_fractional_edges_and_ties() {
        // Края: целые, дробные, ровно на .5 (ничья: центр пикселя ровно на границе), отрицательные.
        let edges = [-1.5, -0.5, 0.0, 0.3, 0.5, 0.7, 1.0, 2.5, 3.2, 4.5, 6.0];
        for &min_x in &edges {
            for &max_x in &edges {
                let clip = rect(min_x, 0.25, max_x - min_x, 3.5);
                let pixels = clip_pixel_rect(&clip);
                for pixel_x in -4..10 {
                    for pixel_y in -2..6 {
                        let center = PxPoint::new(pixel_x as f32 + 0.5, pixel_y as f32 + 0.5);
                        let in_integer_rect = center.x >= pixels.origin.x
                            && center.x < pixels.max_x()
                            && center.y >= pixels.origin.y
                            && center.y < pixels.max_y();
                        assert_eq!(
                            in_integer_rect,
                            // Для max < min прямоугольник схлопнут: пикселей нет.
                            max_x > min_x && clip_keeps_pixel(&clip, center),
                            "ребро [{min_x}, {max_x}), пиксель ({pixel_x}, {pixel_y})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ties_at_half_pixel_edges_follow_the_half_open_rule() {
        // min = 2,5: центр пикселя 2 ровно на границе -> остаётся; max = 4,5: центр пикселя 4 -> отсекается.
        let pixels = clip_pixel_rect(&rect(2.5, 0.0, 2.0, 1.0));
        assert_eq!((pixels.origin.x, pixels.max_x()), (2.0, 4.0));
    }

    #[test]
    fn collapsed_or_non_finite_clip_has_empty_pixel_rect() {
        for clip in [
            rect(3.0, 3.0, 0.0, 5.0),
            rect(f32::NAN, 0.0, 4.0, 4.0),
            rect(0.0, 0.0, -2.0, 4.0),
        ] {
            let pixels = clip_pixel_rect(&clip);
            assert!(
                pixels.size.width == 0.0 || pixels.size.height == 0.0,
                "{clip:?} -> {pixels:?}"
            );
        }
    }

    #[test]
    fn nested_groups_intersect_and_pixel_rule_commutes_with_intersection() {
        let outer = rect(0.0, 0.0, 10.3, 10.0);
        let inner = rect(4.4, 2.0, 10.0, 4.0);
        let list = list(vec![clipped(outer, vec![clipped(inner, vec![circle(5.0, 5.0, 4.0)])])]);
        let leaf = clipped_leaves(&list.items).next().unwrap();
        let EffectiveClip::Rect(effective) = leaf.clip else {
            panic!("ожидали обрезку");
        };
        assert_eq!(effective.origin, PxPoint::new(4.4, 2.0));
        assert_eq!((effective.max_x(), effective.max_y()), (10.3, 6.0));
        // Сворачивание в один пиксельный прямоугольник = пересечение пиксельных прямоугольников групп.
        let intersected = clip_pixel_rect(&outer).intersection(&clip_pixel_rect(&inner)).unwrap();
        assert_eq!(leaf.clip.pixel_rect(), Some(intersected));
    }

    #[test]
    fn disjoint_nested_groups_keep_nothing() {
        let list = list(vec![clipped(
            rect(0.0, 0.0, 5.0, 5.0),
            vec![clipped(rect(10.0, 10.0, 5.0, 5.0), vec![circle(12.0, 12.0, 4.0)])],
        )]);
        let leaf = clipped_leaves(&list.items).next().unwrap();
        assert!(leaf.clip.keeps_nothing());
        assert_eq!(leaf.clip.visible_part(rect(0.0, 0.0, 100.0, 100.0)), None);
    }

    #[test]
    fn draw_rect_and_bounds_are_clipped_to_the_group_pixels() {
        // Круг диаметром 20 в (10, 10): границы [0, 20]; обрезка [0, 10) × [0, 30).
        let full = circle(10.0, 10.0, 20.0).draw_rect_px().unwrap();
        assert_eq!(full, rect(0.0, 0.0, 20.0, 20.0), "квад круга без обрезки");
        let item = clipped(rect(0.0, 0.0, 10.0, 30.0), vec![circle(10.0, 10.0, 20.0)]);
        assert_eq!(item.draw_rect_px(), Some(rect(0.0, 0.0, 10.0, 20.0)));
        assert_eq!(item.bounds_px(), Some(rect(0.0, 0.0, 10.0, 20.0)));
    }

    #[test]
    fn group_with_nothing_visible_has_no_rects() {
        let outside = clipped(rect(100.0, 100.0, 10.0, 10.0), vec![circle(10.0, 10.0, 4.0)]);
        assert_eq!((outside.draw_rect_px(), outside.bounds_px()), (None, None));
        let collapsed = clipped(rect(0.0, 0.0, 0.0, 50.0), vec![circle(10.0, 10.0, 4.0)]);
        assert_eq!((collapsed.draw_rect_px(), collapsed.bounds_px()), (None, None));
        let empty = clipped(rect(0.0, 0.0, 50.0, 50.0), Vec::new());
        assert_eq!((empty.draw_rect_px(), empty.bounds_px()), (None, None));
    }

    fn glass_group(center_x: f32) -> GlassGroupItem {
        let size = PxSize::new(40.0, 20.0);
        GlassGroupItem {
            params: GlassStyle::default().resolve(size, 2.0),
            members: vec![GlassMember {
                geometry: ShapeGeometry::Capsule { size },
                transform: Affine2::translate(center_x, 20.0),
                press: PressState::RELEASED,
            }],
            merge_radius_px: 0.0,
            effect: 1.0,
            opacity: Opacity::OPAQUE,
        }
    }

    #[test]
    fn glass_keeps_full_bounds_while_other_children_are_clipped() {
        let glass = glass_group(20.0);
        let full_glass_bounds = glass.bounds_px().unwrap();
        let narrow_clip = rect(18.0, 18.0, 4.0, 4.0);
        let group = clipped(narrow_clip, vec![DrawItem::Glass(glass.clone())]);
        assert_eq!(
            group.bounds_px(),
            Some(full_glass_bounds),
            "стекло читает фон и вне обрезки"
        );
        let draw = group.draw_rect_px().unwrap();
        assert_eq!(draw, glass.draw_rect_px().unwrap().intersection(&narrow_clip).unwrap());
        // Фигура в той же группе обрезается.
        let mixed = clipped(narrow_clip, vec![circle(20.0, 20.0, 40.0)]);
        assert_eq!(mixed.bounds_px(), Some(narrow_clip));
    }

    #[test]
    fn validate_rejects_non_finite_and_negative_clip_rects_with_the_top_level_index() {
        let bad_rects = [
            rect(f32::NAN, 0.0, 4.0, 4.0),
            rect(0.0, 0.0, f32::INFINITY, 4.0),
            rect(0.0, 0.0, -1.0, 4.0),
            rect(0.0, 0.0, 4.0, -0.5),
        ];
        for bad in bad_rects {
            // Плохая группа вложена на втором уровне внутри второго элемента списка.
            let list = list(vec![
                circle(5.0, 5.0, 4.0),
                clipped(
                    rect(0.0, 0.0, 10.0, 10.0),
                    vec![clipped(bad, vec![circle(5.0, 5.0, 4.0)])],
                ),
            ]);
            assert!(
                matches!(list.validate(), Err(DrawListError::InvalidClipRect { item: 1, .. })),
                "{bad:?}: {:?}",
                list.validate()
            );
        }
        // Нулевой размер допустим: пустая обрезка.
        let collapsed = list(vec![clipped(rect(1.0, 1.0, 0.0, 0.0), vec![circle(5.0, 5.0, 4.0)])]);
        assert_eq!(collapsed.validate(), Ok(()));
    }

    #[test]
    fn validate_checks_children_recursively() {
        let mut broken_shadow = circle(5.0, 5.0, 4.0);
        if let DrawItem::Shape(shape) = &mut broken_shadow {
            shape.shadow = Some(ShadowParamsPx {
                radius_px: -1.0,
                offset_px: 0.0,
                alpha: 0.5,
            });
        }
        let list = list(vec![
            circle(1.0, 1.0, 2.0),
            clipped(rect(0.0, 0.0, 10.0, 10.0), vec![broken_shadow]),
        ]);
        assert!(matches!(
            list.validate(),
            Err(DrawListError::InvalidSize {
                item: 1,
                field: "shadow.radius_px",
                ..
            })
        ));
    }

    #[test]
    fn leaf_items_are_depth_first_in_paint_order_without_groups() {
        let list = list(vec![
            circle(1.0, 0.0, 2.0),
            clipped(
                rect(0.0, 0.0, 10.0, 10.0),
                vec![
                    circle(2.0, 0.0, 2.0),
                    clipped(rect(0.0, 0.0, 5.0, 5.0), vec![circle(3.0, 0.0, 2.0)]),
                    circle(4.0, 0.0, 2.0),
                ],
            ),
            circle(5.0, 0.0, 2.0),
        ]);
        let xs: Vec<f32> = list
            .leaf_items()
            .map(|item| match item {
                DrawItem::Shape(shape) => shape.transform.tx,
                other => panic!("ожидали фигуру: {other:?}"),
            })
            .collect();
        assert_eq!(xs, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        let indices: Vec<usize> = clipped_leaves(&list.items).map(|leaf| leaf.top_level_index).collect();
        assert_eq!(
            indices,
            vec![0, 1, 1, 1, 2],
            "индекс верхнего уровня наследуется вложенными"
        );
        // Первый и последний лист вне групп: обрезка не протекает наружу.
        let clips: Vec<EffectiveClip> = clipped_leaves(&list.items).map(|leaf| leaf.clip).collect();
        assert_eq!(clips[0], EffectiveClip::WholeTarget);
        assert_eq!(clips[4], EffectiveClip::WholeTarget);
        assert_eq!(clips[3], EffectiveClip::Rect(rect(0.0, 0.0, 10.0, 10.0)));
        assert_eq!(clips[2], EffectiveClip::Rect(rect(0.0, 0.0, 5.0, 5.0)));
    }
}
