//! Слежение за сценой под точкой тапа на настоящем драйвере (решение 241): реальные пиксели → шейдер статистики →
//! детектор → наблюдение «точка устарела» в ящике движка.
//!
//! Тест играет камеру и движок (стенд `ae_rig`): яркость кадра пропорциональна «выдержка × ISO» ЭТОГО кадра, движок
//! читает предложения и ставит экспозицию с задержкой конвейера сенсора. Доказывается:
//! * тап по тёмному месту, затем сцена сменилась (тёмное место стало светлым, светлое — тёмным) — наблюдение
//!   опубликовано с номером тапа и причиной «сцена сменилась»;
//! * сцена прежняя под тем же тапом, пока AE сходится (экспозиция ушла на +2 EV за эталоном) — не опубликовано:
//!   подпись не зависит от выдержки кадра;
//! * блокировка AE/AF — молчание при той же смене сцены.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod ae_rig;
mod awb_support;
mod support;
mod vulkan_support;
mod white_engine;

use ae_rig::{
    AeRig, DARK_START_PRODUCT, RigSetup, RigStream, SCENE_SCALE, SceneLayout, TILE_COUNT, manual_lamp_white, rig_with,
};
use camera_contract::scene_change::PointObsoleteCause;
use camera_contract::three_a::{AeAfLock, MeteringRegion, PointRequestId, SensorPoint};
use render_loop::config::AutoExposureConfig;
use std::sync::Arc;
use support::Journal;

/// Левая половина кадра темнее правой в 8 раз (1/8 против 1): при экспозиции по тёмной половине светлая даёт
/// 0,6 белого — без пересвета.
fn left_dark_right_bright() -> SceneLayout {
    SceneLayout::PerTile(
        (0..TILE_COUNT)
            .map(|index| if index % 32 < 16 { 0.125 } else { 1.0 })
            .collect(),
    )
}

/// То же, но наоборот: тёмным стало то место, где был свет, и светлым — где было тёмное.
fn left_bright_right_dark() -> SceneLayout {
    SceneLayout::PerTile(
        (0..TILE_COUNT)
            .map(|index| if index % 32 < 16 { 1.0 } else { 0.125 })
            .collect(),
    )
}

fn rig_with_layout(layout: SceneLayout, start_product: f64) -> Option<(AeRig, Arc<Journal>)> {
    rig_with(RigSetup {
        auto_exposure: Some(AutoExposureConfig::default()),
        white_balance: Some(manual_lamp_white()),
        scene: "checker_bb2800",
        start_product,
        scene_scale: SCENE_SCALE,
        allow_clipping: false,
        layout,
        stream: RigStream::Reference,
    })
}

/// Тап по центру левой половины кадра.
fn tap_on_the_left(rig: &mut AeRig, request: u64) {
    let region = MeteringRegion::new(SensorPoint::new(0.25, 0.5).expect("точка"), 0.1).expect("регион");
    rig.tap(region, PointRequestId(request));
}

#[test]
fn a_tap_on_a_dark_place_is_reported_obsolete_after_the_scene_changes_under_it() {
    // Экспозиция по тёмному месту: старт уже около цели, чтобы замер по точке не пересвечивал светлую половину.
    let Some((mut rig, journal)) = rig_with_layout(left_dark_right_bright(), 4.0) else {
        return;
    };
    tap_on_the_left(&mut rig, 3);
    rig.run(100);
    let before = rig.viewfinder().auto_exposure.expect("AE включён");
    assert!(before.counters.scene_references_taken >= 1, "эталон снят: {before:?}");
    assert!(
        rig.proposals.point_obsolete_newer_than(None).is_none(),
        "сцена прежняя — наблюдения нет"
    );

    // Сцена сменилась: на месте тапа теперь светлое, справа — тёмное.
    rig.layout = left_bright_right_dark();
    rig.run(100);
    let (_, observation) = rig
        .proposals
        .point_obsolete_newer_than(None)
        .expect("смена сцены под точкой дошла до ящика движка");
    assert_eq!(observation.request, PointRequestId(3));
    assert_eq!(observation.cause, PointObsoleteCause::SceneChanged);
    let after = rig.viewfinder().auto_exposure.expect("AE включён");
    assert!(after.counters.scene_changes_confirmed >= 1, "{after:?}");
    assert!(after.counters.point_obsolete_published >= 1, "{after:?}");
    rig.finish(&journal);
}

#[test]
fn the_same_scene_under_the_same_tap_is_not_reported_while_the_exposure_converges() {
    // Старт на 2 EV темнее равномерной цели, а тап по тёмной половине просит ещё в 8 раз больше света: экспозиция
    // уйдёт вверх на ~5 EV, пока сцена не меняется. Эталон — первый же кадр тапа, то есть ДО того, как экспозиция
    // дошла до кадров (задержка конвейера 8 кадров): подпись обязана не зависеть от выдержки кадра.
    let Some((mut rig, journal)) = rig_with_layout(left_dark_right_bright(), DARK_START_PRODUCT) else {
        return;
    };
    tap_on_the_left(&mut rig, 4);
    rig.run(200);
    let first_product = rig.history.first().expect("кадры были").sensor_product;
    let last_product = rig.history.last().expect("кадры были").sensor_product;
    assert!(
        (last_product / first_product).log2() > 3.0,
        "экспозиция должна была заметно уйти вверх: {first_product} → {last_product}"
    );
    let diagnostics = rig.viewfinder().auto_exposure.expect("AE включён");
    assert!(diagnostics.counters.scene_references_taken >= 1, "{diagnostics:?}");
    assert_eq!(diagnostics.counters.scene_changes_confirmed, 0, "{diagnostics:?}");
    assert_eq!(diagnostics.counters.point_obsolete_published, 0, "{diagnostics:?}");
    assert!(rig.proposals.point_obsolete_newer_than(None).is_none());
    rig.finish(&journal);
}

#[test]
fn a_tap_followed_at_once_by_another_scene_is_reported_while_the_exposure_is_still_converging() {
    // Тапнул и сразу увёл камеру: эталон — первый кадр тапа (старт на 2 EV темнее), через пару кадров сцена другая,
    // а экспозиция ещё сходится.
    let Some((mut rig, journal)) = rig_with_layout(left_dark_right_bright(), DARK_START_PRODUCT) else {
        return;
    };
    tap_on_the_left(&mut rig, 6);
    rig.run(3);
    rig.layout = left_bright_right_dark();
    rig.run(100);
    let (_, observation) = rig
        .proposals
        .point_obsolete_newer_than(None)
        .expect("смена сцены сразу после тапа дошла до движка");
    assert_eq!(observation.request, PointRequestId(6));
    assert_eq!(observation.cause, PointObsoleteCause::SceneChanged);
    rig.finish(&journal);
}

#[test]
fn a_locked_tap_stays_silent_when_the_scene_changes() {
    let Some((mut rig, journal)) = rig_with_layout(left_dark_right_bright(), 4.0) else {
        return;
    };
    tap_on_the_left(&mut rig, 5);
    rig.run(100);
    rig.set_lock(AeAfLock::Locked);
    rig.layout = left_bright_right_dark();
    rig.run(120);
    let diagnostics = rig.viewfinder().auto_exposure.expect("AE включён");
    assert_eq!(diagnostics.counters.point_obsolete_published, 0, "{diagnostics:?}");
    assert!(rig.proposals.point_obsolete_newer_than(None).is_none());
    rig.finish(&journal);
}
