//! Мост «элемент сцены `ui-core` → преобразование фигуры в пикселях экрана».
//!
//! `ui-core` отдаёт элемент в порядке рисования ([`ui_core::tree::NodeVisual`]): рамку после вёрстки
//! (pt, абсолютные координаты) и преобразование «точка вёрстки → точка экрана» (pt, `f64`) с учётом
//! визуального масштаба, поворота и сдвига самого элемента и всех предков. Список рисования
//! работает в пикселях и `f32`, а фигура задаётся в локальных координатах с центром в (0, 0).
//! Здесь одно место, где эти миры сшиваются, — чтобы рендер и экран (12) не пересчитывали
//! чужую математику каждый по-своему.
//!
//! # Ловушка порядка коэффициентов
//! У `ui_core::tree::NodeTransform` `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty` (столбцы матрицы),
//! а у [`Affine2`] `x' = a·x + b·y + tx`, `y' = c·x + d·y + ty` (строки). Поэтому `b` и `c`
//! при переводе меняются местами; тест `bridge_matches_ui_core_transform_point_by_point` ловит ошибку.

use ui_core::geometry::Point;
use ui_core::tree::NodeVisual;

use crate::geometry::{PxPoint, PxRect, PxSize};
use crate::transform::Affine2;

/// Элемент сцены, переведённый в пиксели: всё, что нужно строителю списка рисования.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodePlacementPx {
    /// Размер рамки элемента в пикселях (без визуального масштаба: масштаб — в `transform`).
    pub size: PxSize,
    /// Локальные координаты фигуры (центр рамки = (0, 0), px) → пиксели экрана.
    pub transform: Affine2,
    /// Прозрачность, перемноженная с предками (0..1).
    pub opacity: f32,
}

impl NodePlacementPx {
    /// Рамка элемента на экране, вдоль осей (px): прямоугольник обрезки содержимого ([`crate::draw_list::ClipGroup`]).
    /// Точен, пока элемент повёрнут на кратный 90° угол (так живут все прокручиваемые окна); при ином повороте это
    /// охватывающий прямоугольник. `None` — преобразование не конечно.
    pub fn screen_rect_px(&self) -> Option<PxRect> {
        let (half_width, half_height) = (self.size.width / 2.0, self.size.height / 2.0);
        let corners = [
            PxPoint::new(-half_width, -half_height),
            PxPoint::new(half_width, -half_height),
            PxPoint::new(half_width, half_height),
            PxPoint::new(-half_width, half_height),
        ]
        .map(|corner| self.transform.apply_point(corner));
        if corners
            .iter()
            .all(|corner| corner.x.is_finite() && corner.y.is_finite())
        {
            PxRect::bounding(corners)
        } else {
            None
        }
    }
}

/// Перевести элемент сцены в пиксели экрана при масштабе `px_per_pt`
/// (`ui_core::input::PointScale::pixels_per_point`).
///
/// Цепочка: локальная точка `q` (px, от центра) → `центр рамки + q / s` (pt вёрстки) →
/// `to_screen` (pt экрана) → `× s` (px экрана), где `s = px_per_pt`.
pub fn node_placement_px(visual: &NodeVisual, px_per_pt: f64) -> NodePlacementPx {
    let to_screen = &visual.to_screen;
    let center = visual.frame.center();
    // Сначала линейная часть `to_screen` в строковой записи Affine2 (перестановка b ↔ c, см. доку модуля).
    // Масштабы `1/s` (вход) и `s` (выход) взаимно сокращаются в линейной части: px → px.
    let linear_a = to_screen.a;
    let linear_b = to_screen.c;
    let linear_c = to_screen.b;
    let linear_d = to_screen.d;
    // Сдвиг: образ центра рамки, переведённый в px.
    let screen_center = to_screen.apply(Point::new(center.x, center.y));
    let transform = Affine2 {
        a: linear_a as f32,
        b: linear_b as f32,
        c: linear_c as f32,
        d: linear_d as f32,
        tx: (screen_center.x * px_per_pt) as f32,
        ty: (screen_center.y * px_per_pt) as f32,
    };
    NodePlacementPx {
        size: PxSize::new(
            (visual.frame.size.width * px_per_pt) as f32,
            (visual.frame.size.height * px_per_pt) as f32,
        ),
        transform,
        opacity: visual.opacity.clamp(0.0, 1.0) as f32,
    }
}

/// Точка экрана в pt → px (для касаний, переданных в список рисования, например точки нажатия).
pub fn screen_point_px(point: Point, px_per_pt: f64) -> PxPoint {
    PxPoint::new((point.x * px_per_pt) as f32, (point.y * px_per_pt) as f32)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

    use ui_core::geometry::{Insets, Rect, Size};
    use ui_core::tree::{NodeTransform, UiTree};

    use super::*;

    const SCALE: f64 = 2.5;

    /// Поворот на 90° по часовой (y вниз) с масштабом 2 по x и сдвигом — несимметричная матрица,
    /// на которой перестановка b ↔ c обязательно видна.
    fn skewed_visual() -> NodeVisual {
        // Настоящий id из дерева: у NodeId нет публичного конструктора (поколения — забота дерева).
        let tree = UiTree::new(Size::new(440.0, 956.0), Insets::default()).unwrap();
        NodeVisual {
            node: tree.root(),
            label: "test",
            frame: Rect::new(Point::new(100.0, 40.0), Size::new(60.0, 20.0)),
            to_screen: NodeTransform {
                a: 0.0,
                b: 2.0,
                c: -1.0,
                d: 0.0,
                tx: 150.0,
                ty: -120.0,
            },
            opacity: 0.4,
            depth: 1,
        }
    }

    #[test]
    fn bridge_matches_ui_core_transform_point_by_point() {
        let visual = skewed_visual();
        let placement = node_placement_px(&visual, SCALE);
        let center = visual.frame.center();
        for (dx_pt, dy_pt) in [(0.0, 0.0), (30.0, 0.0), (0.0, 10.0), (-30.0, 10.0), (12.5, -7.0)] {
            // Ожидание — через сам ui-core: точка вёрстки → экран (pt) → px.
            let expected_pt = visual.to_screen.apply(Point::new(center.x + dx_pt, center.y + dy_pt));
            let local_px = PxPoint::new((dx_pt * SCALE) as f32, (dy_pt * SCALE) as f32);
            let actual = placement.transform.apply_point(local_px);
            assert!(
                (f64::from(actual.x) - expected_pt.x * SCALE).abs() < 1e-3,
                "x для ({dx_pt}, {dy_pt})"
            );
            assert!(
                (f64::from(actual.y) - expected_pt.y * SCALE).abs() < 1e-3,
                "y для ({dx_pt}, {dy_pt})"
            );
        }
    }

    #[test]
    fn size_is_frame_in_pixels_and_opacity_is_kept() {
        let placement = node_placement_px(&skewed_visual(), SCALE);
        assert_eq!(placement.size, PxSize::new(150.0, 50.0));
        assert!((placement.opacity - 0.4).abs() < 1e-6);
    }

    #[test]
    fn the_screen_rect_of_a_turned_frame_swaps_its_sides_and_keeps_its_center() {
        let quarter = std::f32::consts::FRAC_PI_2;
        for (turns, size) in [
            (0, (40.0, 10.0)),
            (1, (10.0, 40.0)),
            (2, (40.0, 10.0)),
            (3, (10.0, 40.0)),
        ] {
            let placement = NodePlacementPx {
                size: PxSize::new(40.0, 10.0),
                transform: Affine2::rotate(quarter * turns as f32).then(Affine2::translate(100.0, 200.0)),
                opacity: 1.0,
            };
            let rect = placement.screen_rect_px().expect("конечное преобразование");
            let close = |actual: f32, expected: f32| (actual - expected).abs() < 1e-3;
            assert!(
                close(rect.size.width, size.0) && close(rect.size.height, size.1),
                "{turns}: {rect:?}"
            );
            assert!(close(rect.origin.x, 100.0 - size.0 / 2.0) && close(rect.origin.y, 200.0 - size.1 / 2.0));
        }
        let broken = NodePlacementPx {
            size: PxSize::new(40.0, 10.0),
            transform: Affine2::translate(f32::NAN, 0.0),
            opacity: 1.0,
        };
        assert_eq!(broken.screen_rect_px(), None);
    }
}
