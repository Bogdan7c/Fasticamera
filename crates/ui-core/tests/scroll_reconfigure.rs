#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Прокрутка: отмена жеста, индекс точки возврата, покой при смене сетки, атомарная
//! перенастройка. Дополняет `scroll_behavior.rs`.

use ui_core::scroll::{
    AxisGeometry, ReleaseOutcome, ScrollAxis, ScrollConfig, ScrollError, ScrollPhase, SnapSelection,
};
use ui_core::time::MonotonicNanos;

fn ms(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000 + millis * 1_000_000)
}

fn five_points(selection: SnapSelection, start: f64) -> ScrollAxis {
    let config = ScrollConfig {
        snap_selection: selection,
        ..ScrollConfig::default()
    };
    let mut scroll = ScrollAxis::new(&config, geometry(0.0, 400.0), start).unwrap();
    scroll
        .set_snap_points(&[300.0, 0.0, 100.0, 400.0, 200.0, 100.0], ms(0))
        .unwrap();
    scroll
}

fn geometry(min: f64, max: f64) -> AxisGeometry {
    AxisGeometry {
        min_offset: min,
        max_offset: max,
        viewport_extent: 960.0,
    }
}

/// Отмена и отпускание без скорости — один планировщик: исходы и траектории совпадают бит в бит,
/// в том числе за краем с точками (индекс точки возврата отдаётся готовым).
#[test]
fn cancel_equals_release_with_zero_velocity() {
    for drag in [-250.0, 0.0, 30.0, 130.0, 560.0] {
        let mut cancelled = five_points(SnapSelection::AdjacentOnly, 200.0);
        let mut released = five_points(SnapSelection::AdjacentOnly, 200.0);
        for scroll in [&mut cancelled, &mut released] {
            scroll.begin_drag(ms(0));
            scroll.drag_to(drag).unwrap();
        }
        let by_cancel = cancelled.cancel(ms(5)).unwrap();
        let by_release = released.release(0.0, ms(5)).unwrap();
        assert_eq!(by_cancel, by_release, "drag = {drag}");
        for at in [5, 50, 300, 10_000] {
            let (left, right) = (cancelled.sample(ms(at)), released.sample(ms(at)));
            assert_eq!(left.offset.to_bits(), right.offset.to_bits(), "drag = {drag}, t = {at}");
        }
    }
}

/// За краем с точками возврат отдаёт индекс точки, к которой едет контент: потребителю не нужно
/// искать её сравнением `f64 ==`.
#[test]
fn return_from_overscroll_reports_the_snap_point_index() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 400.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(300.0).unwrap();
    assert_eq!(
        scroll.release(0.0, ms(5)).unwrap(),
        ReleaseOutcome::ReturnsFromOverscroll {
            offset: 400.0,
            snap_index: Some(4)
        }
    );
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    scroll.begin_drag(ms(0));
    scroll.drag_to(-300.0).unwrap();
    assert_eq!(
        scroll.cancel(ms(5)).unwrap(),
        ReleaseOutcome::ReturnsFromOverscroll {
            offset: 0.0,
            snap_index: Some(0)
        }
    );
}

/// Покой после прилипания не уезжает при смене сетки точек и пределов, пока лежит в новых
/// границах (раньше успокоившаяся пружина перенацеливалась на «ближайшую новую точку»).
#[test]
fn settled_content_stays_put_when_snap_points_or_bounds_change() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 0.0);
    // Прилипание к точке 200 и полное успокоение пружины.
    scroll.begin_drag(ms(0));
    scroll.drag_to(190.0).unwrap();
    scroll.release(0.0, ms(10)).unwrap();
    assert_eq!(scroll.sample(ms(10_000)).offset, 200.0);

    // Сетка сдвинулась: ближайшая новая точка к 200 — 230, но стоящий контент остаётся на 200.
    scroll
        .set_snap_points(&[30.0, 130.0, 230.0, 330.0], ms(10_000))
        .unwrap();
    for at in [10_000, 10_010, 12_000] {
        let sample = scroll.sample(ms(at));
        assert_eq!((sample.offset, sample.phase), (200.0, ScrollPhase::Resting), "t = {at}");
    }
    // То же при смене границ, не затрагивающей положение.
    scroll.set_geometry(geometry(0.0, 500.0), ms(12_000)).unwrap();
    let sample = scroll.sample(ms(14_000));
    assert_eq!((sample.offset, sample.phase), (200.0, ScrollPhase::Resting));
    // А границы, оставившие контент снаружи, возвращают его пружиной.
    scroll.set_geometry(geometry(0.0, 100.0), ms(14_000)).unwrap();
    assert_eq!(scroll.sample(ms(14_001)).phase, ScrollPhase::ReturningFromEdge);
    assert_eq!(scroll.sample(ms(30_000)).offset, 100.0);
}

/// `reconfigure` меняет границы и точки вместе, а при ошибке не меняет ничего: ни границ, ни точек,
/// ни движения.
#[test]
fn reconfigure_is_atomic() {
    let mut scroll = five_points(SnapSelection::NearestToProjection, 200.0);
    let geometry_before = scroll.geometry();
    let points_before = scroll.snap_points().to_vec();

    let bad_points = [10.0, f64::NAN];
    assert!(matches!(
        scroll.reconfigure(geometry(0.0, 50.0), &bad_points, ms(0)),
        Err(ScrollError::InvalidSnapPoint(_))
    ));
    let bad_geometry = AxisGeometry {
        min_offset: 5.0,
        max_offset: 1.0,
        viewport_extent: 960.0,
    };
    assert!(matches!(
        scroll.reconfigure(bad_geometry, &[1.0], ms(0)),
        Err(ScrollError::InvalidBounds { .. })
    ));
    assert_eq!(
        (scroll.geometry(), scroll.snap_points()),
        (geometry_before, points_before.as_slice())
    );
    assert_eq!(scroll.sample(ms(1)).offset, 200.0);

    scroll
        .reconfigure(geometry(0.0, 600.0), &[0.0, 300.0, 600.0], ms(0))
        .unwrap();
    assert_eq!(scroll.geometry(), geometry(0.0, 600.0));
    assert_eq!(scroll.snap_points(), [0.0, 300.0, 600.0]);
}
