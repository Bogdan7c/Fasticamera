//! Миниатюра снимка на настоящем бэкенде (RADV, headless, полная validation; сессия 14g): снимок доходит до
//! `PrimaryReadback`, а миниатюра, пришедшая ДО итога, равна эталону уменьшения от прочитанной картинки этого же
//! снимка (с поворотом «как покажет галерея»); два снимка с разным содержимым дают две разные миниатюры со своими
//! номерами. Стенд — `still_rig`.

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

use std::sync::Arc;

use continuity_rig::{SIZE_A, finish};
use isp_vulkan::photo::{PhotoConfig, reference_thumbnail};
use render_loop::backend::DisplayBackend;
use render_loop::config::StillDevelopConfig;
use render_loop::still::{StillCommand, StillCommandOutcome};
use still_contract::develop::{DevelopEvent, PrimaryTarget, PrimaryTargetOffer};
use still_contract::plan::StillId;
use still_contract::shot::StillRotation;
use still_rig::{JPEG, Sink, StillBench, plan, press_during, setup};

/// Длинная сторона миниатюры в тесте: 64×48 с поворотом 90° → 48×64 → миниатюра 12×16 (уменьшение 4:1).
const LONG_SIDE: u32 = 16;
/// Допуск значений, градации sRGB: веса одинаковы, расходится только `pow` видеокарты (см. `photo_thumbnail.rs`).
const TOLERANCE_LEVELS: u8 = 1;

fn offer_readback(bench: &mut StillBench, id: u64) {
    let outcome = bench
        .rig
        .backend
        .apply_still_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(id),
            target: PrimaryTarget::Readback,
        }));
    assert!(matches!(outcome, StillCommandOutcome::Accepted), "{outcome:?}");
}

fn config() -> StillDevelopConfig {
    StillDevelopConfig {
        photo: PhotoConfig {
            slice_rows: 16,
            thumbnail_long_side_px: LONG_SIDE,
            ..PhotoConfig::default()
        },
        ..StillDevelopConfig::default()
    }
}

/// Снимок №`id`, нажатый на показе `shown`, доведённый до `PrimaryReadback`.
fn shoot(bench: &mut StillBench, id: u64, shown: usize) -> Arc<Sink> {
    let sink = bench.begin(plan(id, press_during(shown), JPEG));
    assert!(bench.frames_until(30, || sink.has_secured()), "Secured не пришёл");
    offer_readback(bench, id);
    assert!(bench.frames_until(60, || sink.finals() == 1), "итога нет");
    sink
}

fn readback_of(sink: &Sink) -> still_contract::shot::Rgba8Image {
    match sink.events().last() {
        Some(DevelopEvent::PrimaryReadback { image, .. }) => image.clone(),
        other => panic!("итог должен быть PrimaryReadback: {other:?}"),
    }
}

#[test]
fn each_still_gets_its_own_rotated_thumbnail_matching_the_reference_of_its_picture() {
    let Some(mut bench) = setup(config(), 12) else {
        return;
    };
    bench.frames(10);
    bench.inject_timings();

    let first = shoot(&mut bench, 1, 2);
    let second = shoot(&mut bench, 2, 5);

    let mut seen = Vec::new();
    for (id, sink) in [(1_u64, &first), (2, &second)] {
        let thumbnails = sink.thumbnails();
        assert_eq!(thumbnails.len(), 1, "снимок {id}: ровно одна миниатюра");
        assert_eq!(
            thumbnails[0].id(),
            StillId(id),
            "миниатюра помечена номером своего снимка"
        );
        assert_eq!(
            sink.thumbnails_before_final(),
            [1],
            "снимок {id}: к моменту итога миниатюра уже у приёмника"
        );

        // Эталон от картинки, которую получила запись (портрет на сенсоре 90°: поворот по часовой на 90°).
        let picture = readback_of(sink);
        let (want_size, want) =
            reference_thumbnail(picture.rgba_bytes(), SIZE_A, StillRotation::Clockwise90, LONG_SIDE).expect("эталон");
        assert_eq!(thumbnails[0].size(), want_size, "размер повёрнутой миниатюры");
        assert_eq!(
            (want_size.width, want_size.height),
            (12, 16),
            "портрет из ландшафтного кадра"
        );
        let worst = thumbnails[0]
            .rgba_bytes()
            .iter()
            .zip(&want)
            .map(|(got, want)| got.abs_diff(*want))
            .max()
            .expect("пиксели");
        assert!(
            worst <= TOLERANCE_LEVELS,
            "снимок {id}: миниатюра расходится с эталоном на {worst}"
        );
        seen.push(thumbnails[0].rgba_bytes().to_vec());
    }
    assert_ne!(seen[0], seen[1], "разное содержимое кадров — разные миниатюры");

    let still = bench.rig.backend.still_diagnostics().expect("снимки включены");
    assert_eq!(still.outcomes.readback, 2);
    assert_eq!((still.thumbnails_delivered, still.thumbnails_unavailable), (2, 0));
    first.events().clear();
    second.events().clear();
    finish(&mut bench.rig, &[bench.epoch]);
    assert_eq!(
        bench.rig.backend.context().live_objects().total(),
        bench.rig.baseline.total(),
        "миниатюра не оставляет объектов GPU"
    );
}
