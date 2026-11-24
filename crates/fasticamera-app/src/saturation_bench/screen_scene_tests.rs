//! Списки рисования экрана камеры для работ показа: настоящий экран на фактах продукта (без GPU).

use camera_contract::lens_shading::LensShadingCalibrations;
use render_loop::config::RenderLoopConfig;
use scope_contract::ScopeKind;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::look_service::LookHandle;
use crate::look_startup::catalog_environment;
use crate::saturation_bench::inputs::{InputSources, gather_inputs};

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
    let recount = summarize(&scene.draw_list);
    assert_eq!(recount, scene.summary);
    let total = recount.shapes + recount.glass_groups + recount.texts + recount.icons + recount.images;
    assert_eq!(
        total as usize,
        scene.draw_list.items.len(),
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
