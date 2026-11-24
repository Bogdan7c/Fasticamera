//! Стенд насыщения приложения на настоящем GPU ПК (RADV, полная validation): ВСЕ работы каталога через раннер.
//!
//! Что проверяется (результат, а не вызов):
//! * по каждой работе есть цифра пропускной способности (`Measured`), кадры в окне > 0, метки проходов ожидаемые;
//! * работы показа рисуют НАСТОЯЩИЙ экран камеры: стеклянные группы, глифы, фигуры; раздел CALIBRATE рисует живую
//!   картинку прибора (`live_images_drawn > 0`, `live_images_missing == 0`);
//! * `*_recompose` строят пирамиду размытия один раз, «новый кадр на каждом кадре» — на каждом кадре;
//! * после раннера живые объекты Vulkan вернулись к исходному числу, validation не нашла ошибок;
//! * списки рисования PHOTO и CALIBRATE различаются по составу и оба валидны.
//!
//! Печатает таблицу цифр: `cargo test -p fasticamera-app --release --test saturation_gpu -- --nocapture`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::sync::Mutex;
use std::time::Duration;

use camera_contract::fixtures::{camera_id, front_camera, main_wide_camera, ultra_wide_camera};
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::lens_shading::LensShadingCalibrations;
use camera_core::role_classifier::classify_catalog;
use camera_core::zoom::{ZoomConfig, build_zoom_catalog};
use camera_screen::{ActiveLens, LensFacts, LensSwitch};
use fasticamera::camera_screen_config::CameraScreenConfig;
use fasticamera::look_service::LookHandle;
use fasticamera::look_startup::catalog_environment;
use fasticamera::power_measurement::ThermalSnapshot;
use fasticamera::saturation_bench::catalog::{
    BackdropRhythm, CatalogEntry, DisplayScreen, WorkloadGroup, WorkloadKind, find_entry, group_entries,
};
use fasticamera::saturation_bench::digest_report::DigestOutcome;
use fasticamera::saturation_bench::draw_filter::DrawFilter;
use fasticamera::saturation_bench::gpu_executor::GpuWorkloadExecutor;
use fasticamera::saturation_bench::inputs::{InputSources, ProductInputs, SensorSource, gather_inputs};
use fasticamera::saturation_bench::report::{RowOutcome, RunSummary, WorkloadRow};
use fasticamera::saturation_bench::runner::{BenchRunner, PauseOutcome, RunClock, RunEvent, ThermalProbe};
use fasticamera::saturation_bench::screen_scene::{ScreenPlan, SurfaceSizePx};
use fasticamera::saturation_bench::summary::{detail_item, table_row};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::saturation::CancelToken;
use gpu_vulkan::swapchain::FramesInFlight;
use gpu_vulkan::validation::ValidationMode;
use look_presets::PresetStoreConfig;
use render_loop::config::RenderLoopConfig;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Замер честен, только когда GPU занят одним тестом.
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_context_or_skip() -> Option<GpuContext> {
    match GpuContext::new(&GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL)) {
        Ok(context) => Some(context),
        Err(error) if std::env::var(SKIP_ENV).as_deref() == Ok("1") => {
            eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
            None
        }
        Err(error) => panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно"),
    }
}

/// Входы продукта на ПК: запасной сенсор, библиотека пресетов в памяти, шкала объективов OnePlus 11 (как у golden).
fn product_inputs() -> ProductInputs {
    let render = RenderLoopConfig::default();
    let look = LookHandle::in_memory(&PresetStoreConfig::default(), &catalog_environment(&render)).expect("библиотека");
    let screen = CameraScreenConfig::default();
    let lens_shading = LensShadingCalibrations::none();
    let mut inputs = gather_inputs(&InputSources {
        engine_status: None,
        lens_shading: &lens_shading,
        render_config: &render,
        look: Some(&look),
        screen: &screen,
    });
    let classified = classify_catalog(
        vec![main_wide_camera(), ultra_wide_camera(), front_camera()],
        &camera_core::config::CameraEngineConfig::default().roles,
    )
    .expect("образцы камер без повторов");
    inputs.camera_facts.lens = LensFacts {
        catalog: Some(std::sync::Arc::new(build_zoom_catalog(
            &classified.catalog,
            &ZoomConfig::default(),
        ))),
        active: Some(ActiveLens {
            camera: camera_id("2"),
            role: LensRole::Wide,
            facing: LensFacing::Back,
        }),
        switch: LensSwitch::Steady,
        last_failure: None,
    };
    inputs
}

struct NoPause;

impl RunClock for NoPause {
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
    fn pause(&self, _duration: Duration, _cancel: &CancelToken) -> PauseOutcome {
        PauseOutcome::Elapsed
    }
}

struct NoThermal;

impl ThermalProbe for NoThermal {
    fn snapshot(&self) -> ThermalSnapshot {
        ThermalSnapshot {
            status: None,
            headroom: None,
            error: Some("на ПК теплового сервиса нет".to_owned()),
        }
    }
}

fn bench_with_window(measure_ms: u64) -> gpu_vulkan::saturation::SaturationConfig {
    gpu_vulkan::saturation::SaturationConfig {
        frames_in_flight: FramesInFlight::new(2).unwrap(),
        warmup: Duration::from_millis(100),
        measure: Duration::from_millis(measure_ms),
        timeline_step: Duration::from_millis(100),
        fence_timeout: Duration::from_secs(30),
    }
}

fn run_entries(context: &GpuContext, entries: &[&'static CatalogEntry]) -> RunSummary {
    run_entries_with_window(context, entries, 300)
}

/// Прогон списка работ с окном замера `measure_ms` (от окна зависит, сколько кадров успеет пройти).
fn run_entries_with_window(context: &GpuContext, entries: &[&'static CatalogEntry], measure_ms: u64) -> RunSummary {
    let mut executor = GpuWorkloadExecutor::new(
        context.clone(),
        product_inputs(),
        bench_with_window(measure_ms),
        SurfaceSizePx::ONEPLUS_11,
    );
    let mut events: Vec<RunEvent> = Vec::new();
    let mut observer = |event: RunEvent| events.push(event);
    let summary = BenchRunner {
        executor: &mut executor,
        clock: &NoPause,
        thermal: &NoThermal,
        observer: &mut observer,
        cooldown_between: Duration::ZERO,
    }
    .run(entries, &CancelToken::new());
    drop(executor);
    assert_eq!(
        events.len(),
        entries.len() * 2,
        "на каждую работу событие начала и конца"
    );
    summary
}

fn row<'a>(summary: &'a RunSummary, name: &str) -> &'a WorkloadRow {
    summary.rows.iter().find(|row| row.name == name).expect("строка работы")
}

fn expected_labels(entry: &CatalogEntry) -> Vec<&'static str> {
    match entry.kind {
        WorkloadKind::Viewfinder { .. } => vec!["isp-end"],
        WorkloadKind::Scopes { .. } | WorkloadKind::ScopesAblated { .. } => vec!["accumulate-end", "render-end"],
        WorkloadKind::Compositor => vec!["compose"],
        WorkloadKind::Display { .. } => vec!["compose", "pyramid", "ui"],
        WorkloadKind::HeadroomV4 => isp_vulkan::saturation::HeadroomSaturationConfig::default().pass_labels(),
        // У снимка времён по проходам нет по построению (в `photo` нет timestamp-запросов): каждый кадр «без замера».
        WorkloadKind::Still => Vec::new(),
    }
}

#[test]
fn every_workload_of_the_catalog_is_measured_on_a_real_gpu_and_released_one_by_one() {
    let _serial = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let entries: Vec<&'static CatalogEntry> = group_entries(WorkloadGroup::Baseline);

    let summary = run_entries(&context, &entries);

    println!(
        "\nСтенд насыщения приложения, {}, прогрев 100 мс, окно 300 мс, 2 слота (validation FULL: цифры завышены)",
        context.capabilities().device_name
    );
    println!("| работа | к/с | мс/кадр | вердикт | проходы (медиана, мс) |");
    for row in &summary.rows {
        let passes = row.measurement().map_or_else(String::new, |measurement| {
            measurement
                .passes
                .iter()
                .map(|pass| format!("{} {:.3}", pass.label, pass.median_ms))
                .collect::<Vec<_>>()
                .join("; ")
        });
        println!("| {} | {passes} |", table_row(row).replace('|', " | "));
    }

    assert!(!summary.cancelled);
    assert_eq!(summary.rows.len(), 14);
    for (entry, row) in entries.iter().zip(&summary.rows) {
        let RowOutcome::Measured { measurement } = &row.outcome else {
            panic!("{}: нет цифры: {:?}", entry.name, row.outcome);
        };
        assert!(measurement.measured_frames > 0, "{}: в окне нет кадров", entry.name);
        assert!(measurement.frames_per_second.unwrap() > 0.0, "{}", entry.name);
        let labels: Vec<&str> = measurement.passes.iter().map(|pass| pass.label).collect();
        let expected = expected_labels(entry);
        if expected.is_empty() {
            assert!(labels.is_empty(), "{}: проходов у снимка нет", entry.name);
            assert!(
                measurement.unmeasured_pass_frames > 0,
                "{}: кадры «без замера» посчитаны",
                entry.name
            );
        } else {
            assert_eq!(labels, expected, "{}: метки проходов", entry.name);
        }
        assert!(!measurement.timeline_fps.is_empty(), "{}: шкалы нет", entry.name);
    }

    // Работы показа рисуют настоящий экран, а не пустой.
    for name in [
        "display_photo",
        "display_photo_recompose",
        "display_calibrate",
        "display_calibrate_recompose",
    ] {
        let row = row(&summary, name);
        let scene = row.scene.expect("состав сцены");
        let ui = row.ui.expect("счётчики UI");
        assert!(
            scene.glass_groups > 0 && scene.texts > 0,
            "{name}: пустая сцена {scene:?}"
        );
        assert!(
            ui.glass_groups > 0 && ui.glyph_instances > 0 && ui.shape_instances > 0,
            "{name}: UI не нарисован {ui:?}"
        );
    }
    let photo = row(&summary, "display_photo");
    let calibrate = row(&summary, "display_calibrate");
    assert_eq!(photo.scene.unwrap().live_images, 0, "в PHOTO прибора нет");
    assert_eq!(photo.ui.unwrap().live_images_drawn, 0);
    assert!(
        calibrate.scene.unwrap().live_images > 0,
        "в CALIBRATE есть панель прибора"
    );
    let calibrate_ui = calibrate.ui.unwrap();
    assert!(
        calibrate_ui.live_images_drawn > 0,
        "живая картинка прибора нарисована: {calibrate_ui:?}"
    );
    assert_eq!(
        calibrate_ui.live_images_missing, 0,
        "картинка прибора найдена на каждом кадре"
    );
    assert_ne!(photo.scene, calibrate.scene, "экраны различаются по составу");

    // Каденция фона: пирамида строится на каждом кадре или один раз.
    for name in ["display_photo", "display_calibrate"] {
        let ui = row(&summary, name).ui.unwrap();
        assert_eq!(
            ui.pyramid_rebuilds, ui.frames_recorded,
            "{name}: новый кадр камеры на каждом кадре"
        );
    }
    for name in ["display_photo_recompose", "display_calibrate_recompose"] {
        let ui = row(&summary, name).ui.unwrap();
        assert_eq!(
            ui.pyramid_rebuilds, 1,
            "{name}: тот же кадр камеры — пирамида строится один раз"
        );
        assert!(ui.pyramid_reuses > 0, "{name}");
    }

    // Работы освобождены одна за другой: к концу живых объектов столько же, сколько до начала.
    context.wait_idle().unwrap();
    assert_eq!(context.live_objects().differences_from(&baseline), Vec::new());
    assert_eq!(
        context.validation_report().errors,
        0,
        "validation: {:?}",
        context.validation_report()
    );
}

#[test]
fn photo_and_calibrate_draw_lists_are_valid_and_differ_by_what_the_real_screen_draws() {
    let inputs = product_inputs();
    let plan = ScreenPlan::build(&inputs, SurfaceSizePx::ONEPLUS_11).expect("экраны строятся");
    let photo = plan.scene(DisplayScreen::Photo);
    let calibrate = plan.scene(DisplayScreen::Calibrate);
    photo.draw_list.validate().expect("список PHOTO валиден");
    calibrate.draw_list.validate().expect("список CALIBRATE валиден");
    assert!(photo.summary.glass_groups > 0 && calibrate.summary.glass_groups > 0);
    // В CALIBRATE есть панель прибора (живая картинка) и колонка ручек (больше значков и подписей).
    assert_eq!(photo.summary.live_images, 0);
    assert!(
        calibrate.summary.live_images > 0,
        "панель прибора: {:?}",
        calibrate.summary
    );
    assert!(photo.scope_request.is_none());
    let request = calibrate.scope_request.expect("раздел просит прибор");
    assert!(request.image_size.width() > 0 && request.image_size.height() > 0);
    assert_ne!(photo.summary, calibrate.summary);
    assert_ne!(
        photo.viewfinder, calibrate.viewfinder,
        "в CALIBRATE видоискатель на весь экран"
    );
    assert_eq!((calibrate.viewfinder.width, calibrate.viewfinder.height), (1080, 2412));
    assert!(matches!(inputs.sensor_source, SensorSource::Fallback { .. }));
    // Умолчание каталога не перепутало имя и вид.
    assert_eq!(
        find_entry("display_calibrate").unwrap().kind,
        WorkloadKind::Display {
            screen: DisplayScreen::Calibrate,
            rhythm: BackdropRhythm::NewFrameEveryFrame,
            filter: DrawFilter::All,
        }
    );
}

/// Для проверки скрипта сравнения на настоящем формате: подробности прогона в виде строк `FASTICAMERA_DETAIL`
/// (как их печатает `device-smoke.sh`) пишутся в `<каталог из FASTICAMERA_SATURATION_DUMP>/<имя>.txt`.
fn dump_details_if_asked(summary: &RunSummary, file_stem: &str) {
    let Ok(directory) = std::env::var("FASTICAMERA_SATURATION_DUMP") else {
        return;
    };
    let items: Vec<serde_json::Value> = summary.rows.iter().map(detail_item).collect();
    let line = serde_json::json!({
        "scenario": "gpu-saturation-bench", "stage": "detail_rows", "part": 1, "parts": 1, "items": items
    });
    let path = std::path::Path::new(&directory).join(format!("{file_stem}.txt"));
    std::fs::write(&path, format!("FASTICAMERA_DETAIL {line}\n")).expect("запись выгрузки подробностей");
}

/// Общий отпечаток выхода строки; паника с причиной, если отпечатка нет.
fn combined_digest(row: &WorkloadRow) -> String {
    match row
        .digest
        .as_ref()
        .unwrap_or_else(|| panic!("{}: у строки нет дайджеста", row.name))
    {
        DigestOutcome::Computed { combined, .. } => combined.clone(),
        DigestOutcome::Unavailable { reason } => panic!("{}: дайджест недоступен: {reason}", row.name),
    }
}

fn entries_named(names: &[&str]) -> Vec<&'static CatalogEntry> {
    names
        .iter()
        .map(|name| find_entry(name).expect("имя каталога"))
        .collect()
}

fn measured_frames(row: &WorkloadRow) -> u64 {
    let RowOutcome::Measured { measurement } = &row.outcome else {
        panic!("{}: нет цифры: {:?}", row.name, row.outcome);
    };
    measurement.measured_frames
}

#[test]
fn ui_ablations_draw_exactly_the_filtered_scene_and_each_kind_gives_a_different_picture() {
    let _serial = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut names = vec!["display_calibrate", "display_photo"];
    names.extend(group_entries(WorkloadGroup::UiAblation).iter().map(|entry| entry.name));
    let summary = run_entries(&context, &entries_named(&names));

    println!("\nАбляции прохода UI (validation FULL, окно 300 мс):");
    println!("| работа | мс/кадр | UI, мс | стекло px | доля экрана | дайджест |");
    for row in &summary.rows {
        let measurement = row.measurement().expect("измерение");
        let ui_pass = measurement
            .passes
            .iter()
            .find(|pass| pass.label == "ui")
            .map_or(0.0, |pass| pass.median_ms);
        let scene = row.scene.expect("состав сцены");
        println!(
            "| {} | {:.3} | {ui_pass:.3} | {} | {:.3} | {} |",
            row.name,
            measurement.frame_cost_ms.unwrap_or(0.0),
            scene.glass_bounds_area_px,
            scene.glass_screen_fraction,
            combined_digest(row)
        );
        assert!(measured_frames(row) > 0, "{}", row.name);
    }

    let by = |name: &str| row(&summary, name);
    let (full, glass, shapes, text, images, no_glass) = (
        by("display_calibrate"),
        by("display_calibrate_glass_only"),
        by("display_calibrate_shapes_only"),
        by("display_calibrate_text_only"),
        by("display_calibrate_images_only"),
        by("display_calibrate_no_glass"),
    );
    // Состав сцены в результате — фактический, после фильтра, и рендер нарисовал именно его.
    let (full_scene, full_ui) = (full.scene.unwrap(), full.ui.unwrap());
    let (glass_scene, glass_ui) = (glass.scene.unwrap(), glass.ui.unwrap());
    assert_eq!(glass_scene.glass_groups, full_scene.glass_groups);
    assert_eq!(
        (
            glass_scene.shapes,
            glass_scene.texts,
            glass_scene.icons,
            glass_scene.images
        ),
        (0, 0, 0, 0)
    );
    assert!(
        glass_ui.glass_groups > 0 && glass_ui.glyph_instances == 0 && glass_ui.live_images_drawn == 0,
        "{glass_ui:?}"
    );
    assert!(
        glass_scene.glass_bounds_area_px > 0 && glass_scene.glass_screen_fraction > 0.0,
        "{glass_scene:?}"
    );
    assert_eq!(
        glass_scene.glass_bounds_area_px, full_scene.glass_bounds_area_px,
        "геометрия стекла та же"
    );

    let shapes_ui = shapes.ui.unwrap();
    assert!(
        shapes_ui.shape_instances > 0 && shapes_ui.glass_groups == 0 && shapes_ui.glyph_instances == 0,
        "{shapes_ui:?}"
    );
    let text_ui = text.ui.unwrap();
    assert!(
        text_ui.glyph_instances > 0 && text_ui.glass_groups == 0 && text_ui.shape_instances == 0,
        "{text_ui:?}"
    );
    let images_ui = images.ui.unwrap();
    assert!(
        images_ui.live_images_drawn > 0 && images_ui.live_images_missing == 0 && images_ui.glass_groups == 0,
        "{images_ui:?}"
    );
    let no_glass_ui = no_glass.ui.unwrap();
    assert_eq!(no_glass.scene.unwrap().glass_groups, 0);
    assert!(
        no_glass_ui.glass_groups == 0
            && no_glass_ui.glyph_instances
                == full_ui.glyph_instances / full_ui.frames_recorded * no_glass_ui.frames_recorded,
        "всё остальное нарисовано: {no_glass_ui:?} против {full_ui:?}"
    );

    // Разные вырезки — разные картинки; полный экран отличается от стекла отдельно.
    let digests: Vec<String> = [full, glass, shapes, text, images, no_glass]
        .iter()
        .map(|row| combined_digest(row))
        .collect();
    let distinct: std::collections::HashSet<&String> = digests.iter().collect();
    assert_eq!(
        distinct.len(),
        digests.len(),
        "каждая вырезка даёт свою картинку: {digests:?}"
    );

    // PHOTO: стекло отдельно и без стекла тоже различаются, и оба отличаются от всего экрана.
    let (photo, photo_glass, photo_flat) = (
        by("display_photo"),
        by("display_photo_glass_only"),
        by("display_photo_no_glass"),
    );
    let photo_digests = [
        combined_digest(photo),
        combined_digest(photo_glass),
        combined_digest(photo_flat),
    ];
    assert_eq!(
        photo_digests.iter().collect::<std::collections::HashSet<_>>().len(),
        3,
        "{photo_digests:?}"
    );

    context.wait_idle().unwrap();
    assert_eq!(context.live_objects().differences_from(&baseline), Vec::new());
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn isp_ablations_are_measured_with_pass_times_and_stage_changes_change_the_picture_but_awb_statistics_do_not() {
    let _serial = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(context) = gpu_context_or_skip() else { return };
    let mut names = vec!["viewfinder"];
    names.extend(group_entries(WorkloadGroup::IspAblation).iter().map(|entry| entry.name));
    let summary = run_entries(&context, &entries_named(&names));

    println!("\nАбляции видоискателя (validation FULL, окно 300 мс):");
    println!("| работа | мс/кадр | проход ISP, мс | p95 прохода, мс | дайджест | кадр |");
    for row in &summary.rows {
        let measurement = row.measurement().expect("измерение");
        let pass = &measurement.passes[0];
        let note = match row.digest.as_ref().expect("дайджест") {
            DigestOutcome::Computed { frame_note, .. } => frame_note.clone().unwrap_or_default(),
            DigestOutcome::Unavailable { reason } => reason.clone(),
        };
        println!(
            "| {} | {:.3} | {:.3} | {:.3} | {} | {note} |",
            row.name,
            measurement.frame_cost_ms.unwrap_or(0.0),
            pass.median_ms,
            pass.p95_ms,
            combined_digest(row)
        );
        // Времена проходов и p95 попадают в детали и у работ с изменённым набором стадий.
        assert_eq!(measurement.passes.len(), 1, "{}", row.name);
        assert_eq!(pass.label, "isp-end", "{}", row.name);
        assert!(
            pass.p95_ms >= pass.median_ms && pass.max_ms >= pass.p95_ms,
            "{}",
            row.name
        );
        assert!(measurement.cost.is_some(), "{}: нет p95 и max цены кадра", row.name);
    }
    let digest = |name: &str| combined_digest(row(&summary, name));
    // Статистика AWB — только счёт: картинка не должна зависеть ни от неё, ни от её каденции.
    assert_eq!(digest("viewfinder"), digest("viewfinder_awb_never"));
    assert_eq!(digest("viewfinder"), digest("viewfinder_awb_every_frame"));
    // Стадии, меняющие картинку, меняют дайджест (иначе дайджест ничего не доказывает).
    for name in [
        "viewfinder_without_tone",
        "viewfinder_without_oetf",
        "viewfinder_without_white_balance",
        "viewfinder_without_color",
        "viewfinder_minimal",
    ] {
        assert_ne!(
            digest(name),
            digest("viewfinder"),
            "{name}: картинка не изменилась без стадии"
        );
    }
    println!(
        "Стадии, при отключении которых дайджест совпал с продуктовым (в этом наборе параметров они уже ничего не делают): {:?}",
        summary
            .rows
            .iter()
            .filter(|row| row.name.starts_with("viewfinder_without_") && combined_digest(row) == digest("viewfinder"))
            .map(|row| row.name)
            .collect::<Vec<_>>()
    );
    // Заметка о кадре: какой кадр читали (со статистикой AWB или без).
    let note = |name: &str| match row(&summary, name).digest.as_ref().unwrap() {
        DigestOutcome::Computed { frame_note, .. } => frame_note.clone(),
        DigestOutcome::Unavailable { .. } => None,
    };
    assert_eq!(
        note("viewfinder_awb_never").as_deref(),
        Some("last_frame=no_awb_statistics")
    );
    assert_eq!(
        note("viewfinder_awb_every_frame").as_deref(),
        Some("last_frame=awb_statistics")
    );
}

#[test]
fn the_digest_of_every_baseline_work_is_the_same_for_two_runs_with_different_windows() {
    let _serial = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(context) = gpu_context_or_skip() else { return };
    let entries = group_entries(WorkloadGroup::Baseline);
    let short = run_entries_with_window(&context, &entries, 150);
    let long = run_entries_with_window(&context, &entries, 500);
    dump_details_if_asked(&short, "short");
    dump_details_if_asked(&long, "long");
    // Каждая строка подробностей помещается в строку стадии logcat вместе с дайджестом и ценой.
    for row in short.rows.iter().chain(&long.rows) {
        let bytes = detail_item(row).to_string().len();
        assert!(
            bytes < 3_400,
            "{}: строка подробностей {bytes} байт не влезает в бюджет строки",
            row.name
        );
    }
    println!("\nДайджесты baseline (окно 150 мс и 500 мс):");
    for (first, second) in short.rows.iter().zip(&long.rows) {
        assert_eq!(first.name, second.name);
        let (digest_short, digest_long) = (combined_digest(first), combined_digest(second));
        println!(
            "| {} | {digest_short} | {digest_long} | кадров {} и {} |",
            first.name,
            measured_frames(first),
            measured_frames(second)
        );
        assert_eq!(
            digest_short, digest_long,
            "{}: дайджест зависит от числа кадров прогона",
            first.name
        );
    }
    let counts: Vec<u64> = short
        .rows
        .iter()
        .map(measured_frames)
        .zip(long.rows.iter().map(measured_frames))
        .map(|(a, b)| u64::from(a != b))
        .collect();
    assert!(
        counts.iter().sum::<u64>() > 0,
        "окна разной длины действительно дали разное число кадров"
    );
}

#[test]
fn the_digest_follows_the_picture_not_the_run_two_different_works_never_share_one() {
    let _serial = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(context) = gpu_context_or_skip() else { return };
    let entries = group_entries(WorkloadGroup::Baseline);
    let summary = run_entries(&context, &entries);
    // Приборы дают две части (счётчики и картинка), снимок — картинку и миниатюру, остальные — одну.
    let parts = |name: &str| match row(&summary, name).digest.as_ref().unwrap() {
        DigestOutcome::Computed { parts, .. } => parts.iter().map(|part| part.label).collect::<Vec<_>>(),
        DigestOutcome::Unavailable { reason } => panic!("{name}: {reason}"),
    };
    assert_eq!(parts("viewfinder"), ["image"]);
    assert_eq!(parts("scopes_waveform"), ["counters", "picture"]);
    assert_eq!(parts("display_calibrate"), ["target"]);
    assert_eq!(parts("headroom_v4"), ["output"]);
    assert!(parts("still").starts_with(&["image"]), "{:?}", parts("still"));
    // Разные выходы — разные отпечатки (серый кадр и сцена, waveform и parade — не одна картинка).
    let distinct = |names: &[&str]| {
        let digests: Vec<String> = names.iter().map(|name| combined_digest(row(&summary, name))).collect();
        assert_eq!(
            digests.iter().collect::<std::collections::HashSet<_>>().len(),
            names.len(),
            "{names:?}: {digests:?}"
        );
    };
    distinct(&[
        "scopes_waveform",
        "scopes_parade",
        "scopes_vectorscope_ycbcr",
        "scopes_vectorscope_oklab",
        "scopes_parade_gray",
    ]);
    distinct(&["display_photo", "display_calibrate", "compositor"]);
    // Тот же экран с пирамидой, построенной каждый кадр, и с переиспользованной — картинка та же.
    assert_eq!(
        combined_digest(row(&summary, "display_calibrate")),
        combined_digest(row(&summary, "display_calibrate_recompose")),
        "каденция пирамиды не меняет картинку"
    );
}
