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
//! * кадр серии AE не видит; без ящиков AE рендер работает как прежде (подтяжки и диагностики нет);
//! * на контрастной сцене (стол под лампой в тёмной комнате) контур сходится к обычному среднему света, а не к
//!   лог-среднему (13c, решение 133).

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
    AeRig, BRACKET_TS_BASE, DARK_START_PRODUCT, FRAME_NANOS, RigSetup, RigStream, SCENE_SCALE, SENSOR_DELAY_FRAMES,
    SceneLayout, Shot, TILE_COUNT, converged_rig, display_middle_gray, ev_error, manual_lamp_white, rig_or_skip,
    rig_with, tone_display,
};
use camera_contract::auto_exposure::ProposalConvergence;
use camera_contract::three_a::{AeAfLock, BurstId, RequestOrigin};
use isp_model::statistics::TileGrid;
use render_loop::config::AutoExposureConfig;
use scene_estimation::exposure::{MeteringArea, MeteringConfig, MeteringWeights, TilePlacement};

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
fn ae_cold_start_gets_statistics_on_consecutive_frames_and_without_ae_none_are_computed() {
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
    // Без AE потребителей статистики нет: стадия не считается и не публикуется; с AE на холодном старте — подряд.
    assert_eq!(
        (without.counters.statistics_published, without.statistics),
        (0, None),
        "без AE статистика не нужна"
    );
    assert!(
        with.counters.statistics_published >= 2,
        "AE на холодном старте получил свои кадры: {}",
        with.counters.statistics_published
    );
    let schedule = with.statistics.expect("у AE есть расписание статистики");
    assert_eq!(schedule.every_n_frames, 4);
    assert_eq!(
        schedule.requested, with.counters.statistics_published,
        "каждый запрошенный кадр опубликован ровно один раз"
    );
    let ae = with.auto_exposure.expect("AE");
    assert!(ae.counters.cold_start_statistics_frames >= 2, "{ae:?}");
    assert!(ae.counters.accepted >= 2, "замеры приняты: {ae:?}");
    with_ae.finish(&with_journal);
    without_ae.finish(&without_journal);
}

#[test]
fn a_stream_whose_colour_calibration_gives_no_locus_table_still_gets_ae_measurements_in_the_steady_state() {
    // Расписание статистики от цвета потока не зависит: без таблицы локуса (оценщику света нечем работать) AE
    // обязан продолжать получать замеры и вести экспозицию. Раньше расписание жило в старом AWB и без таблицы
    // выдавало «пропустить» на каждом кадре, так что AE слеп оставался после холодного старта.
    let Some((mut rig, journal)) = rig_with(RigSetup {
        auto_exposure: Some(AutoExposureConfig::default()),
        white_balance: Some(manual_lamp_white()),
        scene: "checker_bb2800",
        start_product: DARK_START_PRODUCT,
        scene_scale: SCENE_SCALE,
        allow_clipping: false,
        layout: SceneLayout::Uniform,
        stream: RigStream::WithoutLocusTable,
    }) else {
        return;
    };
    rig.run(50);
    let midway = rig.viewfinder();
    rig.run(50);
    let diagnostics = rig.viewfinder();
    assert!(
        diagnostics.white_balance.table_failure.is_some(),
        "предусловие: таблицу локуса потока построить не удалось ({:?})",
        diagnostics.white_balance
    );
    let ae = diagnostics.auto_exposure.clone().expect("AE включён");
    let ae_midway = midway.auto_exposure.expect("AE включён");
    // Холодный старт кончился давно (кадры 0..50), дальше расписание идёт по периоду — и замеры продолжают приходить.
    let schedule = diagnostics.statistics.expect("у AE есть расписание статистики");
    let schedule_midway = midway.statistics.expect("у AE есть расписание статистики");
    assert!(
        schedule.requested >= schedule_midway.requested + 10,
        "в установившемся режиме статистика считается по периоду: {schedule_midway:?} -> {schedule:?}"
    );
    assert!(
        schedule.skipped_by_period > schedule_midway.skipped_by_period,
        "это именно период, а не «каждый кадр»: {schedule_midway:?} -> {schedule:?}"
    );
    assert!(
        ae.counters.accepted >= ae_midway.counters.accepted + 10,
        "замеры AE идут и после холодного старта: {ae_midway:?} -> {ae:?}"
    );
    // И экспозиция ведётся: контур вывел её из тёмного старта и держит (цель у этой калибровки другая, чем у
    // эталонной: веса яркости идут из её матриц, поэтому «серый на экране» здесь не проверяем).
    assert_eq!(ae.convergence, Some(ProposalConvergence::Settled), "{ae:?}");
    let start = rig.history[0].sensor_product;
    let tail: Vec<f64> = rig.history[rig.history.len() - 10..]
        .iter()
        .map(|shot| shot.sensor_product)
        .collect();
    assert!(
        (tail[9] / start).log2() > 1.0,
        "экспозиция ушла от тёмного старта к цели: {start} -> {}",
        tail[9]
    );
    assert!(
        tail.iter().all(|product| (product / tail[9] - 1.0).abs() < 0.01),
        "в установившемся режиме экспозиция стоит: {tail:?}"
    );
    rig.finish(&journal);
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

#[test]
fn flash_still_frames_are_never_shown_and_invisible_to_the_exposure_loop() {
    let Some((mut rig, journal)) = converged_rig() else {
        return;
    };
    for _ in 0..4 {
        rig.render();
    }
    let before = rig.viewfinder();
    let before_ae = before.auto_exposure.expect("AE");
    let generation_before = rig.proposals.latest().expect("предложение").0;
    for shot in 0..3u64 {
        let sensor_ts = BRACKET_TS_BASE + i64::try_from(shot).unwrap() * FRAME_NANOS;
        // Кадр вспышки в 8 раз светлее: если бы его показали или измерили AE, экран и цель сдвинулись бы.
        rig.insert_scene(sensor_ts, rig.sensor_product * 8.0);
        let result = rig.result_with_flash(
            sensor_ts,
            rig.sensor_product,
            RequestOrigin::FlashStill {
                shot: camera_contract::flash::FlashShotId(shot),
            },
            camera_contract::flash::FlashResult {
                mode: Some(camera_contract::flash::FlashModeEcho::Single),
                state: Some(camera_contract::flash::FlashState::Fired),
            },
        );
        rig.offer(sensor_ts, Some(result));
        assert!(!rig.render(), "кадр вспышки не показан");
    }
    for _ in 0..6 {
        rig.render();
    }
    let after = rig.viewfinder();
    assert_eq!(after.counters.flash_frames_skipped, 3);
    assert_eq!(
        after.counters.bracket_frames_skipped,
        before.counters.bracket_frames_skipped
    );
    assert_eq!(
        after.counters.frames_processed, before.counters.frames_processed,
        "ISP кадры вспышки не обрабатывал"
    );
    assert_eq!(
        after.auto_exposure.expect("AE"),
        before_ae,
        "AE кадров вспышки не видел"
    );
    assert_eq!(rig.proposals.latest().expect("предложение").0, generation_before);
    // Экран кадры вспышки не взял, но кольцо снимков их хранит и отдаёт ТОЛЬКО по метке запроса.
    let ring = rig.stream.snapshot_ring();
    for shot in 0..3u64 {
        match ring.pick_flash_frame(camera_contract::flash::FlashShotId(shot)) {
            raw_frame_stream::FlashRingPick::Found(lease) => {
                let expected = BRACKET_TS_BASE + i64::try_from(shot).unwrap() * FRAME_NANOS;
                assert_eq!(lease.passport().sensor_timestamp.as_nanos(), expected);
                lease.release_unused();
            }
            other => panic!("кадр вспышки {shot} должен лежать в кольце снимков: {other:?}"),
        }
    }
    rig.finish(&journal);
}

#[test]
fn torch_frames_are_shown_and_measured_for_the_flash_but_do_not_pull_the_viewfinder_exposure() {
    use camera_contract::flash::{FlashModeEcho, FlashResult, FlashState, PreflashLight};
    let Some((mut rig, journal)) = converged_rig() else {
        return;
    };
    for _ in 0..4 {
        rig.render();
    }
    let before_ae = rig.viewfinder().auto_exposure.expect("AE");
    let target_before = rig.proposals.latest().expect("предложение").1.target.seconds_iso();
    // Движок объявил предвспышку: рендер мерит каждый кадр, в том числе обычные до фонарика.
    rig.publish_active_preflash(1);
    let torch = FlashResult {
        mode: Some(FlashModeEcho::Torch),
        state: Some(FlashState::Fired),
    };
    let sensor_ts = |index: i64| BRACKET_TS_BASE + index * FRAME_NANOS;
    for index in 0..3 {
        rig.insert_scene(sensor_ts(index), rig.sensor_product);
        let result = rig.result_with(sensor_ts(index), rig.sensor_product, RequestOrigin::Repeating);
        rig.offer(sensor_ts(index), Some(result));
        rig.render();
    }
    for index in 3..6 {
        // Фонарик делает сцену в 8 раз ярче при том же произведении.
        rig.insert_scene(sensor_ts(index), rig.sensor_product * 8.0);
        let result = rig.result_with_flash(sensor_ts(index), rig.sensor_product, RequestOrigin::Repeating, torch);
        rig.offer(sensor_ts(index), Some(result));
        assert!(rig.render(), "кадр с фонариком показан");
    }
    for _ in 0..6 {
        rig.render();
    }
    let after = rig.viewfinder();
    let after_ae = after.auto_exposure.expect("AE");
    assert!(
        after_ae.counters.measurements_withheld_for_led_light >= 1,
        "{after_ae:?}"
    );
    assert!(after.counters.led_lit_frames_withheld_from_smoothing >= 1);
    assert_eq!(after.counters.flash_frames_skipped, 0);
    let target_after = rig.proposals.latest().expect("предложение").1.target.seconds_iso();
    let drift_ev = (target_after / target_before).log2().abs();
    assert!(
        drift_ev < 0.2,
        "цель видоискателя ушла к свету фонарика на {drift_ev} EV; было {before_ae:?}"
    );

    let (_, samples) = rig.proposals.preflash_newer_than(None).expect("замеры предвспышки");
    let ambient = samples.ambient.expect("замер без фонарика");
    let lit = samples.torch.expect("замер с фонариком");
    assert_eq!(
        (ambient.light, lit.light),
        (PreflashLight::Ambient, PreflashLight::Torch)
    );
    // Сцена с фонариком в 8 раз ярче: до цели ей нужно в 8 раз меньше произведения (линейно, 3 EV).
    let ev_gap = lit.mid_gray_product.ev_above(ambient.mid_gray_product);
    assert!(
        (ev_gap + 3.0).abs() < 0.4,
        "разница линейных замеров {ev_gap} EV, ждали −3"
    );
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

// ---- способ усреднения (сессия 13c, решение 133) ----

/// Сцена 13c в миниатюре: стол под лампой (центр, 10×8 плиток) в 32 раза светлее тёмной комнаты вокруг.
fn lamp_lit_table_in_a_dark_room() -> Vec<f64> {
    (0..TILE_COUNT)
        .map(|index| {
            let (column, row) = (index % 32, index / 32);
            if (11..21).contains(&column) && (8..16).contains(&row) {
                1.0
            } else {
                1.0 / 32.0
            }
        })
        .collect()
}

/// Средняя отражательная способность сцены по весам матрицы: обычное среднее и среднее геометрическое.
fn matrix_means(reflectance: &[f64]) -> (f64, f64) {
    let weights = MeteringWeights::build(
        TileGrid { columns: 32, rows: 24 },
        TilePlacement::full_frame(4.0 / 3.0).expect("4:3"),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .expect("веса матрицы");
    let total: f64 = weights.weights().iter().map(|&weight| f64::from(weight)).sum();
    let weighted = |value: &dyn Fn(f64) -> f64| {
        weights
            .weights()
            .iter()
            .zip(reflectance)
            .map(|(&weight, &tile)| f64::from(weight) * value(tile))
            .sum::<f64>()
            / total
    };
    (weighted(&|tile| tile), weighted(&|tile| tile.log2()).exp2())
}

#[test]
fn a_lamp_lit_table_in_a_dark_room_converges_to_the_average_light_not_to_the_log_mean() {
    // Причина «×12» из 13c, сквозь настоящий шейдер статистики: на контрастной сцене контур обязан сойтись к
    // экспозиции обычного среднего света. Опора — та же камера на ровной сцене (цель ≈ 1 с·ISO): у неё оба
    // способа совпадают, поэтому отношение целей зависит только от способа усреднения.
    let Some((uniform, uniform_journal)) = converged_rig() else {
        return;
    };
    let uniform_target = uniform.proposals.latest().expect("предложение").1.target.seconds_iso();
    uniform.finish(&uniform_journal);

    let reflectance = lamp_lit_table_in_a_dark_room();
    let (linear_mean, geometric_mean) = matrix_means(&reflectance);
    let Some((mut rig, journal)) = rig_with(RigSetup {
        auto_exposure: Some(AutoExposureConfig::default()),
        white_balance: Some(manual_lamp_white()),
        scene: "checker_bb2800",
        start_product: DARK_START_PRODUCT,
        scene_scale: SCENE_SCALE,
        allow_clipping: false,
        layout: SceneLayout::PerTile(reflectance),
        stream: RigStream::Reference,
    }) else {
        return;
    };
    rig.run(120);
    let diagnostics = rig.viewfinder().auto_exposure.expect("AE включён");
    assert_eq!(
        diagnostics.convergence,
        Some(ProposalConvergence::Settled),
        "{diagnostics:?}"
    );
    let target = rig.proposals.latest().expect("предложение").1.target.seconds_iso();
    let measured_ev = (target / uniform_target).log2();
    let linear_ev = (1.0 / linear_mean).log2();
    let log_ev = (1.0 / geometric_mean).log2();
    assert!(
        log_ev - linear_ev > 1.5,
        "сцена различает способы: обычное {linear_ev:.2} EV, лог {log_ev:.2} EV"
    );
    assert!(
        (measured_ev - linear_ev).abs() < 0.15,
        "контур сошёлся к обычному среднему: {measured_ev:.2} EV (обычное {linear_ev:.2}, лог {log_ev:.2})"
    );
    assert!(
        rig.history
            .iter()
            .rev()
            .take(10)
            .any(|shot| shot.display_luminance.is_some()),
        "кадры сцены дошли до экрана"
    );
    rig.finish(&journal);
}
