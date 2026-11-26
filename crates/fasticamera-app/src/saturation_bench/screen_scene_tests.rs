//! Списки рисования экрана камеры для работ показа: настоящий экран на фактах продукта (без GPU).

use camera_contract::lens_shading::LensShadingCalibrations;
use render_loop::config::RenderLoopConfig;
use scope_contract::ScopeKind;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::look_service::LookHandle;
use crate::look_startup::catalog_environment;
use crate::saturation_bench::inputs::{InputSources, gather_inputs};
use ui_draw::geometry::PxPoint;

fn inputs(with_look: bool) -> ProductInputs {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let none = LensShadingCalibrations::none();
    let look = LookHandle::in_memory(
        &look_presets::PresetStoreConfig::default(),
        &catalog_environment(&render),
    )
    .expect("библиотека");
    gather_inputs(&InputSources {
        engine_status: None,
        lens_shading: &none,
        render_config: &render,
        look: with_look.then_some(&look),
        screen: &screen,
    })
}

#[test]
fn photo_frame_is_inside_the_screen_and_calibrate_viewfinder_covers_all_of_it() {
    let plan = ScreenPlan::build(&inputs(true), SurfaceSizePx::ONEPLUS_11).expect("экраны");
    let photo = plan.scene(DisplayScreen::Photo).viewfinder;
    let SurfaceSizePx { width, height } = SurfaceSizePx::ONEPLUS_11;
    assert!(photo.width > 0 && photo.height > 0);
    assert!(
        photo.x + photo.width <= width && photo.y + photo.height <= height,
        "{photo:?}"
    );
    assert!(
        photo.height < height,
        "кадр PHOTO занимает не весь экран: сверху и снизу полосы управления"
    );
    let calibrate = plan.scene(DisplayScreen::Calibrate).viewfinder;
    assert_eq!(
        (calibrate.x, calibrate.y, calibrate.width, calibrate.height),
        (0, 0, width, height)
    );
}

#[test]
fn calibrate_asks_for_a_scope_sized_by_the_layout_and_photo_does_not() {
    let plan = ScreenPlan::build(&inputs(true), SurfaceSizePx::ONEPLUS_11).expect("экраны");
    assert!(plan.scene(DisplayScreen::Photo).scope_request.is_none());
    let request = plan
        .scene(DisplayScreen::Calibrate)
        .scope_request
        .expect("раздел просит прибор");
    // Умолчание раздела — waveform; размер — из раскладки экрана, а не прежняя оценка 540×360.
    assert_eq!(request.kind, ScopeKind::Waveform);
    assert_ne!(
        (request.image_size.width(), request.image_size.height()),
        (540, 360),
        "размер картинки прибора берётся из раскладки экрана"
    );
}

#[test]
fn calibrate_with_the_look_library_draws_more_than_without_it() {
    let with = ScreenPlan::build(&inputs(true), SurfaceSizePx::ONEPLUS_11).expect("с видом");
    let without = ScreenPlan::build(&inputs(false), SurfaceSizePx::ONEPLUS_11).expect("без вида");
    let (with, without) = (
        with.scene(DisplayScreen::Calibrate).summary,
        without.scene(DisplayScreen::Calibrate).summary,
    );
    assert!(
        with.texts + with.icons + with.shapes > without.texts + without.icons + without.shapes,
        "колонка ручек и пресеты рисуются, только когда у экрана есть факты о виде: {with:?} против {without:?}"
    );
}

#[test]
fn summary_counts_items_by_kind_and_live_images_separately() {
    let plan = ScreenPlan::build(&inputs(true), SurfaceSizePx::ONEPLUS_11).expect("экраны");
    let scene = plan.scene(DisplayScreen::Calibrate);
    let recount = summarize(&scene.draw_list, SurfaceSizePx::ONEPLUS_11);
    assert_eq!(recount, scene.summary);
    let total = recount.shapes + recount.glass_groups + recount.texts + recount.icons + recount.images;
    assert_eq!(
        total as usize,
        scene.draw_list.leaf_items().count(),
        "каждый элемент списка учтён ровно один раз"
    );
    assert!(recount.live_images <= recount.images);
}

#[test]
fn zero_width_surface_is_a_typed_error_not_a_panic() {
    let error = ScreenPlan::build(&inputs(true), SurfaceSizePx { width: 0, height: 2412 })
        .err()
        .expect("нулевая ширина");
    assert!(
        matches!(error, ScreenPlanError::PointScale { width_px: 0, .. }),
        "{error}"
    );
}

#[test]
fn glass_geometry_is_the_clamped_sum_of_group_bounds_and_the_screen_fraction() {
    let surface = SurfaceSizePx::ONEPLUS_11;
    let plan = ScreenPlan::build(&inputs(true), surface).expect("экраны");
    // После 14m стекло есть только у PHOTO; раздел CALIBRATE стекла не рисует (проверяется ниже).
    let summary = plan.scene(DisplayScreen::Photo).summary;
    assert_eq!(summary.surface_area_px, 1080 * 2412);
    assert!(
        summary.glass_bounds_area_px > 0,
        "стекло закрывает часть экрана: {summary:?}"
    );
    let expected_fraction = summary.glass_bounds_area_px as f64 / summary.surface_area_px as f64;
    assert!(
        (summary.glass_screen_fraction - expected_fraction).abs() < 1e-6,
        "{summary:?}"
    );

    // Независимый пересчёт: сумма площадей bounds групп, обрезанных по экрану.
    let list = &plan.scene(DisplayScreen::Photo).draw_list;
    let recount: f64 = list
        .items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Glass(group) => group.bounds_px(),
            _ => None,
        })
        .map(|rect| {
            let width = f64::from(rect.max_x().min(1080.0) - rect.origin.x.max(0.0)).max(0.0);
            let height = f64::from(rect.max_y().min(2412.0) - rect.origin.y.max(0.0)).max(0.0);
            width * height
        })
        .sum();
    assert_eq!(summary.glass_bounds_area_px, recount.round() as u64);

    // Список без стекла — нулевая геометрия стекла.
    let without = crate::saturation_bench::draw_filter::DrawFilter::NoGlass.apply(list);
    let flat = summarize(&without, surface);
    assert_eq!((flat.glass_groups, flat.glass_bounds_area_px), (0, 0));
    assert_eq!(flat.glass_screen_fraction, 0.0);
}

#[test]
fn a_rect_hanging_over_the_screen_edge_counts_only_its_visible_part() {
    let surface = SurfaceSizePx {
        width: 100,
        height: 100,
    };
    let rect = PxRect::new(PxPoint::new(-20.0, 90.0), PxSize::new(60.0, 40.0));
    // Видимая часть: x от 0 до 40, y от 90 до 100.
    assert_eq!(clamped_rect(&rect, surface), Some((0.0, 90.0, 40.0, 100.0)));
    let outside = PxRect::new(PxPoint::new(200.0, 200.0), PxSize::new(10.0, 10.0));
    assert_eq!(clamped_rect(&outside, surface), None);
}

#[test]
fn union_area_counts_overlaps_once_and_disjoint_rects_fully() {
    // Два перекрывающихся квадрата 10×10 со сдвигом 5 по обеим осям: 100 + 100 - 25.
    assert_eq!(union_area(&[(0.0, 0.0, 10.0, 10.0), (5.0, 5.0, 15.0, 15.0)]), 175.0);
    // Один внутри другого: площадь большего.
    assert_eq!(union_area(&[(0.0, 0.0, 10.0, 10.0), (2.0, 2.0, 4.0, 4.0)]), 100.0);
    // Не пересекаются: сумма. Пустой список — ноль.
    assert_eq!(union_area(&[(0.0, 0.0, 2.0, 2.0), (5.0, 5.0, 6.0, 7.0)]), 6.0);
    assert_eq!(union_area(&[]), 0.0);
}

#[test]
fn the_real_screens_glass_union_is_not_larger_than_the_sum_nor_than_the_screen() {
    let plan = ScreenPlan::build(&inputs(true), SurfaceSizePx::ONEPLUS_11).expect("экраны");
    // CALIBRATE после 14m без стекла: площадь стекла нулевая; у PHOTO — положительная.
    let calibrate = plan.scene(DisplayScreen::Calibrate).summary;
    assert_eq!(
        (calibrate.glass_groups, calibrate.glass_union_area_px),
        (0, 0),
        "{calibrate:?}"
    );
    let photo = plan.scene(DisplayScreen::Photo).summary;
    assert!(photo.glass_union_area_px > 0, "{photo:?}");
    assert!(photo.glass_union_area_px <= photo.glass_bounds_area_px, "{photo:?}");
    assert!(photo.glass_union_screen_fraction <= 1.0 + 1e-9, "{photo:?}");
}
