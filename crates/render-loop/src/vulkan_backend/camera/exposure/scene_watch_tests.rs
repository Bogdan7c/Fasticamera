//! Слежение за сценой под точкой тапа на потоке рендера без GPU (решение 241): кого оно кормит, когда молчит и
//! что именно публикует движку. Кадры идут через настоящий `ExposureRunner` (план кадра, приём статистики).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::flash::{FlashModeEcho, FlashResult, FlashState};
use camera_contract::scene_change::{PointObsolete, PointObsoleteCause};
use camera_contract::stream::ReadoutAspect;
use camera_contract::three_a::{AeAfLock, MeteringRegion, RequestOrigin, SensorPoint};

use camera_contract::geometry::PixelSize;

use super::super::test_support::*;
use super::super::*;
use super::*;

const TORCH: FlashResult = FlashResult {
    mode: Some(FlashModeEcho::Torch),
    state: Some(FlashState::Fired),
};

/// Сцена «до»: ровная тёмная; «после»: ровная яркая (4 EV светлее) — панорама на другую сцену.
fn before() -> AwbStatistics {
    uniform_scene(DARK_VALUE)
}

fn after() -> AwbStatistics {
    uniform_scene(BRIGHT_VALUE)
}

/// Кадр стенда с заменой кадрирования статистики (зум).
fn step_cropped(harness: &mut Harness, crop: PixelRect, statistics: &AwbStatistics) {
    let passport = frame_at(&harness.stream, harness.next_index, Some(UNIT_EXPOSURE));
    harness.next_index += 1;
    let plan = harness.runner.plan_frame(&passport);
    let facts = ExposureFrameFacts {
        crop,
        ..facts_of(&passport)
    };
    harness
        .runner
        .ingest_statistics(plan.stream, identity_of(&passport), &facts, statistics);
}

fn tap(harness: &mut Harness, request: u64) {
    harness.directives.publish(point_directive(0.5, 0.5, request));
}

/// Последнее наблюдение движку и его поколение.
fn latest_observation(harness: &Harness) -> Option<(camera_contract::three_a::DirectiveGeneration, PointObsolete)> {
    harness.proposals.point_obsolete_newer_than(None)
}

fn published(harness: &Harness) -> u64 {
    harness.runner.diagnostics().counters.point_obsolete_published
}

fn references(harness: &Harness) -> u64 {
    harness.runner.diagnostics().counters.scene_references_taken
}

fn confirmed(harness: &Harness) -> u64 {
    harness.runner.diagnostics().counters.scene_changes_confirmed
}

fn camera_tag() -> CameraTag {
    CameraTag::of(&camera_contract::fixtures::camera_id("2"))
}

/// Метка сенсора кадра с номером `index` стенда.
fn timestamp_of(index: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(1_000_000_000 + index * FRAME_NANOS)
}

#[test]
fn a_changed_scene_under_the_tap_reaches_the_engine_with_the_tap_the_camera_and_the_confirming_frame() {
    let mut harness = Harness::new();
    tap(&mut harness, 5);
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    assert_eq!(references(&harness), 1, "эталон снят, когда AE устоялся");
    assert!(
        latest_observation(&harness).is_none(),
        "сцена прежняя — движку нечего сообщать"
    );

    let change_starts_at = harness.next_index;
    harness.steps(40, Some(UNIT_EXPOSURE), &after());
    let (_, observation) = latest_observation(&harness).expect("смена подтверждена и опубликована");
    assert_eq!(observation.request, PointRequestId(5));
    assert_eq!(observation.measured_on, camera_tag());
    assert_eq!(observation.cause, PointObsoleteCause::SceneChanged);
    // Подтверждение — не раньше 500 мс «не похож» по меткам сенсора (30 кадров по 16,7 мс) и не позже чем через кадр.
    let earliest = timestamp_of(change_starts_at + 30);
    assert!(
        observation.frame >= earliest && observation.frame <= timestamp_of(change_starts_at + 31),
        "кадр подтверждения {:?}, ожидали около {earliest:?}",
        observation.frame
    );
    let counters = harness.runner.diagnostics().counters;
    assert_eq!(counters.point_obsolete_published, 1);
    assert!(counters.scene_changes_confirmed >= 1);
}

#[test]
fn an_unchanged_scene_never_reports_however_long_the_tap_lives() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    harness.steps(600, Some(UNIT_EXPOSURE), &before());
    assert_eq!(references(&harness), 1);
    assert_eq!(confirmed(&harness), 0);
    assert_eq!(published(&harness), 0);
    assert!(latest_observation(&harness).is_none());
}

#[test]
fn a_report_the_engine_did_not_act_on_repeats_no_faster_than_the_confirmation_time_and_not_for_other_taps() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    harness.steps(40, Some(UNIT_EXPOSURE), &after());
    let (first_generation, _) = latest_observation(&harness).expect("первое наблюдение");
    assert_eq!(published(&harness), 1);
    // Точку не сняли (вспышка, серия): сцена всё ещё другая, но в пределах 500 мс повтора нет.
    harness.steps(20, Some(UNIT_EXPOSURE), &after());
    assert!(
        harness
            .proposals
            .point_obsolete_newer_than(Some(first_generation))
            .is_none()
    );
    assert_eq!(published(&harness), 1);
    // Через confirm_ms повтор уходит: потерянное наблюдение не залипнет.
    harness.steps(20, Some(UNIT_EXPOSURE), &after());
    let (_, repeated) = harness
        .proposals
        .point_obsolete_newer_than(Some(first_generation))
        .expect("повтор");
    assert_eq!(repeated.request, PointRequestId(1));
    assert_eq!(published(&harness), 2);

    // Другой тап — своя история: прежняя публикация его не задерживает, прежний эталон не действует.
    tap(&mut harness, 2);
    harness.steps(20, Some(UNIT_EXPOSURE), &after());
    assert_eq!(
        published(&harness),
        2,
        "эталон нового тапа снят с нынешней сцены: смены нет"
    );
    harness.steps(40, Some(UNIT_EXPOSURE), &before());
    let (_, for_new_tap) = latest_observation(&harness).expect("наблюдение нового тапа");
    assert_eq!(for_new_tap.request, PointRequestId(2));
    assert_eq!(published(&harness), 3);
}

#[test]
fn nothing_is_watched_without_a_tap() {
    let mut harness = Harness::new();
    harness.steps(60, Some(UNIT_EXPOSURE), &before());
    harness.steps(60, Some(UNIT_EXPOSURE), &after());
    assert_eq!(
        (references(&harness), confirmed(&harness), published(&harness)),
        (0, 0, 0)
    );
    assert!(latest_observation(&harness).is_none());
}

#[test]
fn a_locked_or_cleared_tap_is_not_watched_and_the_watch_restarts_from_a_fresh_reference() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    assert_eq!(references(&harness), 1);
    // Блокировка AE/AF: за сценой не следим, как бы она ни менялась.
    harness.directives.publish(ExposureDirective {
        lock: AeAfLock::Locked,
        ..point_directive(0.5, 0.5, 1)
    });
    harness.steps(100, Some(UNIT_EXPOSURE), &after());
    assert_eq!((confirmed(&harness), published(&harness)), (0, 0));
    // Блокировку сняли: сцена уже другая, но это новая точка отсчёта — прежний эталон забыт, смены нет.
    tap(&mut harness, 1);
    harness.steps(100, Some(UNIT_EXPOSURE), &after());
    assert_eq!(published(&harness), 0);
    assert_eq!(references(&harness), 2, "эталон снят заново после разблокировки");
    harness.steps(40, Some(UNIT_EXPOSURE), &before());
    assert_eq!(published(&harness), 1, "после разблокировки слежение работает");

    // Точку сняли (весь кадр): эталон тоже забыт, возвращённая точка начинает с чистого листа.
    harness.directives.publish(ExposureDirective::DEFAULT);
    harness.steps(60, Some(UNIT_EXPOSURE), &after());
    tap(&mut harness, 1);
    harness.steps(100, Some(UNIT_EXPOSURE), &after());
    assert_eq!(published(&harness), 1, "новый эталон снят с нынешней сцены");
    assert_eq!(references(&harness), 3);
}

#[test]
fn a_locked_tap_is_never_watched_from_the_start() {
    let mut harness = Harness::new();
    harness.directives.publish(ExposureDirective {
        lock: AeAfLock::Locked,
        ..point_directive(0.5, 0.5, 1)
    });
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    harness.steps(100, Some(UNIT_EXPOSURE), &after());
    assert_eq!(
        (references(&harness), confirmed(&harness), published(&harness)),
        (0, 0, 0)
    );
}

#[test]
fn frames_lit_by_the_led_do_not_feed_the_watch() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    // Фонарик предвспышки освещает сцену: яркий кадр — не смена сцены.
    for _ in 0..90 {
        let passport = flash_frame_at(
            &harness.stream,
            harness.next_index,
            Some(UNIT_EXPOSURE),
            RequestOrigin::Repeating,
            TORCH,
        );
        harness.next_index += 1;
        let plan = harness.runner.plan_frame(&passport);
        harness
            .runner
            .ingest_statistics(plan.stream, identity_of(&passport), &facts_of(&passport), &after());
    }
    assert_eq!((confirmed(&harness), published(&harness)), (0, 0));
    // Фонарик погас, сцена прежняя: по-прежнему тихо и эталон цел.
    harness.steps(60, Some(UNIT_EXPOSURE), &before());
    assert_eq!((confirmed(&harness), published(&harness)), (0, 0));
    assert_eq!(references(&harness), 1);
}

#[test]
fn frames_without_a_paired_exposure_do_not_feed_the_watch() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    // Яркая сцена без выдержки и ISO: её свет не к чему отнести — не смена.
    harness.steps(90, None, &after());
    assert_eq!((confirmed(&harness), published(&harness)), (0, 0));
    harness.steps(60, Some(UNIT_EXPOSURE), &before());
    assert_eq!(published(&harness), 0);
}

#[test]
fn the_watch_runs_before_the_measurement_gates_manual_authority_and_a_busy_meter_do_not_stop_it() {
    // Ручная экспозиция: замер ничего не решает, но точка фокуса всё равно устаревает со сценой.
    let mut manual = Harness::new();
    manual.directives.publish(ExposureDirective {
        authority: ExposureAuthority::Manual,
        ..point_directive(0.5, 0.5, 1)
    });
    // AE не сходится (ручной режим): эталон по сроку 1500 мс, то есть через ~90 кадров.
    manual.steps(100, Some(UNIT_EXPOSURE), &before());
    assert_eq!(references(&manual), 1, "эталон по сроку, а не по Settled");
    manual.steps(40, Some(UNIT_EXPOSURE), &after());
    assert_eq!(published(&manual), 1);

    // Исполнитель замера занят всегда: до замера кадры не доходят, слежение работает.
    struct AlwaysBusy;
    impl ExposureMeter for AlwaysBusy {
        fn submit(
            &mut self,
            _request: ExposureMeteringRequest<'_>,
        ) -> Result<(), scene_estimation::exposure::SubmitError> {
            Err(scene_estimation::exposure::SubmitError::Busy)
        }
        fn poll(&mut self) -> Option<scene_estimation::exposure::CompletedExposureMeasurement> {
            None
        }
    }
    let mut busy = Harness::new();
    busy.runner.meter = Box::new(AlwaysBusy);
    tap(&mut busy, 1);
    busy.steps(100, Some(UNIT_EXPOSURE), &before());
    busy.steps(40, Some(UNIT_EXPOSURE), &after());
    assert_eq!(published(&busy), 1);
}

#[test]
fn zooming_without_losing_the_tap_place_retakes_the_reference_and_publishes_nothing() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    // Зум ×2: плитки легли на середину кадра (место тапа — центр — осталось в кадре), статистика другая.
    let zoomed = PixelRect {
        left: 16,
        top: 12,
        size: PixelSize::new(32, 24),
    };
    for _ in 0..90 {
        step_cropped(&mut harness, zoomed, &after());
    }
    assert_eq!(
        (confirmed(&harness), published(&harness)),
        (0, 0),
        "зум сам точку не снимает"
    );
    assert_eq!(references(&harness), 2, "эталон переснят под новое кадрирование");
    // После зума настоящая смена сцены по-прежнему ловится.
    for _ in 0..40 {
        step_cropped(&mut harness, zoomed, &before());
    }
    assert_eq!(published(&harness), 1);
}

#[test]
fn a_tap_place_that_zoom_pushed_out_of_the_frame_is_reported_at_once_and_repeated_only_after_the_confirmation_time() {
    let mut harness = Harness::new();
    // Тап в левом верхнем углу кадра.
    harness.directives.publish(point_directive(0.1, 0.1, 9));
    harness.steps(20, Some(UNIT_EXPOSURE), &before());
    assert_eq!(published(&harness), 0, "пока место тапа в кадре, всё тихо");
    let centre_zoom = PixelRect {
        left: 16,
        top: 12,
        size: PixelSize::new(32, 24),
    };
    step_cropped(&mut harness, centre_zoom, &before());
    let (first_generation, observation) = latest_observation(&harness).expect("регион вне кадра сообщён сразу");
    assert_eq!(observation.request, PointRequestId(9));
    assert_eq!(observation.cause, PointObsoleteCause::RegionOutsideFrame);
    assert_eq!(observation.measured_on, camera_tag());
    // Повторов раньше confirm_ms нет.
    for _ in 0..10 {
        step_cropped(&mut harness, centre_zoom, &before());
    }
    assert!(
        harness
            .proposals
            .point_obsolete_newer_than(Some(first_generation))
            .is_none()
    );
    // Через 500 мс (30 кадров) наблюдение повторяется: движок мог его не принять.
    for _ in 0..25 {
        step_cropped(&mut harness, centre_zoom, &before());
    }
    let (repeat_generation, _) = harness
        .proposals
        .point_obsolete_newer_than(Some(first_generation))
        .expect("повтор");
    // Другой тап, чей регион тоже вне кадра, сообщается сразу: историю повторов прежнего тапа он не наследует.
    harness.directives.publish(point_directive(0.05, 0.05, 10));
    step_cropped(&mut harness, centre_zoom, &before());
    let (_, for_other_tap) = harness
        .proposals
        .point_obsolete_newer_than(Some(repeat_generation))
        .expect("наблюдение нового тапа");
    assert_eq!(for_other_tap.request, PointRequestId(10));
}

#[test]
fn a_region_outside_the_sixteen_by_nine_band_is_reported_even_under_a_lock() {
    let mut harness = Harness::new();
    harness.stream = stream_of(ReadoutAspect::SixteenByNine, 36);
    let region = MeteringRegion::new(SensorPoint::new(0.5, 0.02).expect("точка"), 0.02).expect("регион");
    harness.directives.publish(ExposureDirective {
        metering: PointOfInterest::Point {
            region,
            request: PointRequestId(7),
        },
        lock: AeAfLock::Locked,
        ..ExposureDirective::DEFAULT
    });
    harness.step(Some(UNIT_EXPOSURE), &before());
    let (_, observation) = latest_observation(&harness).expect("факт «регион вне кадра» сообщается и под блокировкой");
    assert_eq!(observation.request, PointRequestId(7));
    assert_eq!(observation.cause, PointObsoleteCause::RegionOutsideFrame);
    // Смену сцены под блокировкой не наблюдаем.
    assert_eq!(confirmed(&harness), 0);
}

#[test]
fn a_new_stream_forgets_the_reference_and_statistics_of_the_old_stream_are_not_watched() {
    let mut harness = Harness::new();
    tap(&mut harness, 1);
    let old_plan = harness.steps(20, Some(UNIT_EXPOSURE), &before());
    assert_eq!(references(&harness), 1);
    // Камера сменилась: новый поток.
    harness.stream = new_stream();
    harness.steps(20, Some(UNIT_EXPOSURE), &after());
    assert_eq!(references(&harness), 2, "эталон нового потока снят заново с его сцены");
    assert_eq!(published(&harness), 0, "яркая сцена нового потока — не смена");
    // Запоздавшая статистика старого потока (слот был в полёте): чужая, не кормит слежение нового.
    let stale_before = harness.runner.diagnostics().counters.statistics_stale;
    let late = frame_at(&new_stream(), 500, Some(UNIT_EXPOSURE));
    for _ in 0..60 {
        harness
            .runner
            .ingest_statistics(old_plan.stream, identity_of(&late), &facts_of(&late), &before());
    }
    let counters = harness.runner.diagnostics().counters;
    assert_eq!(counters.statistics_stale, stale_before + 60);
    assert_eq!(
        (counters.scene_changes_confirmed, counters.point_obsolete_published),
        (0, 0)
    );
}

#[test]
fn a_tap_followed_at_once_by_another_scene_is_reported_without_waiting_for_the_exposure_to_settle() {
    let mut harness = Harness::new();
    tap(&mut harness, 8);
    // Эталон — первый кадр тапа; уже на втором кадре статистики камеру увели.
    harness.steps(2, Some(UNIT_EXPOSURE), &before());
    assert_eq!(
        references(&harness),
        1,
        "эталон снят на первом кадре, ждать схождения AE не нужно"
    );
    harness.steps(40, Some(UNIT_EXPOSURE), &after());
    let (_, observation) = latest_observation(&harness).expect("тап снят, хотя AE ещё не сошёлся");
    assert_eq!(observation.request, PointRequestId(8));
    assert_eq!(observation.cause, PointObsoleteCause::SceneChanged);
    assert_eq!(references(&harness), 1, "эталон за это время не переснимался");
}

#[test]
fn a_fresh_tap_is_not_blamed_for_the_outside_region_of_the_previous_tap() {
    let mut harness = Harness::new();
    harness.stream = stream_of(ReadoutAspect::SixteenByNine, 36);
    // Тап A по y = 0,02 вне сетки 16:9: сообщено.
    let region = MeteringRegion::new(SensorPoint::new(0.5, 0.02).expect("точка"), 0.02).expect("регион");
    harness.directives.publish(ExposureDirective {
        metering: PointOfInterest::Point {
            region,
            request: PointRequestId(1),
        },
        ..ExposureDirective::DEFAULT
    });
    harness.step(Some(UNIT_EXPOSURE), &before());
    assert_eq!(published(&harness), 1);
    // Тап B внутри кадра. Кадр без выдержки замер не ставит и весов области B не строит: флаг «вне сетки» от тапа A
    // остался бы поднятым, но к B он не относится.
    harness.directives.publish(point_directive(0.5, 0.5, 2));
    harness.step(None, &before());
    harness.step(None, &before());
    assert_eq!(published(&harness), 1, "про тап B ничего не опубликовано");
    // С выдержкой веса B построены (регион внутри) — тоже тихо.
    harness.steps(5, Some(UNIT_EXPOSURE), &before());
    assert_eq!(published(&harness), 1);
}

#[test]
fn the_outside_region_flag_of_a_zoomed_frame_is_not_reported_for_a_frame_with_the_full_crop() {
    let mut harness = Harness::new();
    harness.directives.publish(point_directive(0.1, 0.1, 3));
    harness.steps(5, Some(UNIT_EXPOSURE), &before());
    let centre_zoom = PixelRect {
        left: 16,
        top: 12,
        size: PixelSize::new(32, 24),
    };
    step_cropped(&mut harness, centre_zoom, &before());
    assert_eq!(published(&harness), 1, "при зуме место тапа вне кадра");
    // Зум снят, а кадры без выдержки весов не строят: «вне сетки» относится к прежнему кадрированию, не к этому.
    harness.steps(60, None, &before());
    assert_eq!(
        published(&harness),
        1,
        "для полного кадра повторов нет, хотя прошло больше confirm_ms"
    );
}
