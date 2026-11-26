//! Фильтр абляций на настоящих списках рисования экрана (без GPU): ничего лишнего не выкидывается, порядок
//! и валидность сохраняются, виды элементов делятся без пропусков и пересечений.

use camera_contract::lens_shading::LensShadingCalibrations;
use render_loop::config::RenderLoopConfig;
use ui_draw::draw_list::DrawItem;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::look_service::LookHandle;
use crate::look_startup::catalog_environment;
use crate::saturation_bench::catalog::DisplayScreen;
use crate::saturation_bench::inputs::{InputSources, gather_inputs};
use crate::saturation_bench::screen_scene::{ScreenPlan, SurfaceSizePx, summarize};

/// Настоящий список экрана `display` на ONEPLUS_11.
fn screen_list(display: DisplayScreen) -> DrawList {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let none = LensShadingCalibrations::none();
    let look = LookHandle::in_memory(
        &look_presets::PresetStoreConfig::default(),
        &catalog_environment(&render),
    )
    .expect("библиотека");
    let inputs = gather_inputs(&InputSources {
        engine_status: None,
        lens_shading: &none,
        render_config: &render,
        look: Some(&look),
        screen: &screen,
    });
    ScreenPlan::build(&inputs, SurfaceSizePx::ONEPLUS_11)
        .expect("экраны")
        .scene(display)
        .draw_list
        .clone()
}

fn calibrate_list() -> DrawList {
    screen_list(DisplayScreen::Calibrate)
}

/// Список PHOTO: единственный экран со стеклом (раздел калибровки с 14m рисует поверхности без стекла), поэтому на
/// нём проверяются абляции стекла.
fn photo_list() -> DrawList {
    screen_list(DisplayScreen::Photo)
}

const SINGLE_KINDS: [DrawFilter; 4] = [
    DrawFilter::GlassOnly,
    DrawFilter::ShapesOnly,
    DrawFilter::TextOnly,
    DrawFilter::ImagesOnly,
];

#[test]
fn all_keeps_the_list_unchanged() {
    let list = calibrate_list();
    assert_eq!(DrawFilter::All.apply(&list), list);
}

#[test]
fn every_item_belongs_to_exactly_one_single_kind_filter_and_the_counts_add_up() {
    let list = calibrate_list();
    assert!(
        list.leaf_items().count() > 10,
        "сцена настоящая, а не пустая: {}",
        list.leaf_items().count()
    );
    for item in list.leaf_items() {
        let owners = SINGLE_KINDS.iter().filter(|filter| filter.keeps(item)).count();
        assert_eq!(owners, 1, "элемент {item:?} должен принадлежать ровно одному виду");
    }
    let total: usize = SINGLE_KINDS
        .iter()
        .map(|filter| filter.apply(&list).leaf_items().count())
        .sum();
    assert_eq!(total, list.leaf_items().count(), "виды вместе дают весь список");
}

#[test]
fn single_kind_filters_keep_only_their_kind_with_the_real_screen_counts() {
    let list = calibrate_list();
    let whole = summarize(&list, SurfaceSizePx::ONEPLUS_11);
    let glass = summarize(&DrawFilter::GlassOnly.apply(&list), SurfaceSizePx::ONEPLUS_11);
    assert_eq!(
        (glass.glass_groups, glass.shapes, glass.texts, glass.icons, glass.images),
        (whole.glass_groups, 0, 0, 0, 0)
    );
    let shapes = summarize(&DrawFilter::ShapesOnly.apply(&list), SurfaceSizePx::ONEPLUS_11);
    assert_eq!((shapes.shapes, shapes.glass_groups, shapes.texts), (whole.shapes, 0, 0));
    let text = summarize(&DrawFilter::TextOnly.apply(&list), SurfaceSizePx::ONEPLUS_11);
    assert_eq!((text.texts, text.icons, text.shapes), (whole.texts, whole.icons, 0));
    let images = summarize(&DrawFilter::ImagesOnly.apply(&list), SurfaceSizePx::ONEPLUS_11);
    assert_eq!(
        (images.images, images.live_images, images.glass_groups),
        (whole.images, whole.live_images, 0)
    );
    assert!(whole.live_images > 0, "у CALIBRATE есть живая картинка прибора");
}

#[test]
fn no_glass_drops_only_glass_and_keeps_the_order_of_the_rest() {
    // После 14p стекла нет ни на одном экране в покое: к настоящему списку PHOTO подмешаны две группы стекла
    // (посередине и в конце), фильтр обязан убрать ровно их.
    let mut list = photo_list();
    let mut glass = crate::saturation_bench::test_glass::two_glass_panels().into_iter();
    let middle = list.items.len() / 2;
    list.items.insert(middle, glass.next().expect("первая группа"));
    list.items.extend(glass);
    let without = DrawFilter::NoGlass.apply(&list);
    let expected: Vec<&DrawItem> = list
        .leaf_items()
        .filter(|item| !matches!(item, DrawItem::Glass(_)))
        .collect();
    assert_eq!(
        without.leaf_items().collect::<Vec<_>>(),
        expected,
        "порядок остальных сохранён"
    );
    assert_eq!(summarize(&without, SurfaceSizePx::ONEPLUS_11).glass_groups, 0);
    assert!(
        without.leaf_items().count() < list.leaf_items().count(),
        "стекло в списке было"
    );
}

#[test]
fn every_filtered_list_stays_valid_and_keeps_the_light() {
    let list = calibrate_list();
    for filter in [DrawFilter::All, DrawFilter::NoGlass].into_iter().chain(SINGLE_KINDS) {
        let filtered = filter.apply(&list);
        filtered
            .validate()
            .unwrap_or_else(|error| panic!("{filter:?}: список невалиден: {error}"));
        assert_eq!(filtered.light, list.light, "{filter:?}");
    }
}

/// Фигура и иконка внутри группы обрезки, фигура снаружи: фильтр обязан применяться к листьям внутри группы.
#[test]
fn filter_applies_inside_clip_groups_and_keeps_the_group_rect() {
    use ui_draw::draw_list::{ClipGroup, IconItem};
    use ui_draw::geometry::{PxPoint, PxRect, PxSize};

    let list = calibrate_list();
    let shape = list
        .leaf_items()
        .find(|item| matches!(item, DrawItem::Shape(_)))
        .expect("в сцене есть фигура")
        .clone();
    let icon = DrawItem::Icon(IconItem {
        icon: ui_draw::icons::IconId::Grid,
        transform: ui_draw::transform::Affine2::translate(5.0, 5.0),
        size_px: PxSize::new(10.0, 10.0),
        color: ui_draw::color::DisplayColor::WHITE,
        opacity: ui_draw::color::Opacity::OPAQUE,
    });
    let rect = PxRect::new(PxPoint::new(1.0, 2.0), PxSize::new(30.0, 40.0));
    let mut nested = DrawList::new(list.light);
    nested.items = vec![
        shape.clone(),
        DrawItem::Clipped(ClipGroup {
            rect,
            items: vec![shape.clone(), icon.clone()],
        }),
    ];

    let shapes_only = DrawFilter::ShapesOnly.apply(&nested);
    assert_eq!(shapes_only.leaf_items().count(), 2, "обе фигуры, иконка убрана");
    assert!(shapes_only.leaf_items().all(|item| matches!(item, DrawItem::Shape(_))));
    assert!(
        matches!(&shapes_only.items[1], DrawItem::Clipped(group) if group.rect == rect && group.items == vec![shape.clone()])
    );

    // Вид, которого в группе нет, убирает группу целиком (пустая группа ничего не рисовала бы).
    let glass_only = DrawFilter::GlassOnly.apply(&nested);
    assert!(glass_only.items.is_empty());
    assert_eq!(DrawFilter::All.apply(&nested), nested);
    // Сводка считает листья внутри групп.
    let summary = summarize(&nested, SurfaceSizePx::ONEPLUS_11);
    assert_eq!((summary.shapes, summary.icons), (2, 1));
}
