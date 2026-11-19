//! Стеки: промежутки, выравнивание, расположение по главной оси, вложенность, ошибки.

// Вспомогательные функции тестов не помечены `#[test]`, но падение на `unwrap` в них — это провал теста.
#![allow(clippy::unwrap_used)]

use ui_core::geometry::{Insets, Rect, Size};
use ui_core::id::NodeId;
use ui_core::layout::{
    AnchorRule, AxisPlacement, ContainerLayout, CrossAlignment, LayoutError, MainDistribution, Placement,
    ReferenceArea, StackAxis, StackSpec,
};
use ui_core::tree::{UiTree, Visibility};

/// Контейнер-стек 300x100 в точке (20, 40) экрана 440x900 + дети с заданными размерами.
fn stack_with(spec: StackSpec, child_sizes: &[Size]) -> (UiTree, NodeId, Vec<NodeId>) {
    let mut tree = UiTree::new(Size::new(440.0, 900.0), Insets::ZERO).unwrap();
    let root = tree.root();
    let stack = tree.insert_child(root, "stack").unwrap();
    tree.set_placement(
        stack,
        Placement::Anchored(AnchorRule {
            area: ReferenceArea::Frame,
            horizontal: AxisPlacement::Start {
                inset: 20.0,
                length: 300.0,
            },
            vertical: AxisPlacement::Start {
                inset: 40.0,
                length: 100.0,
            },
        }),
    )
    .unwrap();
    tree.set_container_layout(stack, ContainerLayout::Stack(spec)).unwrap();
    let children = child_sizes
        .iter()
        .map(|&size| {
            let child = tree.insert_child(stack, "item").unwrap();
            tree.set_placement(child, Placement::InStack(size)).unwrap();
            child
        })
        .collect();
    tree.layout().unwrap();
    (tree, stack, children)
}

fn horizontal(spacing: f64, cross: CrossAlignment, distribution: MainDistribution) -> StackSpec {
    StackSpec {
        axis: StackAxis::Horizontal,
        spacing,
        cross_alignment: cross,
        distribution,
        area: ReferenceArea::Frame,
    }
}

const SIZES: [Size; 3] = [Size::new(40.0, 20.0), Size::new(50.0, 40.0), Size::new(30.0, 30.0)];

#[test]
fn horizontal_start_with_spacing_and_cross_alignments() {
    let cases = [
        (CrossAlignment::Start, [40.0, 40.0, 40.0]),
        (CrossAlignment::Center, [40.0 + 40.0, 40.0 + 30.0, 40.0 + 35.0]),
        (CrossAlignment::End, [40.0 + 80.0, 40.0 + 60.0, 40.0 + 70.0]),
    ];
    for (cross, expected_y) in cases {
        let (tree, _, items) = stack_with(horizontal(10.0, cross, MainDistribution::Start), &SIZES);
        let expected_x = [20.0, 20.0 + 40.0 + 10.0, 20.0 + 40.0 + 10.0 + 50.0 + 10.0];
        for (index, &item) in items.iter().enumerate() {
            let expected = Rect::from_xywh(
                expected_x[index],
                expected_y[index],
                SIZES[index].width,
                SIZES[index].height,
            );
            assert_eq!(tree.frame(item).unwrap(), expected, "{cross:?} #{index}");
        }
    }
}

#[test]
fn main_axis_distribution_center_and_end() {
    // Занято 40+50+30 + 2*10 = 140 из 300 -> свободно 160.
    let (tree, _, items) = stack_with(
        horizontal(10.0, CrossAlignment::Start, MainDistribution::Center),
        &SIZES,
    );
    assert_eq!(tree.frame(items[0]).unwrap().min_x(), 20.0 + 80.0);
    assert_eq!(tree.frame(items[2]).unwrap().max_x(), 20.0 + 80.0 + 140.0);
    let (tree, _, items) = stack_with(horizontal(10.0, CrossAlignment::Start, MainDistribution::End), &SIZES);
    assert_eq!(tree.frame(items[0]).unwrap().min_x(), 20.0 + 160.0);
    assert_eq!(
        tree.frame(items[2]).unwrap().max_x(),
        320.0,
        "последний упирается в правый край стека"
    );
}

#[test]
fn vertical_stack_uses_height_as_main_axis() {
    let spec = StackSpec {
        axis: StackAxis::Vertical,
        spacing: 5.0,
        cross_alignment: CrossAlignment::End,
        distribution: MainDistribution::Start,
        area: ReferenceArea::Frame,
    };
    let (tree, _, items) = stack_with(spec, &SIZES);
    // Правый край стека x = 320; ось y от 40: 20 + 5 + 40 + 5 + 30.
    assert_eq!(tree.frame(items[0]).unwrap(), Rect::from_xywh(280.0, 40.0, 40.0, 20.0));
    assert_eq!(tree.frame(items[1]).unwrap(), Rect::from_xywh(270.0, 65.0, 50.0, 40.0));
    assert_eq!(tree.frame(items[2]).unwrap(), Rect::from_xywh(290.0, 110.0, 30.0, 30.0));
}

#[test]
fn empty_stack_and_single_child_have_no_gaps() {
    let (tree, stack, items) = stack_with(horizontal(10.0, CrossAlignment::Start, MainDistribution::Center), &[]);
    assert!(items.is_empty());
    assert_eq!(tree.frame(stack).unwrap(), Rect::from_xywh(20.0, 40.0, 300.0, 100.0));
    let (tree, _, items) = stack_with(
        horizontal(10.0, CrossAlignment::Start, MainDistribution::Center),
        &[Size::new(100.0, 10.0)],
    );
    assert_eq!(
        tree.frame(items[0]).unwrap().min_x(),
        20.0 + 100.0,
        "промежуток не добавляется к одному ребёнку"
    );
}

#[test]
fn overflowing_content_is_not_clipped() {
    let wide = [Size::new(200.0, 10.0), Size::new(200.0, 10.0)];
    let (tree, _, items) = stack_with(horizontal(0.0, CrossAlignment::Start, MainDistribution::Center), &wide);
    // 400 в области 300: выходит поровну в обе стороны.
    assert_eq!(tree.frame(items[0]).unwrap().min_x(), 20.0 - 50.0);
    assert_eq!(tree.frame(items[1]).unwrap().max_x(), 320.0 + 50.0);
}

#[test]
fn nested_stack_inside_stack_follows_outer_position() {
    // Внешний вертикальный стек 300x100 с двумя детьми; второй — сам горизонтальный стек.
    let outer = StackSpec {
        axis: StackAxis::Vertical,
        spacing: 10.0,
        cross_alignment: CrossAlignment::Start,
        distribution: MainDistribution::Start,
        area: ReferenceArea::Frame,
    };
    let (mut tree, _, items) = stack_with(outer, &[Size::new(100.0, 20.0), Size::new(200.0, 30.0)]);
    let row = items[1];
    tree.set_container_layout(
        row,
        ContainerLayout::Stack(horizontal(4.0, CrossAlignment::Center, MainDistribution::End)),
    )
    .unwrap();
    let zoom_small = tree.insert_child(row, "zoom_small").unwrap();
    let zoom_big = tree.insert_child(row, "zoom_big").unwrap();
    tree.set_placement(zoom_small, Placement::InStack(Size::new(30.0, 10.0)))
        .unwrap();
    tree.set_placement(zoom_big, Placement::InStack(Size::new(40.0, 20.0)))
        .unwrap();
    tree.layout().unwrap();
    // Строка: x 20..220, y 70..100 (40 + 20 + 10 = 70). Конец главной оси: занято 30+4+40 = 74.
    assert_eq!(tree.frame(row).unwrap(), Rect::from_xywh(20.0, 70.0, 200.0, 30.0));
    assert_eq!(
        tree.frame(zoom_small).unwrap(),
        Rect::from_xywh(220.0 - 74.0, 70.0 + 10.0, 30.0, 10.0)
    );
    assert_eq!(
        tree.frame(zoom_big).unwrap(),
        Rect::from_xywh(220.0 - 40.0, 70.0 + 5.0, 40.0, 20.0)
    );
}

#[test]
fn stack_can_use_parent_safe_area() {
    let mut tree = UiTree::new(Size::new(440.0, 900.0), Insets::new(60.0, 10.0, 30.0, 10.0)).unwrap();
    let root = tree.root();
    tree.set_container_layout(
        root,
        ContainerLayout::Stack(StackSpec {
            axis: StackAxis::Vertical,
            spacing: 0.0,
            cross_alignment: CrossAlignment::Start,
            distribution: MainDistribution::End,
            area: ReferenceArea::SafeArea,
        }),
    )
    .unwrap();
    let item = tree.insert_child(root, "item").unwrap();
    tree.set_placement(item, Placement::InStack(Size::new(50.0, 50.0)))
        .unwrap();
    tree.layout().unwrap();
    assert_eq!(
        tree.frame(item).unwrap(),
        Rect::from_xywh(10.0, 900.0 - 30.0 - 50.0, 50.0, 50.0)
    );
}

#[test]
fn configuration_errors_are_explicit() {
    let spec = horizontal(10.0, CrossAlignment::Start, MainDistribution::Start);
    let (mut tree, stack, items) = stack_with(spec, &SIZES);
    let good = tree.frame(items[1]).unwrap();

    // Отрицательный размер ребёнка.
    tree.set_placement(items[0], Placement::InStack(Size::new(-1.0, 5.0)))
        .unwrap();
    assert!(matches!(tree.layout(), Err(LayoutError::InvalidLength { node, .. }) if node == items[0]));
    tree.set_placement(items[0], Placement::InStack(SIZES[0])).unwrap();

    // Отрицательный промежуток.
    let negative_spacing = horizontal(-1.0, CrossAlignment::Start, MainDistribution::Start);
    tree.set_container_layout(stack, ContainerLayout::Stack(negative_spacing))
        .unwrap();
    assert!(matches!(tree.layout(), Err(LayoutError::InvalidOffset { node, .. }) if node == stack));
    tree.set_container_layout(stack, ContainerLayout::Stack(spec)).unwrap();

    // Ребёнок с якорем в стеке и ребёнок со стековым размером в свободном контейнере.
    tree.set_placement(items[2], Placement::default()).unwrap();
    assert!(matches!(tree.layout(), Err(LayoutError::PlacementMismatch { node }) if node == items[2]));
    tree.set_placement(items[2], Placement::InStack(SIZES[2])).unwrap();
    tree.set_container_layout(stack, ContainerLayout::Free).unwrap();
    assert!(matches!(tree.layout(), Err(LayoutError::PlacementMismatch { .. })));

    // После починки вёрстка снова работает, а прежние рамки на ошибках не менялись.
    tree.set_container_layout(stack, ContainerLayout::Stack(spec)).unwrap();
    tree.layout().unwrap();
    assert_eq!(tree.frame(items[1]).unwrap(), good);
}

#[test]
fn hidden_child_keeps_its_slot_in_stack() {
    let (mut tree, _, items) = stack_with(horizontal(10.0, CrossAlignment::Start, MainDistribution::Start), &SIZES);
    let before = tree.frame(items[2]).unwrap();
    tree.set_visibility(items[1], Visibility::Hidden).unwrap();
    tree.layout().unwrap();
    assert_eq!(tree.frame(items[2]).unwrap(), before);
}
