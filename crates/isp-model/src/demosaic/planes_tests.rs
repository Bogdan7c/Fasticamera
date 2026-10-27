use super::*;

#[test]
fn mirror_reflects_without_repeating_the_edge_sample() {
    // d c b | a b c d | c b a для длины 4: индексы -3..=6 → 3 2 1 | 0 1 2 3 | 2 1 0.
    let mirrored: Vec<usize> = (-3..=6).map(|index| mirror_index(index, 4)).collect();
    assert_eq!(mirrored, vec![3, 2, 1, 0, 1, 2, 3, 2, 1, 0]);
}

#[test]
fn mirror_handles_frames_shorter_than_the_kernel() {
    // Длина 2: период 2, отражение чередует 0 и 1 в обе стороны.
    let mirrored: Vec<usize> = (-3..=3).map(|index| mirror_index(index, 2)).collect();
    assert_eq!(mirrored, vec![1, 0, 1, 0, 1, 0, 1]);
}

fn ramp_row() -> Plane {
    // Одна «строка» из 4 отсчётов 1, 2, 4, 8 высотой 2 (вторая строка — десятикратная).
    Plane::from_fn(4, 2, |x, y| [1.0, 2.0, 4.0, 8.0][x] * if y == 0 { 1.0 } else { 10.0 })
}

#[test]
fn horizontal_convolution_flips_the_kernel_and_mirrors_the_border() {
    // Несимметричное ядро [1, 0, 0]: настоящая свёртка даёт out[i] = in[i + 1] (ядро перевёрнуто).
    let shifted = ramp_row().convolve_mirrored(Direction::Horizontal, &[1.0, 0.0, 0.0]);
    // Последний отсчёт берёт in[4] → зеркало → in[2] = 4.
    assert_eq!(
        (0..4).map(|x| shifted.at(x, 0)).collect::<Vec<_>>(),
        vec![2.0, 4.0, 8.0, 4.0]
    );
}

#[test]
fn vertical_convolution_runs_along_columns_only() {
    let plane = Plane::from_fn(2, 4, |x, y| (y * 10 + x) as f32);
    let shifted = plane.convolve_mirrored(Direction::Vertical, &[0.0, 0.0, 1.0]);
    // Ядро [0,0,1] даёт out[i] = in[i − 1]; верхняя строка берёт in[−1] → зеркало → in[1].
    assert_eq!(
        (0..4).map(|y| shifted.at(1, y)).collect::<Vec<_>>(),
        vec![11.0, 1.0, 11.0, 21.0]
    );
}

#[test]
fn forward_difference_uses_mirrored_neighbour_at_the_far_edge() {
    let difference = ramp_row().absolute_forward_difference(Direction::Horizontal);
    // |1−4|, |2−8|, |4−in[4→2]=4| = 0, |8−in[5→1]=2|.
    assert_eq!(
        (0..4).map(|x| difference.at(x, 0)).collect::<Vec<_>>(),
        vec![3.0, 6.0, 0.0, 6.0]
    );
}

#[test]
fn zero_padded_5x5_is_a_true_convolution_with_zeros_outside() {
    let mut kernel = [[0.0_f32; 5]; 5];
    kernel[0][2] = 1.0; // Ядро: единица в верхней строке по центру.
    let plane = Plane::from_fn(5, 5, |x, y| (y * 5 + x + 1) as f32);
    let result = plane.convolve_zero_padded_5x5(&kernel);
    // Свёртка: out[y][x] = in[y + 2][x]; нижние две строки уходят за край → 0.
    assert_eq!(result.at(1, 0), plane.at(1, 2));
    assert_eq!(result.at(3, 2), plane.at(3, 4));
    assert_eq!(result.at(3, 3), 0.0);
    assert_eq!(result.at(0, 4), 0.0);
}
