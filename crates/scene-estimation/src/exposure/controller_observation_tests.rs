//! Диагностика «сырой замер сцены» (08c): видна под блокировкой и в ручном режиме, на цель не влияет.
//!
//! Нужна приёмке на телефоне: доказать, что под блокировкой сцена действительно сменилась, а цель
//! (и экспозиция) стоит. Вспомогательные функции — из родительского модуля тестов контроллера.

use super::*;

#[test]
fn observed_scene_follows_the_scene_under_lock_while_the_target_stays_frozen() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        ..directive()
    });
    controller.ingest(&whole_frame_measurement(1, 0.01));
    controller.advance(ts(1));
    assert!(controller.is_holding());
    let frozen = controller.target_log2().unwrap();
    // Рука закрыла объектив: сцена темнее на 3 EV — нужно в 8 раз больше света.
    let darker = above(0.01, 3.0);
    assert_eq!(
        controller.ingest(&whole_frame_measurement(2, darker)),
        IngestOutcome::IgnoredHolding
    );
    run_frames(&mut controller, 2, 100);
    // Цель заморожена, а диагностика честно видит новую сцену.
    assert_eq!(controller.target_log2().unwrap(), frozen);
    let observed = controller.last_observed_log2().unwrap();
    assert!((observed - darker.log2()).abs() < 1e-9, "observed {observed}");
}

#[test]
fn observed_scene_is_updated_in_manual_mode_but_not_by_a_foreign_area() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(ExposureDirective {
        authority: ExposureAuthority::Manual,
        ..directive()
    });
    assert_eq!(
        controller.ingest(&whole_frame_measurement(3, 0.5)),
        IngestOutcome::IgnoredManual
    );
    assert!((controller.last_observed_log2().unwrap() - 0.5_f64.log2()).abs() < 1e-9);
    // Замер чужой области (старый тап) — не про текущую сцену: диагностика его не берёт.
    let foreign = measured(
        4,
        MeteredArea::for_point(tap(7).metering),
        2.0,
        MeasurementQuality::Measured,
    );
    assert_eq!(controller.ingest(&foreign), IngestOutcome::RejectedForeignArea);
    assert!((controller.last_observed_log2().unwrap() - 0.5_f64.log2()).abs() < 1e-9);
}

#[test]
fn observed_scene_is_forgotten_with_a_new_stream() {
    let mut controller = started_at(directive(), 0.01);
    assert!(controller.last_observed_log2().is_some());
    controller.begin_stream();
    assert!(controller.last_observed_log2().is_none());
}
