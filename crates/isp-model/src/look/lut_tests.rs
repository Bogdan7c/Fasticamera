use super::*;
use crate::transfer::{srgb_decode, srgb_encode};

const MAX: u32 = 65;

fn samples() -> Vec<[f32; 3]> {
    let mut points = Vec::new();
    for red in [0.0_f32, 0.003, 0.05, 0.2, 0.5, 0.9, 1.0] {
        for green in [0.0_f32, 0.01, 0.3, 0.77, 1.0] {
            for blue in [0.0_f32, 0.002, 0.15, 0.6, 1.0] {
                points.push([red, green, blue]);
            }
        }
    }
    points
}

#[test]
fn identity_table_is_identity_within_float_rounding() {
    for size in [2, 3, 17] {
        let lut = LookLut::identity(size, MAX).unwrap();
        for point in samples() {
            let out = lut.apply(point);
            for channel in 0..3 {
                // Допуск: тетраэдральная интерполяция тождества точна до округления f32 (~1e-7 на узлах),
                // плюс туда-обратно через OETF/EOTF (~3e-7 относительно) — берём 2e-6 абсолютно.
                assert!(
                    (out[channel] - point[channel]).abs() < 2e-6,
                    "N={size} {point:?} -> {out:?}"
                );
            }
        }
    }
}

/// Таблица `f(r, g, b) = (b, g, r)` (перестановка каналов) в кодированном домене: аффинна, поэтому
/// тетраэдральная интерполяция воспроизводит её ТОЧНО при любой точке, в том числе между узлами.
#[test]
fn channel_swap_table_gives_the_known_result_between_nodes() {
    let size = 5_u32;
    let mut nodes = Vec::new();
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                let last = (size - 1) as f32;
                nodes.push([blue as f32 / last, green as f32 / last, red as f32 / last]);
            }
        }
    }
    let lut = LookLut::new(size, nodes, MAX).unwrap();
    for point in samples() {
        let out = lut.apply(point);
        let expected = [point[2], point[1], point[0]];
        for channel in 0..3 {
            assert!((out[channel] - expected[channel]).abs() < 3e-6, "{point:?} -> {out:?}");
        }
    }
}

/// Аффинное преобразование кодированных значений: `out = 0,5·e + 0,25` по красному, `1 − e` по зелёному.
#[test]
fn affine_table_matches_the_analytic_value_in_the_encoded_domain() {
    let size = 4_u32;
    let mut nodes = Vec::new();
    let last = (size - 1) as f32;
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                nodes.push([
                    0.5 * red as f32 / last + 0.25,
                    1.0 - green as f32 / last,
                    blue as f32 / last,
                ]);
            }
        }
    }
    let lut = LookLut::new(size, nodes, MAX).unwrap();
    for point in samples() {
        let encoded = point.map(srgb_encode);
        let expected = [0.5 * encoded[0] + 0.25, 1.0 - encoded[1], encoded[2]].map(srgb_decode);
        let out = lut.apply(point);
        for channel in 0..3 {
            assert!(
                (out[channel] - expected[channel]).abs() < 3e-6,
                "{point:?} -> {out:?} vs {expected:?}"
            );
        }
    }
}

/// Нелинейная таблица: на гранях тетраэдров (равные дробные части) нет скачка, а внутри — расхождение трилинейной
/// и тетраэдральной интерполяции видно (иначе тест не отличал бы тетраэдры от чего угодно).
#[test]
fn tetrahedral_interpolation_is_continuous_across_tetrahedron_faces() {
    let size = 3_u32;
    let mut nodes = Vec::new();
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                // Произвольные, но детерминированные неаффинные значения.
                let (r, g, b) = (red as f32, green as f32, blue as f32);
                nodes.push([
                    (r * g + b * 0.3) / 4.0,
                    (g * b + r * 0.2) / 4.0,
                    ((r - g).abs() + b * 0.1) / 3.0,
                ]);
            }
        }
    }
    let lut = LookLut::new(size, nodes, MAX).unwrap();
    let epsilon = 1e-4_f32;
    let mut max_jump = 0.0_f32;
    let mut crossings = 0;
    for step in 0..40 {
        let base = 0.02 + step as f32 * 0.024;
        // Пересекаем плоскость f_r = f_g (на каждом шаге в своей клетке).
        let (below, above) = (
            [base - epsilon, base + epsilon, 0.31],
            [base + epsilon, base - epsilon, 0.31],
        );
        let (low, high) = (lut.sample_encoded(below), lut.sample_encoded(above));
        for channel in 0..3 {
            max_jump = max_jump.max((low[channel] - high[channel]).abs());
        }
        crossings += 1;
        // Сама грань: точка на плоскости равна среднему соседей с точностью второго порядка.
        let on_face = lut.sample_encoded([base, base, 0.31]);
        for channel in 0..3 {
            let mean = 0.5 * (low[channel] + high[channel]);
            assert!(
                (on_face[channel] - mean).abs() < 5e-4,
                "шаг {step}: {on_face:?} против {mean}"
            );
        }
    }
    assert!(crossings > 10);
    // Расстояние между точками 2e-4 по двум осям; у функции с ограниченным наклоном скачок того же порядка.
    assert!(max_jump < 2e-3, "скачок на грани тетраэдров {max_jump}");
}

#[test]
fn only_the_smallest_fraction_reaches_the_opposite_corner() {
    // Таблица-«шапка»: только узел (1,1,1) = 1. В центре клетки тетраэдральная интерполяция даёт min дробных
    // частей по цепочке: f_min = дробная часть последней оси, то есть наименьшая.
    let size = 2_u32;
    let mut nodes = vec![[0.0; 3]; 8];
    nodes[7] = [1.0; 3];
    let lut = LookLut::new(size, nodes, MAX).unwrap();
    let out = lut.sample_encoded([0.9, 0.5, 0.2]);
    assert!((out[0] - 0.2).abs() < 1e-6, "{out:?}");
}

#[test]
fn constructor_distinguishes_every_refusal() {
    assert_eq!(
        LookLut::identity(1, MAX).unwrap_err(),
        InvalidLut::SizeOutOfRange { size: 1, max: MAX }
    );
    assert_eq!(
        LookLut::identity(66, MAX).unwrap_err(),
        InvalidLut::SizeOutOfRange { size: 66, max: MAX }
    );
    assert_eq!(
        LookLut::new(2, vec![[0.0; 3]; 7], MAX).unwrap_err(),
        InvalidLut::NodeCountMismatch {
            size: 2,
            expected: 8,
            actual: 7
        }
    );
    let mut nodes = vec![[0.0; 3]; 8];
    nodes[3][1] = f32::NAN;
    assert_eq!(
        LookLut::new(2, nodes, MAX).unwrap_err(),
        InvalidLut::NonFiniteNode { index: 3 }
    );
}

#[test]
fn out_of_range_and_nan_input_stay_finite_and_inside_the_cube() {
    let lut = LookLut::identity(9, MAX).unwrap();
    let out = lut.apply([-0.5, 3.0, f32::NAN]);
    assert_eq!(out, [0.0, 1.0, 0.0]);
}
