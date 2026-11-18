//! Наш AE на настоящем драйвере (RADV, полная validation): замкнутый контур «сцена → статистика → замер →
//! предложение → камера → сцена».
//!
//! Тест играет роль камеры и движка. Яркость мозаики кадра пропорциональна «выдержка × ISO» ЭТОГО кадра (серая
//! плашка на весь кадр, цвет белой плитки лампы 2800 К из независимой фикстуры), поэтому экспозиция честно
//! влияет на картинку. Камера отвечает как настоящая: читает предложение из ящика, публикует указание с
//! `applied`, а на кадрах новая экспозиция появляется только через [`SENSOR_DELAY_FRAMES`] кадров.
//!
//! Доказывается:
//! * контур сходится: серая плашка на экране встаёт в экранный средний серый (`display_middle_gray`) в допуске,
//!   хотя старт на 2 EV темнее нужного;
//! * остаточная подтяжка работает: на кадрах, пока сенсор догоняет, плашка ближе к цели, чем без подтяжки;
//! * блокировка держит цель: после «Holding» сцена светлеет на 1 EV, и плашка светлеет вместе с ней (экспозиция
//!   не догоняет); без блокировки та же смена сцены возвращает плашку к цели;
//! * кадр без метаданных не ломает поток и показывается без подтяжки;
//! * кадр серии AE не видит; без ящиков AE рендер работает как прежде (подтяжки и диагностики нет).

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

use ae_rig::{
    AeRig, BRACKET_TS_BASE, FRAME_NANOS, SENSOR_DELAY_FRAMES, Shot, converged_rig, display_middle_gray, ev_error,
    rig_or_skip, tone_display,
};
use camera_contract::auto_exposure::ProposalConvergence;
use camera_contract::three_a::{AeAfLock, BurstId, RequestOrigin};
use render_loop::config::AutoExposureConfig;

// ---- сходимость ----

#[test]
fn the_loop_brings_the_gray_card_to_the_display_middle_gray_from_two_stops_under() {
    let Some((mut rig, journal)) = rig_or_skip(Some(AutoExposureConfig::default())) else {
        return;
    };
    rig.run(80);
    let first = rig
        .history
        .iter()
        .find_map(|shot| shot.display_luminance)
        .expect("кадры показаны");
    let wanted = display_middle_gray();
    assert!(
        ev_error(first, wanted) < -1.0,
        "старт заметно темнее цели: {first} против {wanted}"
    );
    let settled = rig.last_luminance();
    assert!(
        ev_error(settled, wanted).abs() < 0.12,
        "плашка встала в средний серый: {settled} против {wanted}"
    );
    let diagnostics = rig.viewfinder().auto_exposure.expect("AE включён");
    assert_eq!(
        diagnostics.convergence,
        Some(ProposalConvergence::Settled),
        "{diagnostics:?}"
    );
    assert!(diagnostics.counters.proposals_published >= 1);
    assert!(diagnostics.counters.accepted >= 5);
    assert_eq!(diagnostics.counters.skipped_no_paired_exposure, 0);
    assert!(
        diagnostics.display_gain_ev.abs() < 0.05,
        "сенсор догнал: {diagnostics:?}"
    );
    // Предложение приходит к движку в разумном объёме, а не на каждом кадре.
    assert!(diagnostics.counters.proposals_published < 25, "{diagnostics:?}");
    rig.finish(&journal);
}

#[test]
fn the_scene_is_calibrated_so_the_target_is_near_one_second_iso() {
    let Some((rig, journal)) = converged_rig() else {
        return;
    };
    let target = rig.proposals.latest().expect("предложение").1.target.seconds_iso();
    assert!(
        (0.7..1.4).contains(&target),
        "масштаб сцены подобран на цель ≈ 1 с·ISO, а получилось {target} (старт 0,25 — это ≈ 2 EV ниже)"
    );
    rig.finish(&journal);
}

// ---- подтяжка ----

/// Яркость плашки на кадрах окна «предложение уже в запросе, сенсор ещё не догнал».
fn catch_up_window(rig: &AeRig) -> (i64, Vec<Shot>) {
    let applied_at = rig.first_applied_at.expect("предложение было");
    let window = rig
        .history
        .iter()
        .filter(|shot| shot.frame > applied_at && shot.frame < applied_at + SENSOR_DELAY_FRAMES)
        .copied()
        .collect();
    (applied_at, window)
}

#[test]
fn residual_gain_keeps_the_picture_closer_to_the_target_while_the_sensor_catches_up() {
    let with_gain = AutoExposureConfig::default();
    let mut without_gain = AutoExposureConfig::default();
    without_gain.controller.max_residual_ev = 0.0;
    let Some((mut pulled, pulled_journal)) = rig_or_skip(Some(with_gain)) else {
        return;
    };
    let Some((mut plain, plain_journal)) = rig_or_skip(Some(without_gain)) else {
        return;
    };
    pulled.run(40);
    plain.run(40);
    let (pulled_at, pulled_window) = catch_up_window(&pulled);
    let (plain_at, plain_window) = catch_up_window(&plain);
    assert_eq!(pulled_at, plain_at, "контуры идут одинаково до подтяжки");
    assert_eq!(pulled_window.len(), (SENSOR_DELAY_FRAMES - 1) as usize);
    let wanted = display_middle_gray();
    for (with, without) in pulled_window.iter().zip(&plain_window) {
        assert_eq!(with.sensor_product, without.sensor_product, "сенсор на обоих одинаков");
        let (with, without) = (
            with.display_luminance.expect("кадр показан"),
            without.display_luminance.expect("кадр показан"),
        );
        assert!(
            with > without * 1.15 && ev_error(with, wanted).abs() < ev_error(without, wanted).abs() - 0.2,
            "с подтяжкой плашка ближе к цели: {with} против {without} (цель {wanted})"
        );
    }
    let diagnostics = pulled.viewfinder().auto_exposure.expect("AE");
    assert!(diagnostics.counters.frames_with_residual_gain >= 5, "{diagnostics:?}");
    let plain_diagnostics = plain.viewfinder().auto_exposure.expect("AE");
    assert_eq!(plain_diagnostics.counters.frames_with_residual_gain, 0);
    // Подтяжка не копит пересчётов параметров вне окна: когда сенсор догнал, `k` = 1 и кэш не пересчитывается.
    let counters = pulled.viewfinder().counters;
    assert!(counters.params_unchanged > 20, "{counters:?}");
    pulled.finish(&pulled_journal);
    plain.finish(&plain_journal);
}

// ---- общее расписание статистики ----

#[test]
fn ae_cold_start_gets_statistics_on_consecutive_frames_while_awb_keeps_its_own_schedule() {
    let Some((mut with_ae, with_journal)) = rig_or_skip(Some(AutoExposureConfig::default())) else {
        return;
    };
    let Some((mut without_ae, without_journal)) = rig_or_skip(None) else {
        return;
    };
    with_ae.run(8);
    without_ae.run(8);
    let with = with_ae.viewfinder();
    let without = without_ae.viewfinder();
    // Без AE статистика идёт по расписанию AWB (каждый 4-й кадр); с AE на холодном старте — подряд.
    assert!(
        with.counters.statistics_published > without.counters.statistics_published,
        "AE на холодном старте получил свои кадры: {} против {}",
        with.counters.statistics_published,
        without.counters.statistics_published
    );
    // AWB не получил лишних кадров: его запросы и оценки те же, что без AE.
    assert_eq!(
        with.white_balance.statistics_requested,
        without.white_balance.statistics_requested
    );
    assert_eq!(with.white_balance.estimates, without.white_balance.estimates);
    let ae = with.auto_exposure.expect("AE");
    assert!(ae.counters.cold_start_statistics_frames >= 2, "{ae:?}");
    assert!(ae.counters.accepted >= 2, "замеры приняты: {ae:?}");
    with_ae.finish(&with_journal);
    without_ae.finish(&without_journal);
}

// ---- блокировка ----

#[test]
fn a_locked_target_does_not_follow_a_brighter_scene_while_an_unlocked_one_does() {
    let Some((mut locked, locked_journal)) = converged_rig() else {
        return;
    };
    let Some((mut free, free_journal)) = converged_rig() else {
        return;
    };
    let wanted = display_middle_gray();
    assert!(ev_error(locked.last_luminance(), wanted).abs() < 0.12);

    locked.set_lock(AeAfLock::Locked);
    let mut frames = 0;
    while locked.viewfinder().auto_exposure.and_then(|ae| ae.convergence) != Some(ProposalConvergence::Holding) {
        locked.run(1);
        frames += 1;
        assert!(frames < 120, "блокировка так и не встала в Holding");
    }
    locked.run(20);
    let before = locked.last_luminance();

    // Сцена светлеет на 1 EV.
    locked.light = 2.0;
    free.light = 2.0;
    locked.run(60);
    free.run(80);

    // Цель заморожена: экспозиция не меняется, плашка светлеет на ту же ступень, что даёт тон.
    let after_lock = locked.last_luminance();
    let expected_after = tone_display(0.075 * 2.0);
    assert!(
        ev_error(after_lock, expected_after).abs() < 0.15,
        "под блокировкой плашка светлеет на 1 EV сцены: {after_lock} (ждали {expected_after}, было {before})"
    );
    assert!(
        after_lock > before * 1.5,
        "заметно светлее: {after_lock} против {before}"
    );
    // Без блокировки контур возвращает плашку к цели.
    let after_free = free.last_luminance();
    assert!(
        ev_error(after_free, wanted).abs() < 0.15,
        "без блокировки AE снова ставит плашку в средний серый: {after_free} против {wanted}"
    );
    locked.finish(&locked_journal);
    free.finish(&free_journal);
}

// ---- кадры без метаданных и кадры серии ----

#[test]
fn a_frame_without_metadata_is_shown_without_gain_and_does_not_break_the_loop() {
    let Some((mut rig, journal)) = rig_or_skip(Some(AutoExposureConfig::default())) else {
        return;
    };
    // Идём до окна догонки: на кадрах окна подтяжка не единичная.
    while rig.first_applied_at.is_none() {
        rig.step();
        assert!(rig.frame < 60, "предложения нет");
    }
    rig.step();
    let gain_before = rig.viewfinder().auto_exposure.expect("AE").display_gain_ev;
    assert!(gain_before > 0.1, "в окне догонки подтяжка есть: {gain_before}");

    // Восемь кадров подряд без результата (пара не придёт): статистика считается каждый 4-й кадр, значит
    // хотя бы два таких кадра дойдут до замера и будут учтены.
    for _ in 0..8 {
        let shot = rig.step_with(false);
        assert!(shot.display_luminance.is_some(), "кадр без метаданных показан");
        let gain = rig.viewfinder().auto_exposure.expect("AE").display_gain_ev;
        assert_eq!(gain, 0.0, "выдержку кадра не знаем: подтяжки нет");
    }
    rig.run(60);
    let after = rig.viewfinder().auto_exposure.expect("AE");
    assert!(
        after.counters.skipped_no_paired_exposure >= 2,
        "кадры без пары учтены: {after:?}"
    );
    assert!(
        ev_error(rig.last_luminance(), display_middle_gray()).abs() < 0.12,
        "контур дальше сходится"
    );
    assert_eq!(rig.stream.counters().frames_missing_meta_timeout, 8);
    rig.finish(&journal);
}

#[test]
fn bracket_frames_are_invisible_to_the_exposure_loop() {
    let Some((mut rig, journal)) = converged_rig() else {
        return;
    };
    // Тики без новых кадров: слоты кольца, в которых ещё лежат обычные кадры, успевают собраться.
    for _ in 0..4 {
        rig.render();
    }
    let before = rig.viewfinder();
    let before_ae = before.auto_exposure.expect("AE");
    let generation_before = rig.proposals.latest().expect("предложение").0;
    for index in 0..3u32 {
        let sensor_ts = BRACKET_TS_BASE + i64::from(index) * FRAME_NANOS;
        // Кадры серии в 8 раз темнее при том же заявленном произведении: если бы AE их измерил, он потребовал бы
        // на 3 EV больше света и счётчик принятых замеров вырос бы.
        rig.insert_scene(sensor_ts, rig.sensor_product / 8.0);
        let result = rig.result_with(
            sensor_ts,
            rig.sensor_product,
            RequestOrigin::Burst {
                burst: BurstId(1),
                index,
            },
        );
        rig.offer(sensor_ts, Some(result));
        assert!(!rig.render(), "кадр серии не показан");
    }
    for _ in 0..6 {
        rig.render();
    }
    let after = rig.viewfinder();
    assert_eq!(after.counters.bracket_frames_skipped, 3);
    assert_eq!(
        after.counters.frames_processed, before.counters.frames_processed,
        "ISP кадры серии не обрабатывал"
    );
    assert_eq!(
        after.auto_exposure.expect("AE"),
        before_ae,
        "AE не видел кадры серии: ни замеров, ни пропусков, ни сдвига цели"
    );
    assert_eq!(rig.proposals.latest().expect("предложение").0, generation_before);
    rig.finish(&journal);
}

// ---- AE выключен ----

#[test]
fn without_auto_exposure_links_the_render_works_as_before() {
    let Some((mut rig, journal)) = rig_or_skip(None) else {
        return;
    };
    rig.run(30);
    let diagnostics = rig.viewfinder();
    assert!(diagnostics.auto_exposure.is_none(), "AE выключен: диагностики нет");
    assert!(rig.proposals.latest().is_none(), "предложений нет");
    assert!(
        rig.history.iter().any(|shot| shot.display_luminance.is_some()),
        "картинка показывается"
    );
    // Экспозиция не менялась: плашка осталась тёмной (никто её не вытягивал).
    let wanted = display_middle_gray();
    assert!(ev_error(rig.last_luminance(), wanted) < -1.0);
    rig.finish(&journal);
}
