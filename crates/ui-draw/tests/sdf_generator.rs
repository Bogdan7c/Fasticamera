//! Генератор SDF против аналитических формул: круг, квадрат, кольцо-штрих, обмотка, концы штриха.

// Это тест: вспомогательные функции падают через expect/panic, как и сами проверки.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use support::sdf::{
    GridSize, LineCap, Point, StrokePolyline, decode_distance, encode_distance, fill_distance_field,
    stroke_distance_field,
};

const SPREAD: f64 = 8.0;

/// Правильный многоугольник, вписанный в круг: направление обмотки задаёт знак `direction`.
fn circle_polygon(center: Point, radius: f64, direction: f64) -> Vec<Point> {
    let corners = 720;
    (0..corners)
        .map(|index| {
            let angle = direction * std::f64::consts::TAU * index as f64 / corners as f64;
            Point::new(center.x + radius * angle.cos(), center.y + radius * angle.sin())
        })
        .collect()
}

fn texel_center(column: usize, row: usize) -> Point {
    Point::new(column as f64 + 0.5, row as f64 + 0.5)
}

/// Максимальное отклонение поля от аналитической формулы в полосе `|d| < SPREAD - 1`.
fn max_error_in_band(field: &[f32], grid: GridSize, analytic: impl Fn(Point) -> f64) -> f64 {
    let mut worst: f64 = 0.0;
    let mut checked = 0;
    for row in 0..grid.height {
        for column in 0..grid.width {
            let expected = analytic(texel_center(column, row));
            if expected.abs() < SPREAD - 1.0 {
                worst = worst.max((f64::from(field[row * grid.width + column]) - expected).abs());
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "в полосу попало слишком мало текселей: {checked}");
    worst
}

#[test]
fn circle_fill_matches_analytic_sdf_within_half_texel() {
    let grid = GridSize { width: 80, height: 80 };
    let center = Point::new(40.0, 40.0);
    let radius = 20.0;
    let field = fill_distance_field(grid, &[circle_polygon(center, radius, 1.0)], SPREAD);

    let error = max_error_in_band(&field, grid, |point| {
        (point.x - center.x).hypot(point.y - center.y) - radius
    });
    assert!(error < 0.5, "круг: отклонение {error} текселя");
    // Центр глубоко внутри: расстояние обрезано до -spread и кодируется байтом 1.
    assert_eq!(encode_distance(field[40 * 80 + 40], SPREAD), 1);
    // Угол сетки далеко снаружи.
    assert_eq!(encode_distance(field[0], SPREAD), 255);
}

#[test]
fn square_fill_matches_analytic_sdf() {
    let grid = GridSize { width: 60, height: 60 };
    let square = vec![
        Point::new(15.0, 15.0),
        Point::new(45.0, 15.0),
        Point::new(45.0, 45.0),
        Point::new(15.0, 45.0),
    ];
    let field = fill_distance_field(grid, &[square], SPREAD);

    let error = max_error_in_band(&field, grid, |point| {
        let outside_x = (point.x - 30.0).abs() - 15.0;
        let outside_y = (point.y - 30.0).abs() - 15.0;
        outside_x.max(0.0).hypot(outside_y.max(0.0)) + outside_x.max(outside_y).min(0.0)
    });
    assert!(error < 0.01, "квадрат: отклонение {error} текселя");
}

#[test]
fn ring_stroke_matches_analytic_sdf() {
    let grid = GridSize { width: 80, height: 80 };
    let center = Point::new(40.0, 40.0);
    let (radius, half_width) = (20.0, 3.0);
    let ring = StrokePolyline {
        points: circle_polygon(center, radius, 1.0),
        closed: true,
    };
    let field = stroke_distance_field(grid, &[ring], half_width, LineCap::Round, SPREAD);

    let error = max_error_in_band(&field, grid, |point| {
        ((point.x - center.x).hypot(point.y - center.y) - radius).abs() - half_width
    });
    assert!(error < 0.5, "кольцо: отклонение {error} текселя");
    // Тело штриха — внутри, дырка кольца и внешняя область — снаружи.
    let on_ring = field[40 * 80 + 60];
    let in_hole = field[40 * 80 + 40];
    assert!(on_ring < 0.0, "середина штриха должна быть внутри: {on_ring}");
    assert!(in_hole > 0.0, "дырка кольца должна быть снаружи: {in_hole}");
}

#[test]
fn opposite_winding_cuts_a_hole_and_same_winding_fills_it() {
    let grid = GridSize { width: 80, height: 80 };
    let center = Point::new(40.0, 40.0);
    let outer = circle_polygon(center, 25.0, 1.0);

    // «Буква O»: внутренний контур обходится в обратную сторону — ненулевая обмотка даёт дырку.
    let with_hole = fill_distance_field(grid, &[outer.clone(), circle_polygon(center, 12.0, -1.0)], SPREAD);
    assert!(with_hole[40 * 80 + 40] > 0.0, "центр дырки должен быть снаружи формы");
    assert!(
        with_hole[40 * 80 + 40 + 18] < 0.0,
        "тело между контурами должно быть внутри"
    );

    // Тот же внутренний контур с тем же направлением: обмотка 2, дырки нет.
    let without_hole = fill_distance_field(grid, &[outer, circle_polygon(center, 12.0, 1.0)], SPREAD);
    assert!(
        without_hole[40 * 80 + 40] < 0.0,
        "при одинаковой обмотке центр остаётся внутри"
    );
}

#[test]
fn line_caps_differ_beyond_the_endpoint() {
    let grid = GridSize { width: 60, height: 40 };
    let line = StrokePolyline {
        points: vec![Point::new(10.0, 20.0), Point::new(40.0, 20.0)],
        closed: false,
    };
    let half_width = 4.0;
    let field_for = |cap| stroke_distance_field(grid, std::slice::from_ref(&line), half_width, cap, SPREAD);
    let butt = field_for(LineCap::Butt);
    let round = field_for(LineCap::Round);
    let square = field_for(LineCap::Square);
    // Тексель (42, 19) — центр (42.5, 19.5): 2.5 за концом вдоль оси, на 0.5 выше оси.
    let probe = 19 * 60 + 42;
    assert!((f64::from(butt[probe]) - 2.5).abs() < 0.01, "butt: {}", butt[probe]);
    assert!(round[probe] < 0.0, "round: точка в полукруге, {}", round[probe]);
    assert!(square[probe] < 0.0, "square: точка в выносе, {}", square[probe]);
    // Тексель (43, 16) — центр (43.5, 16.5): 3.5 за концом и 3.5 от оси (ось y = 20).
    // Внутри квадратного выноса (оба смещения < 4), но вне круглого конца (гипотенуза ≈ 4.95 > 4).
    let corner = 16 * 60 + 43;
    assert!(square[corner] < 0.0, "square: угол выноса внутри, {}", square[corner]);
    assert!(round[corner] > 0.0, "round: тот же угол снаружи, {}", round[corner]);
}

#[test]
fn encoding_round_trips_within_one_step_and_marks_the_edge_as_128() {
    assert_eq!(encode_distance(0.0, SPREAD), 128);
    assert_eq!(encode_distance(100.0, SPREAD), 255);
    assert_eq!(encode_distance(-100.0, SPREAD), 1);
    for tenth in -80..=80 {
        let distance = f64::from(tenth) / 10.0;
        let decoded = decode_distance(encode_distance(distance as f32, SPREAD), SPREAD);
        assert!(
            (decoded - distance).abs() <= SPREAD / 127.0 * 0.5 + 1e-6,
            "{distance} → {decoded}"
        );
    }
}
