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

fn calibrate_list() -> DrawList {
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
        .scene(DisplayScreen::Calibrate)
        .draw_list
        .clone()
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
        list.items.len() > 10,
        "сцена настоящая, а не пустая: {}",
        list.items.len()
    );
    for item in &list.items {
        let owners = SINGLE_KINDS.iter().filter(|filter| filter.keeps(item)).count();
        assert_eq!(owners, 1, "элемент {item:?} должен принадлежать ровно одному виду");
    }
    let total: usize = SINGLE_KINDS.iter().map(|filter| filter.apply(&list).items.len()).sum();
    assert_eq!(total, list.items.len(), "виды вместе дают весь список");
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
    let list = calibrate_list();
    let without = DrawFilter::NoGlass.apply(&list);
    let expected: Vec<&DrawItem> = list
        .items
        .iter()
        .filter(|item| !matches!(item, DrawItem::Glass(_)))
        .collect();
    assert_eq!(
        without.items.iter().collect::<Vec<_>>(),
        expected,
        "порядок остальных сохранён"
    );
    assert_eq!(summarize(&without, SurfaceSizePx::ONEPLUS_11).glass_groups, 0);
    assert!(without.items.len() < list.items.len(), "стекло в списке было");
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
