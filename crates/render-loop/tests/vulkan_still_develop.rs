//! Снимок без задержки затвора на настоящем бэкенде (RADV, headless, полная validation): кадр, который был на
//! экране в момент нажатия, копируется, проявляется полосами между кадрами видоискателя и приходит картинкой,
//! равной конвейеру `isp_vulkan::photo` бит-в-бит и эталону `isp-model` в пределах 1 LSB. Стенд — `still_rig`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod still_rig;
mod support;
mod vulkan_support;

use camera_contract::geometry::PixelSize;
use continuity_rig::{SIZE_A, finish, scene_neutral};
use isp_vulkan::photo::PhotoConfig;
use render_loop::backend::DisplayBackend;
use render_loop::config::StillDevelopConfig;
use shell_contract::orientation::DeviceOrientation;
use still_contract::develop::DevelopEvent;
use still_contract::frame_choice::{FrameChoice, NoFrameReason, SubstitutionReason};
use still_contract::shot::StillRotation;
use still_rig::{JPEG, JPEG_AND_DNG, offer_readback, plan, press_during, raw_bytes, setup, shown_at, shown_frame};

fn normalized(neutral: [f64; 3]) -> [f64; 3] {
    let maximum = neutral.iter().copied().fold(f64::MIN_POSITIVE, f64::max);
    neutral.map(|channel| channel / maximum)
}

#[test]
fn the_frame_on_screen_at_press_is_secured_and_developed_like_the_photo_pipeline() {
    // Полосы по 16 строк: проявление идёт в несколько кадров видоискателя.
    let config = StillDevelopConfig {
        photo: PhotoConfig {
            slice_rows: 16,
            ..PhotoConfig::default()
        },
        ..StillDevelopConfig::default()
    };
    let Some(mut bench) = setup(config, 12) else {
        return;
    };
    bench.frames(10);
    let known = bench.inject_timings();
    assert!(known >= 6, "показано кадров: {known}");

    // Нажатие приходится на показ №3 (на экране он с 30 мс, показ №4 — с 40 мс).
    let (shown_ts, frame_index) = shown_frame(&mut bench, 3);
    let sink = bench.begin(plan(1, press_during(3), JPEG_AND_DNG));
    let before = bench
        .rig
        .backend
        .diagnostics()
        .camera
        .expect("камера")
        .camera_frames_presented;
    let offered_before = bench.cells.len();
    assert!(bench.frames_until(30, || sink.has_secured()), "Secured не пришёл");

    let (facts, raw) = match sink.events().first() {
        Some(DevelopEvent::Secured { facts, raw }) => ((**facts).clone(), raw.clone()),
        other => panic!("первым событием должен быть Secured: {other:?}"),
    };
    assert_eq!(
        facts.sensor_timestamp.as_nanos(),
        shown_ts,
        "снят кадр, который стоял на экране"
    );
    assert_eq!(
        facts.choice,
        FrameChoice::ShownAtPress {
            sensor_timestamp: facts.sensor_timestamp,
            shown_at: shown_at(3)
        }
    );
    assert_eq!(
        (facts.crop.left, facts.crop.top, facts.crop.size),
        (0, 0, SIZE_A),
        "весь кадр при зуме 1×"
    );
    assert_eq!(
        facts.rotation,
        StillRotation::Clockwise90,
        "сенсор 90°, портрет, задняя"
    );
    assert!(facts.digital_gain.0.abs() < 1e-6, "k вендорского AE = 1");
    let expected_neutral = normalized(scene_neutral());
    for (got, want) in facts.neutral.0.iter().zip(expected_neutral) {
        assert!(
            (got - want).abs() < 1e-9,
            "нейтраль {:?} против {expected_neutral:?}",
            facts.neutral
        );
    }
    // Сырые отсчёты полезной области — бит-в-бит исходные отсчёты ЭТОГО кадра.
    let raw = raw.expect("DNG запрошен");
    assert_eq!(raw.size(), SIZE_A);
    assert_eq!(
        raw.le_bytes(),
        raw_bytes(still_rig::cell(frame_index), SIZE_A).as_slice()
    );

    // Цель: память (JPEG). Видоискатель при этом показывает каждый кадр.
    offer_readback(&mut bench, 1);
    let mut ticks_during = 0_u64;
    for _ in 0..60 {
        if sink.finals() == 1 {
            break;
        }
        bench.frame();
        ticks_during += 1;
    }
    assert_eq!(sink.finals(), 1);
    let image = match sink.events().last() {
        Some(DevelopEvent::PrimaryReadback { image, .. }) => image.clone(),
        other => panic!("итог должен быть PrimaryReadback: {other:?}"),
    };
    assert!(
        ticks_during >= 3,
        "проявление шло по полосам между кадрами: {ticks_during} кадров"
    );
    let diagnostics = bench.rig.backend.diagnostics();
    let camera = diagnostics.camera.expect("камера");
    assert_eq!(camera.camera_frame_failures, 0);
    assert_eq!(
        camera.camera_frames_presented - before,
        u64::try_from(bench.cells.len() - offered_before).expect("кадры"),
        "видоискатель показал каждый кадр, пока снимок проявлялся"
    );

    // Картинка равна конвейеру photo (бит-в-бит) и эталону isp-model (≤ 1 LSB).
    let (params, _) = still_rig::expected_params(&bench.passport);
    let context = bench.rig.backend.context().clone();
    let cell = still_rig::cell(frame_index);
    let photo = still_rig::photo_pipeline_rgba(&context, &params, cell, SIZE_A, &config.photo);
    assert_eq!(image.size(), SIZE_A);
    assert_eq!(image.rgba_bytes(), photo.as_slice(), "рендер = photo бит-в-бит");
    let reference = still_rig::reference_rgba(&params, cell, SIZE_A);
    let worst = image
        .rgba_bytes()
        .iter()
        .zip(&reference)
        .map(|(got, want)| got.abs_diff(*want))
        .max()
        .expect("пиксели");
    assert!(worst <= 1, "отклонение от эталона isp-model {worst} LSB");

    let still = bench.rig.backend.still_diagnostics().expect("снимки включены");
    assert_eq!(still.picks.exact, 1);
    assert_eq!(still.params.own, 1);
    assert_eq!(still.outcomes.readback, 1);
    assert!(still.slices_submitted >= 3, "полос: {}", still.slices_submitted);
    drop(image);
    drop(raw);
    // События держат буферы GPU (сырые отсчёты и картинку) — как их держала бы запись; отпускаем перед проверкой.
    sink.events().clear();
    finish(&mut bench.rig, &[bench.epoch]);
    assert_eq!(
        bench.rig.backend.context().live_objects().total(),
        bench.rig.baseline.total(),
        "после отсоединения объектов GPU снимков не остаётся"
    );
}

#[test]
fn ten_presses_in_a_row_give_ten_pictures_and_the_viewfinder_never_skips_a_frame() {
    let Some(mut bench) = setup(StillDevelopConfig::default(), 12) else {
        return;
    };
    bench.frames(12);
    bench.inject_timings();
    let presented_before = bench
        .rig
        .backend
        .diagnostics()
        .camera
        .expect("камера")
        .camera_frames_presented;
    let frames_before = bench.cells.len();
    let sinks: Vec<_> = (0..10_u64)
        .map(|index| {
            let shown = 2 + usize::try_from(index % 6).expect("номер");
            bench.begin(plan(index + 1, press_during(shown), JPEG))
        })
        .collect();
    let mut offered = [false; 10];
    for _ in 0..400 {
        bench.frame();
        for (index, sink) in sinks.iter().enumerate() {
            if sink.has_secured() && !offered[index] {
                offered[index] = true;
                offer_readback(&mut bench, index as u64 + 1);
            }
        }
        if sinks.iter().all(|sink| sink.finals() == 1) {
            break;
        }
    }
    for (index, sink) in sinks.iter().enumerate() {
        assert_eq!(sink.finals(), 1, "снимок {}: ровно один итог", index + 1);
        assert!(
            matches!(sink.events().last(), Some(DevelopEvent::PrimaryReadback { .. })),
            "снимок {}: картинка",
            index + 1
        );
    }
    let still = bench.rig.backend.still_diagnostics().expect("снимки включены");
    assert_eq!(still.outcomes.total(), 10);
    assert_eq!(still.outcomes.readback, 10);
    let camera = bench.rig.backend.diagnostics().camera.expect("камера");
    let frames_offered = u64::try_from(bench.cells.len() - frames_before).expect("кадры");
    assert_eq!(
        camera.camera_frames_presented - presented_before,
        frames_offered,
        "ни одного кадра видоискателя не потеряно из-за снимков"
    );
    finish(&mut bench.rig, &[bench.epoch]);
}

#[test]
fn without_timing_records_the_press_is_answered_by_an_estimate_and_says_so() {
    let config = StillDevelopConfig {
        presentation_wait_ms: 1,
        ..StillDevelopConfig::default()
    };
    let Some(mut bench) = setup(config, 12) else {
        return;
    };
    bench.frames(8);
    // Записей показа нет вовсе (как на ПК): после короткого срока ожидания — кадр по оценке.
    let sink = bench.begin(plan(1, shown_at(3), JPEG));
    std::thread::sleep(std::time::Duration::from_millis(10));
    assert!(bench.frames_until(20, || sink.has_secured()));
    match sink.events().first() {
        Some(DevelopEvent::Secured { facts, .. }) => {
            assert!(
                matches!(
                    facts.choice,
                    FrameChoice::Substituted {
                        reason: SubstitutionReason::PresentationTimingUnknown,
                        ..
                    }
                ),
                "{:?}",
                facts.choice
            );
            assert!(!facts.choice.is_exact());
        }
        other => panic!("{other:?}"),
    }
    let still = bench.rig.backend.still_diagnostics().expect("снимки");
    assert_eq!((still.picks.exact, still.picks.substituted_timing_unknown), (0, 1));
    assert_eq!(still.presentation.wait_expired, 1);
    offer_readback(&mut bench, 1);
    assert!(bench.frames_until(60, || sink.finals() == 1));
    finish(&mut bench.rig, &[bench.epoch]);
}

#[test]
fn a_disabled_ring_gives_not_secured_ring_disabled_and_the_picture_goes_on() {
    let Some(mut bench) = setup(StillDevelopConfig::default(), 0) else {
        return;
    };
    bench.frames(6);
    bench.inject_timings();
    let sink = bench.begin(plan(1, press_during(2), JPEG));
    assert!(matches!(
        sink.events().as_slice(),
        [DevelopEvent::NotSecured {
            reason: NoFrameReason::RingDisabled,
            ..
        }]
    ));
    bench.frames(3);
    assert_eq!(
        bench
            .rig
            .backend
            .still_diagnostics()
            .expect("снимки")
            .outcomes
            .not_secured,
        1
    );
    finish(&mut bench.rig, &[bench.epoch]);
}

#[test]
fn rotation_follows_the_phone_orientation_in_the_whole_pipeline() {
    let Some(mut bench) = setup(StillDevelopConfig::default(), 12) else {
        return;
    };
    bench.frames(8);
    bench.inject_timings();
    let table = [
        (DeviceOrientation::Portrait, StillRotation::Clockwise90),
        (DeviceOrientation::LandscapeRight, StillRotation::Clockwise180),
        (DeviceOrientation::PortraitUpsideDown, StillRotation::Clockwise270),
        (DeviceOrientation::LandscapeLeft, StillRotation::None),
    ];
    for (index, (orientation, expected)) in table.into_iter().enumerate() {
        let mut still = plan(index as u64 + 1, press_during(2), JPEG);
        still.orientation = orientation;
        let sink = bench.begin(still);
        assert!(bench.frames_until(30, || sink.has_secured()));
        match sink.events().first() {
            Some(DevelopEvent::Secured { facts, .. }) => assert_eq!(facts.rotation, expected, "{orientation:?}"),
            other => panic!("{other:?}"),
        }
        offer_readback(&mut bench, index as u64 + 1);
        assert!(bench.frames_until(60, || sink.finals() == 1));
    }
    finish(&mut bench.rig, &[bench.epoch]);
    let _ = PixelSize::new(1, 1);
}
