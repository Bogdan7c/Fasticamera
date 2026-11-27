//! Цветовые приборы и показ кадра (ревью 14h, решения 189, 190) через НАСТОЯЩИЙ бэкенд показа (RADV, headless,
//! полная validation), настоящий `RawFrameStream` и стенд непрерывности (`continuity_rig`):
//! * 5b: меряется только ПОКАЗАННАЯ картинка. Новые кадры потока B, которые ISP обработал, но экран скрыл
//!   удержанным кадром A, прибор не пишет: `passes_recorded` не растёт, `frames_skipped_not_shown` растёт ровно на
//!   число таких кадров, а первый живой кадр B добавляет ровно один проход. (Чёрный показ новым кадром этот стенд
//!   не даёт: чёрный наступает только на тиках без нового кадра, а там проход и так не пишется.)
//! * 5c: зеркальное окно (объектив с `PreviewMirror::Horizontal`) на НЕСИММЕТРИЧНОЙ сцене: счётчики GPU равны эталону
//!   с `HorizontalOrder::Mirrored` и отличаются от эталона «как хранится»; контроль без зеркала — наоборот.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use camera_contract::fixtures::camera_id;
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::viewfinder_crop::{CropAspect, ZoomFactor};
use camera_contract::zoom::{
    LensGroup, LensZoomPoint, PreviewMirror, ZoomCatalog, ZoomFraming, ZoomRange, ZoomRatio, ZoomStop, ZoomStopKind,
};
use continuity_rig::{MAIN, Rig, SIZE_A, SIZE_B, WIDE, dark, finish, gray, passport, test_continuity};
use render_loop::backend::DisplayBackend;
use render_loop::config::{RevealExposureCheck, ScopesConfig};
use render_loop::viewfinder::{
    FrameLabel, FramingCrop, FramingDestination, ScopeDiagnostics, TransitionKind, ViewfinderCommand,
    ViewfinderCommandOutcome, ViewfinderFraming,
};
use scope_contract::{ScopeImageSize, ScopeIntent, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    HorizontalOrder, ResolvedScope, SampleStride, ScopeCounters, ScopeModelSettings, ScopeRule, ShownCodes, accumulate,
};

/// Запрос waveform 160×96 (тот же, что в остальных тестах приборов).
fn waveform() -> ScopeRequest {
    ScopeRequest {
        kind: ScopeKind::Waveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(160, 96).expect("размер"),
    }
}

/// Маленькая модель приборов, шаг 1: меряется каждый пиксель (эталон на CPU недорог).
fn small_model() -> ScopeModelSettings {
    ScopeModelSettings {
        trace_max_columns: 48,
        trace_levels: 64,
        vectorscope_size: 64,
        sample_stride: SampleStride::try_new(1).expect("шаг"),
        ..ScopeModelSettings::default()
    }
}

fn show_scopes(rig: &mut Rig) {
    rig.command(ViewfinderCommand::Scopes(ScopeIntent::Show(waveform())));
}

fn scope_diagnostics(rig: &mut Rig) -> ScopeDiagnostics {
    rig.backend
        .diagnostics()
        .viewfinder
        .expect("диагностика видоискателя")
        .scopes
}

// ---- 5b ----

/// Новые кадры B, скрытые удержанием, прибор не меряет; показанный кадр — меряет.
///
/// Что проведено: A показан (проходы идут) → поток A закрылся → стартовал B с очень тёмной сценой при проверке
/// «правдоподобной яркости»: кадры B обрабатываются ISP, но экран держит кадр A до срока `max_hold_ms` (метка
/// `Transition(HeldLastFrame)`, у такого кадра в записи есть `crop`), потом первый кадр B показан живым.
#[test]
fn new_frames_hidden_by_the_hold_are_not_measured_and_the_first_shown_frame_is() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::MeasuredAndPlausible), true) else {
        return;
    };
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    show_scopes(&mut rig);
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    rig.stream.producer().end_stream(epoch_a);
    // Пока потока нет, тики — перекомпоновка удержанного кадра (нового кадра нет): проходов нет.
    let _gap = (0..3).map(|_| rig.tick()).collect::<Vec<_>>();
    let before = scope_diagnostics(&mut rig).counters;
    assert!(before.passes_recorded > 0, "пока A был живым, проходы шли: {before:?}");

    let epoch_b = rig.begin(&passport_b);
    let b_ticks = rig.feed_until_live(epoch_b, dark());
    let (reveal, hidden) = b_ticks.split_last().expect("есть тики");
    assert!(!hidden.is_empty(), "до срока кадры B скрыты удержанием");
    for tick in hidden {
        assert_eq!(tick.record.label, FrameLabel::Transition(TransitionKind::HeldLastFrame));
        assert!(tick.record.crop.is_some(), "кадр B действительно обработан ISP");
    }
    assert_eq!(reveal.record.label, FrameLabel::Live);

    let after = scope_diagnostics(&mut rig).counters;
    assert_eq!(
        after.frames_skipped_not_shown - before.frames_skipped_not_shown,
        hidden.len() as u64,
        "каждый скрытый новый кадр учтён как «не показан»: {after:?}"
    );
    assert_eq!(
        after.passes_recorded - before.passes_recorded,
        1,
        "проходов за переход ровно один — на показанном кадре B, скрытые не мерились: {after:?}"
    );
    finish(&mut rig, &[epoch_b]);
}

// ---- 5c ----

/// Объектив камеры: `mirror` — отражение превью (фронтальная/зеркальная камера).
fn lens(camera: &str, role: LensRole, native: f32, mirror: PreviewMirror) -> LensZoomPoint {
    LensZoomPoint {
        role,
        camera: camera_id(camera),
        native_ratio: ZoomRatio::new(native).expect("коэффициент"),
        equivalent_focal_mm: 24.0 / native,
        optical_axis: camera_contract::lens_calibration::OpticalAxis::Uncalibrated {
            reason: camera_contract::lens_calibration::UncalibratedReason::NotReported,
        },
        active_array: SIZE_A,
        preview_mirror: mirror,
        max_digital_zoom: ZoomFactor::new(10.0).expect("зум"),
    }
}

/// Каталог: основная камера не отражается, «широкая» (WIDE) — отражается по горизонтали.
fn catalog() -> Arc<ZoomCatalog> {
    let lenses = vec![
        lens(WIDE, LensRole::UltraWide, 0.59, PreviewMirror::Horizontal),
        lens(MAIN, LensRole::Wide, 1.0, PreviewMirror::None),
    ];
    let stops = lenses
        .iter()
        .map(|lens| ZoomStop {
            ratio: lens.native_ratio,
            kind: ZoomStopKind::Lens(lens.role),
        })
        .collect();
    Arc::new(ZoomCatalog {
        back: Some(LensGroup {
            facing: LensFacing::Back,
            lenses,
            stops,
            range: ZoomRange {
                min: ZoomRatio::new(0.59).expect("коэффициент"),
                max: ZoomRatio::new(10.0).expect("коэффициент"),
            },
        }),
        front: None,
    })
}

fn zoom_command(ratio: f32, catalog: &Arc<ZoomCatalog>) -> ViewfinderCommand {
    ViewfinderCommand::SetFraming(ViewfinderFraming {
        crop: FramingCrop::Zoom(ZoomFraming {
            aspect: CropAspect::SensorNative,
            facing: LensFacing::Back,
            ratio: ZoomRatio::new(ratio).expect("коэффициент"),
            catalog: Arc::clone(catalog),
        }),
        destination: FramingDestination::Configured,
    })
}

/// Счётчики GPU последнего прохода и два эталона CPU по ПРОЧИТАННОЙ картинке видоискателя этого же кадра.
struct Measured {
    gpu: ScopeCounters,
    as_stored: ScopeCounters,
    mirrored: ScopeCounters,
}

fn measure(rig: &Rig) -> Measured {
    rig.backend.context().wait_idle().expect("простой GPU");
    let gpu = rig
        .backend
        .read_scope_counters()
        .expect("чтение счётчиков")
        .expect("проход записан");
    let image = rig
        .backend
        .read_viewfinder_image()
        .expect("чтение изображения")
        .expect("изображение есть");
    let model = small_model();
    let rule = ResolvedScope::resolve(&waveform(), &model, isp_model::image::DisplayGamut::Srgb).expect("правило");
    let reference = |order| {
        let shown = ShownCodes::from_display_rgba(image.width, image.height, &image.pixels, order).expect("коды");
        accumulate(
            &shown,
            rule.planned_samples(shown.width(), shown.height(), &model).steps(),
            &rule as &dyn ScopeRule,
        )
        .expect("эталон")
    };
    Measured {
        gpu,
        as_stored: reference(HorizontalOrder::AsStored),
        mirrored: reference(HorizontalOrder::Mirrored),
    }
}

/// Зеркальный объектив: прибор читает то же окно, что компоновщик, то есть зеркальное; без зеркала — как хранится.
///
/// Что проведено: несимметричная сцена (градиент по x и y), основная камера (контроль, без зеркала) → показ;
/// затем поток B с зеркальным объективом → показ. Счётчики GPU сверяются с эталоном CPU по прочитанной картинке.
#[test]
fn the_scope_follows_the_mirror_of_the_shown_window_on_an_asymmetric_scene() {
    let scopes = ScopesConfig {
        // Каждый кадр: оба показа сверяются с эталоном своего кадра (каденция — отдельные тесты).
        every_nth_frame: 1,
        model: small_model(),
        ..ScopesConfig::default()
    };
    let Some(mut rig) = Rig::with_stills_and_scopes(test_continuity(RevealExposureCheck::Ignore), false, None, scopes)
    else {
        return;
    };
    rig.asymmetric.store(true, Ordering::Relaxed);
    let catalog = catalog();
    assert_eq!(
        rig.backend.apply_viewfinder_command(zoom_command(1.0, &catalog)),
        ViewfinderCommandOutcome::Applied
    );
    show_scopes(&mut rig);

    // Контроль: основная камера, зеркала нет.
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    let plain = measure(&rig);
    assert_ne!(
        plain.as_stored, plain.mirrored,
        "сцена несимметрична: эталоны различаются, тест умеет отличить зеркало"
    );
    assert_eq!(
        plain.gpu, plain.as_stored,
        "без зеркала прибор считает кадр как хранится"
    );

    // Зеркальный объектив B.
    rig.stream.producer().end_stream(epoch_a);
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    let epoch_b = rig.begin(&passport_b);
    rig.feed_until_live(epoch_b, gray());
    let mirrored = measure(&rig);
    assert_ne!(
        mirrored.as_stored, mirrored.mirrored,
        "сцена несимметрична: эталоны различаются"
    );
    assert_eq!(
        mirrored.gpu, mirrored.mirrored,
        "зеркальное окно: прибор считает зеркально, как показано на экране"
    );
    assert_ne!(mirrored.gpu, mirrored.as_stored, "и не «как хранится»");
    finish(&mut rig, &[epoch_b]);
}
