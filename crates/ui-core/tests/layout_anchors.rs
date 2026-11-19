//! Вёрстка якорями: все варианты по обеим осям, безопасная зона, привязка к низу, ошибки.

// Вспомогательные функции тестов не помечены `#[test]`, но падение на `unwrap` в них — это провал теста.
#![allow(clippy::unwrap_used)]

use ui_core::geometry::{Insets, Rect, Size};
use ui_core::layout::{AnchorRule, AxisPlacement, LayoutError, Placement, ReferenceArea};
use ui_core::tree::UiTree;

const EPS: f64 = 1e-9;

fn assert_rect(actual: Rect, expected: Rect) {
    let pairs = [
        (actual.origin.x, expected.origin.x),
        (actual.origin.y, expected.origin.y),
        (actual.size.width, expected.size.width),
        (actual.size.height, expected.size.height),
    ];
    for (a, e) in pairs {
        assert!((a - e).abs() < EPS, "{actual:?} != {expected:?}");
    }
}

fn anchored(horizontal: AxisPlacement, vertical: AxisPlacement) -> Placement {
    Placement::Anchored(AnchorRule {
        area: ReferenceArea::Frame,
        horizontal,
        vertical,
    })
}

/// Экран 440 x height с безопасной зоной iPhone (верх 62, низ 34).
fn screen(height: f64) -> UiTree {
    UiTree::new(Size::new(440.0, height), Insets::new(62.0, 0.0, 34.0, 0.0)).unwrap()
}

#[test]
fn every_horizontal_variant_on_frame() {
    let mut tree = screen(900.0);
    let root = tree.root();
    let vertical = AxisPlacement::Start {
        inset: 10.0,
        length: 20.0,
    };
    let cases = [
        (
            AxisPlacement::Start {
                inset: 15.0,
                length: 50.0,
            },
            15.0,
            50.0,
        ),
        (
            AxisPlacement::End {
                inset: 15.0,
                length: 50.0,
            },
            440.0 - 15.0 - 50.0,
            50.0,
        ),
        (
            AxisPlacement::Center {
                offset: 0.0,
                length: 50.0,
            },
            195.0,
            50.0,
        ),
        (
            AxisPlacement::Center {
                offset: -30.0,
                length: 50.0,
            },
            165.0,
            50.0,
        ),
        (
            AxisPlacement::Stretch {
                start_inset: 16.0,
                end_inset: 24.0,
            },
            16.0,
            400.0,
        ),
    ];
    for (horizontal, expected_x, expected_width) in cases {
        let node = tree.insert_child(root, "h").unwrap();
        tree.set_placement(node, anchored(horizontal, vertical)).unwrap();
        tree.layout().unwrap();
        assert_rect(
            tree.frame(node).unwrap(),
            Rect::from_xywh(expected_x, 10.0, expected_width, 20.0),
        );
    }
}

#[test]
fn every_vertical_variant_on_frame() {
    let mut tree = screen(900.0);
    let root = tree.root();
    let horizontal = AxisPlacement::Start {
        inset: 0.0,
        length: 30.0,
    };
    let cases = [
        (
            AxisPlacement::Start {
                inset: 7.0,
                length: 40.0,
            },
            7.0,
            40.0,
        ),
        (
            AxisPlacement::End {
                inset: 7.0,
                length: 40.0,
            },
            900.0 - 7.0 - 40.0,
            40.0,
        ),
        (
            AxisPlacement::Center {
                offset: 12.0,
                length: 40.0,
            },
            430.0 + 12.0,
            40.0,
        ),
        (
            AxisPlacement::Stretch {
                start_inset: 100.0,
                end_inset: 50.0,
            },
            100.0,
            750.0,
        ),
    ];
    for (vertical, expected_y, expected_height) in cases {
        let node = tree.insert_child(root, "v").unwrap();
        tree.set_placement(node, anchored(horizontal, vertical)).unwrap();
        tree.layout().unwrap();
        assert_rect(
            tree.frame(node).unwrap(),
            Rect::from_xywh(0.0, expected_y, 30.0, expected_height),
        );
    }
}

#[test]
fn safe_area_reference_differs_from_frame_and_root_uses_screen_safe_area() {
    let mut tree = screen(900.0);
    let root = tree.root();
    let in_safe = tree.insert_child(root, "safe").unwrap();
    let in_frame = tree.insert_child(root, "frame").unwrap();
    let edge = |area| {
        Placement::Anchored(AnchorRule {
            area,
            horizontal: AxisPlacement::Stretch {
                start_inset: 0.0,
                end_inset: 0.0,
            },
            vertical: AxisPlacement::Stretch {
                start_inset: 0.0,
                end_inset: 0.0,
            },
        })
    };
    tree.set_placement(in_safe, edge(ReferenceArea::SafeArea)).unwrap();
    tree.set_placement(in_frame, edge(ReferenceArea::Frame)).unwrap();
    tree.layout().unwrap();
    assert_rect(
        tree.frame(in_safe).unwrap(),
        Rect::from_xywh(0.0, 62.0, 440.0, 900.0 - 62.0 - 34.0),
    );
    assert_rect(tree.frame(in_frame).unwrap(), Rect::from_xywh(0.0, 0.0, 440.0, 900.0));
    // Отступ от низа безопасной зоны: нижний край на 34 pt выше низа экрана.
    tree.set_placement(
        in_safe,
        Placement::Anchored(AnchorRule {
            area: ReferenceArea::SafeArea,
            horizontal: AxisPlacement::Start {
                inset: 0.0,
                length: 10.0,
            },
            vertical: AxisPlacement::End {
                inset: 0.0,
                length: 10.0,
            },
        }),
    )
    .unwrap();
    tree.layout().unwrap();
    assert_rect(
        tree.frame(in_safe).unwrap(),
        Rect::from_xywh(0.0, 900.0 - 34.0 - 10.0, 10.0, 10.0),
    );
}

#[test]
fn nested_anchor_is_relative_to_parent_frame_and_safe_insets_of_parent() {
    let mut tree = screen(900.0);
    let root = tree.root();
    let panel = tree.insert_child(root, "panel").unwrap();
    tree.set_placement(
        panel,
        anchored(
            AxisPlacement::Start {
                inset: 100.0,
                length: 200.0,
            },
            AxisPlacement::Start {
                inset: 300.0,
                length: 200.0,
            },
        ),
    )
    .unwrap();
    tree.set_safe_insets(panel, Insets::uniform(20.0)).unwrap();
    let inner = tree.insert_child(panel, "inner").unwrap();
    tree.set_placement(
        inner,
        Placement::Anchored(AnchorRule {
            area: ReferenceArea::SafeArea,
            horizontal: AxisPlacement::Start {
                inset: 5.0,
                length: 10.0,
            },
            vertical: AxisPlacement::End {
                inset: 5.0,
                length: 10.0,
            },
        }),
    )
    .unwrap();
    tree.layout().unwrap();
    // Безопасная зона панели: x 120..280, y 320..480.
    assert_rect(tree.frame(inner).unwrap(), Rect::from_xywh(125.0, 465.0, 10.0, 10.0));
}

/// Нижние элементы экрана камеры (затвор, капсула режимов) и кадр видоискателя, привязанные к низу.
fn camera_like_layout(height: f64) -> (UiTree, [ui_core::id::NodeId; 3]) {
    let mut tree = screen(height);
    let root = tree.root();
    let shutter = tree.insert_child(root, "shutter").unwrap();
    // Центр затвора в iPhone: 801,9 из 956 -> 154,1 pt от низа до центра, Ø 80,1.
    tree.set_placement(
        shutter,
        anchored(
            AxisPlacement::Center {
                offset: 0.0,
                length: 80.1,
            },
            AxisPlacement::End {
                inset: 956.0 - 801.9 - 40.05,
                length: 80.1,
            },
        ),
    )
    .unwrap();
    let capsule = tree.insert_child(root, "capsule").unwrap();
    tree.set_placement(
        capsule,
        anchored(
            AxisPlacement::Center {
                offset: 0.0,
                length: 220.0,
            },
            AxisPlacement::End {
                inset: 34.1,
                length: 47.8,
            },
        ),
    )
    .unwrap();
    let viewfinder = tree.insert_child(root, "viewfinder").unwrap();
    // Кадр 16:9: от 75,7 до 858,2 при высоте 956 -> низ на 97,8 pt выше низа экрана.
    tree.set_placement(
        viewfinder,
        anchored(
            AxisPlacement::Stretch {
                start_inset: 0.0,
                end_inset: 0.0,
            },
            AxisPlacement::End {
                inset: 97.8,
                length: 782.5,
            },
        ),
    )
    .unwrap();
    tree.layout().unwrap();
    (tree, [shutter, capsule, viewfinder])
}

#[test]
fn bottom_anchored_layout_keeps_distance_to_bottom_and_extra_height_goes_up() {
    // Наш телефон: 1080 x 2412 px при 1 pt = 1080/440 px -> высота в pt.
    let our_height = 2412.0 / (1080.0 / 440.0);
    assert!((our_height - 982.666_666_f64).abs() < 1e-3);
    let (reference, ids_reference) = camera_like_layout(956.0);
    let (ours, ids_ours) = camera_like_layout(our_height);
    let extra = our_height - 956.0;
    assert!((extra - 26.666_666_f64).abs() < 1e-3);
    for (id_reference, id_ours) in ids_reference.into_iter().zip(ids_ours) {
        let frame_reference = reference.frame(id_reference).unwrap();
        let frame_ours = ours.frame(id_ours).unwrap();
        // Расстояние до низа экрана одинаково, размеры не изменились.
        let bottom_gap_reference = 956.0 - frame_reference.max_y();
        let bottom_gap_ours = our_height - frame_ours.max_y();
        assert!((bottom_gap_reference - bottom_gap_ours).abs() < EPS);
        assert_eq!(frame_reference.size, frame_ours.size);
        // Весь элемент сместился вниз ровно на лишние pt (то есть «на месте относительно низа»).
        assert!((frame_ours.min_y() - frame_reference.min_y() - extra).abs() < EPS);
    }
    // Лишнее ушло в полосу сверху: она выросла с 75,7 до 75,7 + 26,67 = 102,4 (layout.md 4.3).
    let top_strip_reference = reference.frame(ids_reference[2]).unwrap().min_y();
    let top_strip_ours = ours.frame(ids_ours[2]).unwrap().min_y();
    assert!((top_strip_reference - 75.7).abs() < 1e-9);
    assert!((top_strip_ours - 102.366_666_f64).abs() < 1e-3);
}

#[test]
fn layout_is_bitwise_deterministic() {
    let (first, ids_first) = camera_like_layout(982.666_666_666_666_7);
    let (second, ids_second) = camera_like_layout(982.666_666_666_666_7);
    for (a, b) in ids_first.into_iter().zip(ids_second) {
        let (fa, fb) = (first.frame(a).unwrap(), second.frame(b).unwrap());
        assert_eq!(fa.origin.x.to_bits(), fb.origin.x.to_bits());
        assert_eq!(fa.origin.y.to_bits(), fb.origin.y.to_bits());
        assert_eq!(fa.size.width.to_bits(), fb.size.width.to_bits());
        assert_eq!(fa.size.height.to_bits(), fb.size.height.to_bits());
    }
    // Повторный layout() без изменений ничего не меняет.
    let mut again = first.clone();
    again.layout().unwrap();
    for id in ids_first {
        assert_eq!(first.frame(id).unwrap(), again.frame(id).unwrap());
    }
}

#[test]
fn resize_screen_moves_bottom_anchored_elements_after_relayout() {
    let (mut tree, ids) = camera_like_layout(956.0);
    let shutter_before = tree.frame(ids[0]).unwrap();
    tree.resize_screen(Size::new(440.0, 1000.0), Insets::ZERO).unwrap();
    // До вёрстки рамки устарели и не тронуты.
    assert_eq!(tree.frame(ids[0]).unwrap(), shutter_before);
    tree.layout().unwrap();
    assert_rect(
        tree.frame(ids[0]).unwrap(),
        Rect::from_xywh(shutter_before.origin.x, shutter_before.origin.y + 44.0, 80.1, 80.1),
    );
}

#[test]
fn invalid_configuration_is_explicit_error_and_keeps_old_frames() {
    let mut tree = screen(900.0);
    let root = tree.root();
    let node = tree.insert_child(root, "node").unwrap();
    tree.set_placement(
        node,
        anchored(
            AxisPlacement::Start {
                inset: 10.0,
                length: 50.0,
            },
            AxisPlacement::Start {
                inset: 10.0,
                length: 50.0,
            },
        ),
    )
    .unwrap();
    tree.layout().unwrap();
    let good_frame = tree.frame(node).unwrap();
    let other = tree.insert_child(root, "other").unwrap();

    let bad_cases = [
        anchored(
            AxisPlacement::Start {
                inset: 0.0,
                length: -1.0,
            },
            AxisPlacement::Start {
                inset: 0.0,
                length: 1.0,
            },
        ),
        anchored(
            AxisPlacement::Start {
                inset: 0.0,
                length: f64::NAN,
            },
            AxisPlacement::Start {
                inset: 0.0,
                length: 1.0,
            },
        ),
        anchored(
            AxisPlacement::Start {
                inset: f64::INFINITY,
                length: 1.0,
            },
            AxisPlacement::Start {
                inset: 0.0,
                length: 1.0,
            },
        ),
        anchored(
            AxisPlacement::Stretch {
                start_inset: 300.0,
                end_inset: 200.0,
            },
            AxisPlacement::Start {
                inset: 0.0,
                length: 1.0,
            },
        ),
    ];
    let expect: [fn(&LayoutError) -> bool; 4] = [
        |e| matches!(e, LayoutError::InvalidLength { .. }),
        |e| matches!(e, LayoutError::InvalidLength { .. }),
        |e| matches!(e, LayoutError::InvalidOffset { .. }),
        |e| matches!(e, LayoutError::NegativeStretch { resulting_length, .. } if (*resulting_length + 60.0).abs() < EPS),
    ];
    for (bad, check) in bad_cases.into_iter().zip(expect) {
        tree.set_placement(other, bad).unwrap();
        let error = tree.layout().unwrap_err();
        assert!(check(&error), "{error:?}");
        // Атомарность: уже рассчитанная рамка соседа не изменилась.
        assert_eq!(tree.frame(node).unwrap(), good_frame);
    }
    // Нулевое растяжение — допустимо (длина 0).
    tree.set_placement(
        other,
        anchored(
            AxisPlacement::Stretch {
                start_inset: 200.0,
                end_inset: 240.0,
            },
            AxisPlacement::Start {
                inset: 0.0,
                length: 1.0,
            },
        ),
    )
    .unwrap();
    tree.layout().unwrap();
    assert_eq!(tree.frame(other).unwrap().size.width, 0.0);
}

#[test]
fn root_has_no_placement() {
    let mut tree = screen(900.0);
    let root = tree.root();
    assert!(tree.set_placement(root, Placement::default()).is_err());
    tree.layout().unwrap();
    assert_rect(tree.frame(root).unwrap(), Rect::from_xywh(0.0, 0.0, 440.0, 900.0));
}
