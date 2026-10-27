//! Качественный демозаик: Menon–Andriani–Calvagno 2007 (DDFAPD) с шагом уточнения.
//!
//! Источник: D. Menon, S. Andriani, G. Calvagno, «Demosaicing With Directional Filtering and
//! a posteriori Decision», IEEE Trans. Image Processing 16(1), 2007, doi:10.1109/TIP.2006.884928.
//!
//! # Паритет
//! Независимый эталон — Python-пакет colour-demosaicing 0.2.7 (BSD-3-Clause),
//! `demosaicing_CFA_Bayer_Menon2007(CFA, pattern, refining_step=True)`. Структура кода здесь своя
//! (проходы над плоскостями), но граничные правила и «странности» выбора H/V повторены один в один,
//! чтобы CPU-модель совпадала с эталоном до краёв кадра (см. `tests/demosaic_parity.rs`).
//! Места, где выбор направления взят «как в эталоне/статье», помечены в комментариях.
//!
//! # Идея
//! 1. Для каждого пикселя оцениваем зелёный двумя способами — по строке и по столбцу.
//! 2. По цветовым разностям (R−G, B−G) выбираем направление, где они меняются плавнее
//!    (a posteriori решение), и берём зелёный оттуда.
//! 3. Красный и синий восстанавливаем, интерполируя цветовые разности «цвет − зелёный».
//! 4. Шаг уточнения сглаживает разности вдоль выбранного направления и пересчитывает зелёный, R и B.
//!
//! # Значения вне [0, 1]
//! Результат НЕ обрезается: у резких краёв возможен лёгкий звон (чуть меньше 0 или больше 1).
//! Обрезка — дело стадии тона.

use camera_contract::geometry::PixelSize;

use super::planes::{Direction, Plane};
use crate::cfa::{BayerPattern, RgbChannel};
use crate::image::{LinearCameraRgb, WhiteBalancedMosaic, space};

/// Ядро `h0` из статьи: среднее двух соседей по линии (зелёные рядом с R/B).
const GREEN_NEIGHBOUR_AVERAGE_TAPS: [f32; 5] = [0.0, 0.5, 0.0, 0.5, 0.0];
/// Ядро `h1` из статьи: поправка по соседям через один (того же цвета, что центр):
/// `0.5·центр − 0.25·(левый2 + правый2)` — лапласиан центрального цвета.
const GREEN_LAPLACIAN_CORRECTION_TAPS: [f32; 5] = [-0.25, 0.0, 0.5, 0.0, -0.25];
/// Линейная интерполяция хроматического канала: среднее двух соседей через один отсчёт.
const CHROMA_NEIGHBOUR_AVERAGE_TAPS: [f32; 3] = [0.5, 0.0, 0.5];
/// Сглаживание цветовой разности на шаге уточнения: сумма трёх отсчётов, затем деление на 3 (FIR 1/3).
const REFINEMENT_SUM_TAPS: [f32; 3] = [1.0, 1.0, 1.0];
/// Делитель среднего трёх. Деление (а не умножение на 1/3) точно для плоского поля: 3a / 3 = a.
const REFINEMENT_WINDOW: f32 = 3.0;

/// Ядро классификатора 5×5 из статьи (для горизонтальных разностей; для вертикальных — транспонированное).
/// Веса растут к центру: ближние разности важнее дальних. Ядро несимметрично, поэтому
/// ориентация (свёртка переворачивает ядро) подтверждена тестом паритета.
const CLASSIFIER_KERNEL: [[f32; 5]; 5] = [
    [0.0, 0.0, 1.0, 0.0, 1.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 3.0, 0.0, 3.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0, 0.0, 1.0],
];

/// Решение «по какому направлению восстановлено» для каждого пикселя.
struct DirectionMap {
    width: usize,
    directions: Vec<Direction>,
}

impl DirectionMap {
    fn at(&self, x: usize, y: usize) -> Direction {
        self.directions[y * self.width + x]
    }
}

/// Три полные плоскости R, G, B.
struct ColorPlanes {
    red: Plane,
    green: Plane,
    blue: Plane,
}

/// Демозаик Menon 2007 с шагом уточнения; размер результата равен размеру мозаики.
pub(super) fn demosaic_menon2007(mosaic: &WhiteBalancedMosaic) -> LinearCameraRgb {
    let size = mosaic.size();
    let (width, height) = (size.width as usize, size.height as usize);
    let pattern = mosaic.pattern();
    let cfa = Plane::from_values(width, height, mosaic.samples().to_vec());

    // Шаги 1–2: зелёный по строке и по столбцу, выбор направления.
    let green_horizontal = directional_green_estimate(&cfa, pattern, Direction::Horizontal);
    let green_vertical = directional_green_estimate(&cfa, pattern, Direction::Vertical);
    let directions = directional_classifier(&cfa, pattern, &green_horizontal, &green_vertical);
    let green = select_green(&green_horizontal, &green_vertical, &directions);

    // Шаг 3: красный и синий (сначала в зелёных узлах, потом R↔B в узлах противоположного цвета).
    let red_sparse = sparse_channel(&cfa, pattern, RgbChannel::Red);
    let blue_sparse = sparse_channel(&cfa, pattern, RgbChannel::Blue);
    let red_at_green = fill_chroma_at_green(&red_sparse, &green, pattern, RgbChannel::Red);
    let blue_at_green = fill_chroma_at_green(&blue_sparse, &green, pattern, RgbChannel::Blue);
    let red = fill_chroma_at_opposite_chroma(&red_at_green, &blue_at_green, pattern, RgbChannel::Blue, &directions);
    let blue = fill_chroma_at_opposite_chroma(&blue_at_green, &red, pattern, RgbChannel::Red, &directions);

    // Шаг 4: уточнение.
    let refined = refine(ColorPlanes { red, green, blue }, pattern, &directions);
    assemble_image(size, refined)
}

/// Собирает изображение из плоскостей. Размер и число отсчётов совпадают с мозаикой по построению,
/// поэтому ошибка конструктора здесь невозможна.
fn assemble_image(size: PixelSize, planes: ColorPlanes) -> LinearCameraRgb {
    let pixels: Vec<[f32; 3]> = planes
        .red
        .into_values()
        .into_iter()
        .zip(planes.green.into_values())
        .zip(planes.blue.into_values())
        .map(|((red, green), blue)| [red, green, blue])
        .collect();
    LinearCameraRgb::from_stage_output(size, pixels, space::CameraRgb)
}

/// Разреженный канал: отсчёт мозаики там, где стоит фильтр `channel`, иначе 0.
fn sparse_channel(cfa: &Plane, pattern: BayerPattern, channel: RgbChannel) -> Plane {
    cfa.map_with_position(|x, y, sample| {
        if pattern.channel_at(x as u32, y as u32) == channel {
            sample
        } else {
            0.0
        }
    })
}

/// Оценка зелёного вдоль `direction` (`G_H` / `G_V` в статье).
///
/// В красных и синих узлах: `G = h0 * CFA + h1 * CFA`, то есть среднее двух зелёных соседей по линии
/// плюс лапласианная поправка центрального цвета. В зелёных узлах берётся сам отсчёт.
fn directional_green_estimate(cfa: &Plane, pattern: BayerPattern, direction: Direction) -> Plane {
    let neighbour_average = cfa.convolve_mirrored(direction, &GREEN_NEIGHBOUR_AVERAGE_TAPS);
    let laplacian_correction = cfa.convolve_mirrored(direction, &GREEN_LAPLACIAN_CORRECTION_TAPS);
    cfa.map_with_position(|x, y, sample| {
        if pattern.channel_at(x as u32, y as u32) == RgbChannel::Green {
            sample
        } else {
            neighbour_average.at(x, y) + laplacian_correction.at(x, y)
        }
    })
}

/// Цветовая разность «хроматический отсчёт − оценка зелёного» (`C_H` / `C_V`); в зелёных узлах 0.
fn chroma_minus_green(cfa: &Plane, pattern: BayerPattern, green_estimate: &Plane) -> Plane {
    cfa.zip_with(green_estimate, |sample, green| sample - green)
        .map_with_position(|x, y, difference| {
            if pattern.channel_at(x as u32, y as u32) == RgbChannel::Green {
                0.0
            } else {
                difference
            }
        })
}

/// Выбор направления (a posteriori решение статьи).
///
/// 1. `D = |C[i] − C[i+2]|` вдоль направления: насколько резко меняется разность цветов
///    (в паре красных или синих узлов через один отсчёт).
/// 2. Классификатор 5×5 суммирует `D` в окрестности с весами (`CLASSIFIER_KERNEL`):
///    `d_H = D_H ⊛ k`, `d_V = D_V ⊛ kᵀ`.
/// 3. Горизонталь выбирается, если вертикальные разности не меньше горизонтальных (`d_V ≥ d_H`,
///    как в эталоне — при равенстве побеждает горизонталь).
fn directional_classifier(
    cfa: &Plane,
    pattern: BayerPattern,
    green_horizontal: &Plane,
    green_vertical: &Plane,
) -> DirectionMap {
    let difference_horizontal =
        chroma_minus_green(cfa, pattern, green_horizontal).absolute_forward_difference(Direction::Horizontal);
    let difference_vertical =
        chroma_minus_green(cfa, pattern, green_vertical).absolute_forward_difference(Direction::Vertical);

    let classifier_horizontal = difference_horizontal.convolve_zero_padded_5x5(&CLASSIFIER_KERNEL);
    let classifier_vertical = difference_vertical.convolve_zero_padded_5x5(&transposed(&CLASSIFIER_KERNEL));

    let directions = classifier_vertical
        .as_slice()
        .iter()
        .zip(classifier_horizontal.as_slice())
        .map(|(&vertical, &horizontal)| {
            if vertical >= horizontal {
                Direction::Horizontal
            } else {
                Direction::Vertical
            }
        })
        .collect();
    DirectionMap {
        width: cfa.width(),
        directions,
    }
}

/// Транспонированное ядро 5×5.
fn transposed(kernel: &[[f32; 5]; 5]) -> [[f32; 5]; 5] {
    let mut result = [[0.0_f32; 5]; 5];
    for (row, kernel_row) in kernel.iter().enumerate() {
        for (column, &weight) in kernel_row.iter().enumerate() {
            result[column][row] = weight;
        }
    }
    result
}

/// Итоговый зелёный: из горизонтальной или вертикальной оценки по карте направлений.
fn select_green(green_horizontal: &Plane, green_vertical: &Plane, directions: &DirectionMap) -> Plane {
    green_horizontal.map_with_position(|x, y, horizontal| match directions.at(x, y) {
        Direction::Horizontal => horizontal,
        Direction::Vertical => green_vertical.at(x, y),
    })
}

/// Направление интерполяции R/B в зелёном узле строки `y` — «как в эталоне/статье».
///
/// Красный в зелёном узле красной строки лежит слева и справа (горизонталь), в зелёном узле
/// синей строки — сверху и снизу (вертикаль). Для синего наоборот. Решение классификатора тут
/// не участвует: соседи нужного цвета есть только в одном направлении.
fn chroma_direction_at_green(pattern: BayerPattern, y: usize, channel: RgbChannel) -> Direction {
    let row_has_wanted_colour = pattern.row_has_red(y as u32) == (channel == RgbChannel::Red);
    if row_has_wanted_colour {
        Direction::Horizontal
    } else {
        Direction::Vertical
    }
}

/// Красный/синий в зелёных узлах: `C = G + (C ⊛ k_b) − (G ⊛ k_b)` — интерполируем разность «цвет − зелёный».
/// В остальных узлах плоскость `sparse` не меняется.
fn fill_chroma_at_green(sparse: &Plane, green: &Plane, pattern: BayerPattern, channel: RgbChannel) -> Plane {
    let chroma_h = sparse.convolve_mirrored(Direction::Horizontal, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    let chroma_v = sparse.convolve_mirrored(Direction::Vertical, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    let green_h = green.convolve_mirrored(Direction::Horizontal, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    let green_v = green.convolve_mirrored(Direction::Vertical, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    sparse.map_with_position(|x, y, sample| {
        if pattern.channel_at(x as u32, y as u32) != RgbChannel::Green {
            return sample;
        }
        match chroma_direction_at_green(pattern, y, channel) {
            Direction::Horizontal => green.at(x, y) + chroma_h.at(x, y) - green_h.at(x, y),
            Direction::Vertical => green.at(x, y) + chroma_v.at(x, y) - green_v.at(x, y),
        }
    })
}

/// Красный в синих узлах (или синий в красных): `T = A + (T ⊛ k_b) − (A ⊛ k_b)` вдоль выбранного
/// классификатором направления, где `A` — «якорный» канал узла (`anchor_channel`), `T` — целевой.
/// Диагональные соседи после предыдущего шага уже заполнены в зелёных узлах.
fn fill_chroma_at_opposite_chroma(
    target: &Plane,
    anchor: &Plane,
    pattern: BayerPattern,
    anchor_channel: RgbChannel,
    directions: &DirectionMap,
) -> Plane {
    let target_h = target.convolve_mirrored(Direction::Horizontal, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    let target_v = target.convolve_mirrored(Direction::Vertical, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    let anchor_h = anchor.convolve_mirrored(Direction::Horizontal, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    let anchor_v = anchor.convolve_mirrored(Direction::Vertical, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
    target.map_with_position(|x, y, sample| {
        if pattern.channel_at(x as u32, y as u32) != anchor_channel {
            return sample;
        }
        match directions.at(x, y) {
            Direction::Horizontal => anchor.at(x, y) + target_h.at(x, y) - anchor_h.at(x, y),
            Direction::Vertical => anchor.at(x, y) + target_v.at(x, y) - anchor_v.at(x, y),
        }
    })
}

/// Сглаживает `plane` вдоль направления, выбранного для каждого пикселя (среднее трёх, FIR 1/3).
fn smooth_along_decision(plane: &Plane, directions: &DirectionMap) -> Plane {
    let smooth = |direction| {
        plane
            .convolve_mirrored(direction, &REFINEMENT_SUM_TAPS)
            .map_with_position(|_, _, sum| sum / REFINEMENT_WINDOW)
    };
    let smoothed_h = smooth(Direction::Horizontal);
    let smoothed_v = smooth(Direction::Vertical);
    plane.map_with_position(|x, y, _| match directions.at(x, y) {
        Direction::Horizontal => smoothed_h.at(x, y),
        Direction::Vertical => smoothed_v.at(x, y),
    })
}

/// Шаг уточнения (refining step статьи): три подпрохода в порядке эталона.
fn refine(planes: ColorPlanes, pattern: BayerPattern, directions: &DirectionMap) -> ColorPlanes {
    let green = refine_green(&planes, pattern, directions);
    let (red, blue) = refine_chroma_at_green(&planes, &green, pattern);
    let (red, blue) = refine_chroma_at_opposite_chroma(red, blue, pattern, directions);
    ColorPlanes { red, green, blue }
}

/// Уточнённый зелёный в красных и синих узлах: `G = C − smooth(C − G)` вдоль выбранного направления.
fn refine_green(planes: &ColorPlanes, pattern: BayerPattern, directions: &DirectionMap) -> Plane {
    let red_minus_green = planes.red.zip_with(&planes.green, |red, green| red - green);
    let blue_minus_green = planes.blue.zip_with(&planes.green, |blue, green| blue - green);
    let smoothed_red = smooth_along_decision(&red_minus_green, directions);
    let smoothed_blue = smooth_along_decision(&blue_minus_green, directions);
    planes
        .green
        .map_with_position(|x, y, green| match pattern.channel_at(x as u32, y as u32) {
            RgbChannel::Red => planes.red.at(x, y) - smoothed_red.at(x, y),
            RgbChannel::Blue => planes.blue.at(x, y) - smoothed_blue.at(x, y),
            RgbChannel::Green => green,
        })
}

/// Красный и синий в зелёных узлах по уточнённому зелёному: `C = G + (C − G) ⊛ k_b`.
/// Разности считаются один раз по входным R/B (как в эталоне) и читаются только в R/B-узлах.
fn refine_chroma_at_green(planes: &ColorPlanes, refined_green: &Plane, pattern: BayerPattern) -> (Plane, Plane) {
    let refine_channel = |channel_plane: &Plane, channel: RgbChannel| {
        let difference = channel_plane.zip_with(refined_green, |value, green| value - green);
        let difference_h = difference.convolve_mirrored(Direction::Horizontal, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
        let difference_v = difference.convolve_mirrored(Direction::Vertical, &CHROMA_NEIGHBOUR_AVERAGE_TAPS);
        channel_plane.map_with_position(|x, y, value| {
            if pattern.channel_at(x as u32, y as u32) != RgbChannel::Green {
                return value;
            }
            let smoothed = match chroma_direction_at_green(pattern, y, channel) {
                Direction::Horizontal => difference_h.at(x, y),
                Direction::Vertical => difference_v.at(x, y),
            };
            refined_green.at(x, y) + smoothed
        })
    };
    (
        refine_channel(&planes.red, RgbChannel::Red),
        refine_channel(&planes.blue, RgbChannel::Blue),
    )
}

/// Красный в синих узлах и синий в красных по разности `R − B`, сглаженной вдоль направления.
/// `R − B` считается один раз (синий в красных узлах берёт ту же разность — как в эталоне).
fn refine_chroma_at_opposite_chroma(
    red: Plane,
    blue: Plane,
    pattern: BayerPattern,
    directions: &DirectionMap,
) -> (Plane, Plane) {
    let red_minus_blue = red.zip_with(&blue, |red_value, blue_value| red_value - blue_value);
    let smoothed = smooth_along_decision(&red_minus_blue, directions);
    let refined_red = red.map_with_position(|x, y, value| {
        if pattern.channel_at(x as u32, y as u32) == RgbChannel::Blue {
            blue.at(x, y) + smoothed.at(x, y)
        } else {
            value
        }
    });
    let refined_blue = blue.map_with_position(|x, y, value| {
        if pattern.channel_at(x as u32, y as u32) == RgbChannel::Red {
            red.at(x, y) - smoothed.at(x, y)
        } else {
            value
        }
    });
    (refined_red, refined_blue)
}

#[cfg(test)]
#[path = "menon2007_tests.rs"]
mod tests;
